import zmq
import json
import time
import numpy as np

# =============================================================================
# HyperQ V4.0 AI 引擎 - 治本版
#
# 核心改造：注入"趋势成熟度感知"
# 原版问题：只看 OFI + 动量，完全不知道价格是在底部还是顶部
# 治本方案：加入 price_vs_range（价格位置）和 change_24h_pct（涨幅）
#           当价格处于高位且已大幅上涨时，大幅压制多头概率
# =============================================================================

class MTFEngine:
    def __init__(self):
        self.vol_history = []
        
    def extract_metrics(self, features):
        if not features or len(features) < 47:
            return {"atr": 0.0, "ret": 0.0, "rsi": 50.0, "sma50": 0.0, "close": 0.0, "macd_hist": 0.0}
        
        # 匹配 features.rs 的索引
        # [0]=sma5, [2]=sma50, [6]=macd_hist, [8]=rsi, [16]=atr, [40]=returns
        return {
            "close": float(features[0]), # Approximation via sma5
            "sma50": float(features[2]),
            "macd_hist": float(features[6]),
            "rsi": float(features[8]),
            "atr": float(features[16]),
            "ret": float(features[40]),
            "ofi": float(features[-1]) if len(features) > 47 else 0.0 # ofi is appended
        }

    def predict(self, f_15m, price_vs_range, change_24h_pct, oi_change_pct, liq_imbalance, taker_buy_ratio, distance_to_high):
        m_15m = self.extract_metrics(f_15m)

        # ========================================================
        # 1. 微观扳机点与订单流失衡 (15m 级别) - 寻找精确入场点
        # ========================================================
        micro_ret = m_15m["ret"]
        ofi = m_15m["ofi"]
        
        # ========================================================
        # 4. 核爆级动能穿透 (Nuclear Override) - 最高优先级
        # ========================================================
        # 多头穿透：空头极度连环爆仓，或者 (资金大幅涌入 且 价格急拉 且 OFI大额扫单)
        nuclear_long_override = (liq_imbalance > 0.85) or (
            oi_change_pct > 0.05 and micro_ret > 0.015 and ofi > 0.6
        )
        
        # 空头穿透：多头极度连环爆仓，或者 (大户极度撤退 且 价格拉高诱多 且 OFI大幅被砸)
        nuclear_short_override = (liq_imbalance < -0.85) or (
            taker_buy_ratio < 0.35 and ofi < -0.6 and change_24h_pct > 0.1
        )
        
        # 原始基础胜率 (基于 15m 的动量和吃单失衡)
        micro_edge = (ofi * 0.2) + (micro_ret * 5.0) 
        raw_prob_long = 0.5 + micro_edge
        
        # 优先级判决矩阵
        is_override = False
        if nuclear_long_override:
            raw_prob_long += 0.40 # 强行穿透，极大增加看多胜率
            is_override = True
        elif nuclear_short_override:
            raw_prob_long -= 0.40 # 强行穿透，极大增加看空胜率
            is_override = True
        else:
            if micro_ret > 0.01 and ofi > 0.3:
                raw_prob_long += 0.20
            elif micro_ret < -0.01 and ofi < -0.3:
                raw_prob_long -= 0.20
            else:
                raw_prob_long = 0.5 + (raw_prob_long - 0.5) * 0.5

        raw_prob_long = max(0.01, min(0.99, raw_prob_long))
        
        # ========================================================
        # 4. 治本层：动量衰竭惩罚与假突破过滤
        # ========================================================
        prob_long = raw_prob_long
        prob_short = 1.0 - raw_prob_long

        # 多头惩罚 (防追顶)
        if prob_long > prob_short:
            position_penalty = max(0.0, (price_vs_range - 0.7)) * 1.5
            move_penalty = min(0.6, max(0.0, (change_24h_pct - 0.15) * 2.0))
            prob_long = prob_long * (1.0 - max(position_penalty, move_penalty))
            
            # OI 上升确认趋势，下降识别假突破
            if oi_change_pct > 0.01: prob_long = min(0.99, prob_long + 0.1)
            elif oi_change_pct < -0.01: prob_long *= 0.7
                
            if taker_buy_ratio < 0.45: prob_long *= 0.5
                
        # 空头惩罚 (防追底)
        else:
            # 价格在极低位且跌幅巨大时，禁止做空
            short_pos_penalty = max(0.0, (0.3 - price_vs_range)) * 1.5
            short_move_penalty = min(0.6, max(0.0, (-change_24h_pct - 0.15) * 2.0))
            prob_short = prob_short * (1.0 - max(short_pos_penalty, short_move_penalty))
            
            # OI 逻辑对称
            if oi_change_pct > 0.01: prob_short = min(0.99, prob_short + 0.1)
            elif oi_change_pct < -0.01: prob_short *= 0.7
                
            if taker_buy_ratio > 0.55: prob_short *= 0.5

        # 强平踩踏流 (单向暴力推升)
        if liq_imbalance > 0.6: 
            prob_long = min(0.99, prob_long + 0.2)
            prob_short *= 0.5
        elif liq_imbalance < -0.6: 
            prob_short = min(0.99, prob_short + 0.2)
            prob_long *= 0.5

        prob_long = max(0.01, min(0.99, prob_long))
        prob_short = max(0.01, min(0.99, prob_short))
        
        s = prob_long + prob_short
        return float(prob_long / s), float(prob_short / s), "TREND"


def main():
    context = zmq.Context()
    socket = context.socket(zmq.REP)
    socket.bind("tcp://127.0.0.1:5556")
    print("🚀 [AI ENGINE V5.0 MTF版] 多周期共振引擎已启动，端口 5556")
    print("   ✅ 核心升级：修复动量取值 Bug，现在能正确发出【做空】信号！")
    print("   ✅ 核心升级：15m(单周期微观) + OFI失衡 + 极限追踪")

    engine = MTFEngine()

    while True:
        try:
            message = socket.recv_string()
            data = json.loads(message)
            symbol = data.get("symbol", "UNKNOWN")
            
            f_15m = data.get("features_15m", [])
            
            price_vs_range = float(data.get("price_vs_range", 0.5))
            change_24h_pct = float(data.get("change_24h_pct", 0.0))
            oi_change_pct  = float(data.get("oi_change_pct", 0.0))
            liq_imbalance  = float(data.get("liq_imbalance", 0.0))
            taker_buy_ratio = float(data.get("taker_buy_ratio", 0.5))
            distance_to_high = float(data.get("distance_to_high", 0.0))
            
            p_long, p_short, regime = engine.predict(
                f_15m, 
                price_vs_range, change_24h_pct, oi_change_pct, liq_imbalance, taker_buy_ratio, distance_to_high
            )
            
            if abs(p_long - p_short) > 0.2:
                print(f"🎯 [{symbol}] 刺客出击! p_long={p_long:.3f} p_short={p_short:.3f} | 24H涨幅={change_24h_pct:.1%} OFI={f_15m[-1] if f_15m else 0:.2f}")
            
            response = {
                "symbol": symbol,
                "prob_long": p_long,
                "prob_short": p_short,
                "regime": regime,
                "status": "success"
            }
            
            socket.send_string(json.dumps(response))
            
        except Exception as e:
            print(f"Error: {e}")
            socket.send_string(json.dumps({"status": "error", "message": str(e)}))

if __name__ == "__main__":
    main()
