import zmq
import json
import time
import uuid
import datetime

# Beijing Time (UTC+8)
BEIJING_TZ = datetime.timezone(datetime.timedelta(hours=8))

class ZmqPublisher:
    def __init__(self, config):
        self.config = config
        
        # Setup ZMQ Context
        self.context = zmq.Context()
        self.socket = self.context.socket(zmq.PUB)
        port = self.config['inference']['zmq_port']
        self.socket.bind(f"tcp://127.0.0.1:{port}")
        
        # Cooling State: map of symbol -> last_fired_timestamp
        self.last_fired = {}
        
        # Config Thresholds
        self.prob_reject = self.config['inference']['prob_reject']
        self.prob_record = self.config['inference']['prob_record_only']
        self.prob_t1 = self.config['inference']['prob_tier_1']
        self.prob_t2 = self.config['inference']['prob_tier_2']
        
        self.cooldown_std = self.config['inference']['cooldown_hours_standard'] * 3600
        self.cooldown_l3 = self.config['inference']['cooldown_hours_layer3'] * 3600

    def publish(self, symbol: str, prob: float, current_price: float, atr: float, is_long: bool = True, regime: str = "CHOP_LOW_VOL", registry_info: dict = None):
        now = time.time()
        now_bj = datetime.datetime.now(BEIJING_TZ).strftime("%Y-%m-%d %H:%M:%S")
        
        # Override with dynamic registry parameters if available
        is_new_symbol = False
        target_tier = "layer1"
        
        if registry_info:
            prob_reject = registry_info.get("prob_threshold", self.prob_reject)
            cooldown_duration = registry_info.get("cooldown_hours", 6) * 3600
            target_tier = registry_info.get("tier", "layer1")
            status = registry_info.get("status", "MATURE")
            
            if status != "MATURE":
                is_new_symbol = True
        else:
            prob_reject = self.prob_reject
            cooldown_duration = self.cooldown_std
        
        # --- Regime Modulation Logic ---
        if regime == "BEAR_TREND":
            if target_tier in ["layer2", "layer3"] and is_long:
                print(f"[REGIME-REJECT] BEAR_TREND 强行过滤多头信号: {symbol}")
                return
            if target_tier == "layer1" and not is_long:
                prob_reject = max(prob_reject, 0.70) # Stricter entry for Layer 1 shorts in BEAR_TREND
        elif regime == "BULL_TREND":
            if target_tier in ["layer2", "layer3"] and is_long:
                prob_reject = max(prob_reject * 0.92, 0.62)
        elif regime == "CHOP_HIGH_VOL":
            prob_reject = min(prob_reject + 0.05, 0.95)
        elif regime == "CHOP_LOW_VOL":
            # Shrink cooldown to 25% of the window
            cooldown_duration = cooldown_duration * 0.5 # Window was already Cooldown*2, so 25% of window is 50% of original cooldown
            
        # 1. Low Confidence Rejection (Predictive Rejection)
        if prob < prob_reject:
            print(f"[REJECT] 概率过低: prob={prob:.4f} < {prob_reject:.2f} for {symbol} (Regime: {regime})")
            return
            
        # 2. Record Only
        if prob < self.prob_record:
            print(f"[RECORD-ONLY] 观察数据: prob={prob:.4f} for {symbol}")
            return
            
        # 3. Cooling Mechanism
        cooldown_until_ts = 0
        if registry_info and "cooldown_until" in registry_info:
            try:
                dt = datetime.datetime.fromisoformat(registry_info["cooldown_until"])
                if dt.tzinfo is None:
                    dt = dt.replace(tzinfo=datetime.timezone.utc)
                cooldown_until_ts = dt.timestamp()
            except Exception:
                pass
                
        if now < cooldown_until_ts:
            hours_left = (cooldown_until_ts - now) / 3600
            print(f"[COOLING] 信号被注册表冷却拦截: {symbol} (剩余 {hours_left:.1f} 小时)")
            return
            
        last_time = self.last_fired.get(symbol, 0)
        if now - last_time < 300: # 5 minutes strict anti-spam memory cooling
            # Silent anti-spam return, prevents log flooding
            return
            
        # 4. Confidence Tier Assignment (for internal tracking)
        conf_tier = "T0 (50%)"
        if prob >= self.prob_t2:
            conf_tier = "T2 (150%)"
        elif prob >= self.prob_t1:
            conf_tier = "T1 (100%)"
            
        direction_str = "LONG" if is_long else "SHORT"
        print(f"[FIRE] 发射信号: {symbol} {direction_str} prob={prob:.4f} Tier={conf_tier} AssetTier={target_tier} IsNew={is_new_symbol}")
        
        # 5. Construct Payload (matching HyperQ Rust struct)
        msg_id = str(uuid.uuid4())
        timestamp_ms = int(now * 1000)
        
        # Format: msg_id|timestamp|symbol|price|atr_24h|is_long|raw_score|prob|features|tier|is_new_symbol|regime
        is_new_str = "true" if is_new_symbol else "false"
        is_long_str = "true" if is_long else "false"
        payload = f"{msg_id}|{timestamp_ms}|{symbol}|{current_price:.6f}|{atr:.6f}|{is_long_str}|{prob:.6f}|{prob:.6f}|{{}}|{target_tier}|{is_new_str}|{regime}"
        
        # 6. Publish via ZMQ
        self.socket.send_string(payload)
        self.last_fired[symbol] = now
        
