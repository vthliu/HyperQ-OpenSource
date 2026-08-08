import pandas as pd
import pandas_ta as ta
import numpy as np
import time

import os
import json

STATE_FILE = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'data', 'regime_state.json')

class RegimeDetector:
    def __init__(self):
        self.current_regime = "CHOP_LOW_VOL"
        self.last_detected_regime = "CHOP_LOW_VOL"
        self.consecutive_count = 0
        self.last_update = 0
        self.error_count = 0
        self._load_state()
        
    def _load_state(self):
        if os.path.exists(STATE_FILE):
            try:
                with open(STATE_FILE, 'r') as f:
                    state = json.load(f)
                    self.current_regime = state.get('current_regime', "CHOP_LOW_VOL")
                    self.last_detected_regime = state.get('last_detected_regime', "CHOP_LOW_VOL")
                    self.consecutive_count = state.get('consecutive_count', 0)
                    print(f"[REGIME] 已从本地恢复大盘状态: {self.current_regime} (连续确认: {self.consecutive_count}次)")
            except Exception as e:
                print(f"[REGIME] 恢复状态失败: {e}")
                
    def _save_state(self):
        try:
            with open(STATE_FILE, 'w') as f:
                json.dump({
                    'current_regime': self.current_regime,
                    'last_detected_regime': self.last_detected_regime,
                    'consecutive_count': self.consecutive_count,
                    'updated_at': time.time()
                }, f)
        except Exception as e:
            print(f"[REGIME] 保存状态失败: {e}")
        
    def get_regime(self):
        return self.current_regime
        
    def update(self, exchange):
        now = time.time()
        # Exponential backoff up to 10 minutes
        backoff_time = min(600, (2 ** self.error_count - 1) * 60)
        
        # Update once every 5 minutes (300 seconds), plus any backoff penalty
        if now - self.last_update < (300 + backoff_time):
            return
            
        try:
            params = {'symbol': 'BTCUSDT', 'interval': '1h', 'limit': 100}
            ohlcv = exchange.fapiPublicGetKlines(params)
            
            df = pd.DataFrame(ohlcv, columns=[
                'timestamp', 'open', 'high', 'low', 'close', 'volume',
                'quote_asset_volume', 'number_of_trades', 'taker_buy_quote_asset_volume',
                'ignore1', 'ignore2', 'ignore3'
            ])
            df['close'] = df['close'].astype(float)
            df['high'] = df['high'].astype(float)
            df['low'] = df['low'].astype(float)
            
            # Compute indicators
            df.ta.adx(length=14, append=True) # ADX_14, DMP_14, DMN_14
            df.ta.ema(length=20, append=True)
            df.ta.ema(length=50, append=True)
            df.ta.atr(length=14, append=True)
            df.ta.bbands(length=20, append=True) # BBL_20_2.0, BBM_20_2.0, BBU_20_2.0, BBB_20_2.0
            
            latest = df.iloc[-1]
            adx = latest.get('ADX_14', 0)
            dmp = latest.get('DMP_14', 0)
            dmn = latest.get('DMN_14', 0)
            ema20 = latest.get('EMA_20', 0)
            ema50 = latest.get('EMA_50', 0)
            close = latest['close']
            bbb = latest.get('BBB_20_2.0', 0) # Bollinger Band Width %
            
            # Classification Logic
            new_regime = "CHOP_LOW_VOL"
            if adx > 25:
                if dmp > dmn and close > ema20 and ema20 > ema50:
                    new_regime = "BULL_TREND"
                elif dmn > dmp and close < ema20 and ema20 < ema50:
                    new_regime = "BEAR_TREND"
                else:
                    new_regime = "CHOP_HIGH_VOL"
            else:
                # ADX <= 25 means no strong trend. Check BB width for volatility.
                # BTC BB width > 5% on 1h usually means high volatility chop.
                if bbb > 5.0:
                    new_regime = "CHOP_HIGH_VOL"
                else:
                    new_regime = "CHOP_LOW_VOL"
                    
            # State Lock Logic (Requires 2 consecutive identical readings)
            if new_regime == self.last_detected_regime:
                self.consecutive_count += 1
            else:
                self.consecutive_count = 1
                self.last_detected_regime = new_regime
                
            if self.consecutive_count >= 2:
                if self.current_regime != new_regime:
                    print(f"[REGIME] 🌍 大盘状态切换: {self.current_regime} -> {new_regime}")
                    self.current_regime = new_regime
            
            self._save_state()
            self.last_update = now
            self.error_count = 0
            
        except Exception as e:
            self.error_count += 1
            self.last_update = now
            print(f"[REGIME-ERROR] 状态检测失败: {e}. 次数: {self.error_count}, 退避中...")
