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

class HMMProxy:
    """
    HMM 状态机：检测 TREND / CHOP 市场环境
    基于短期 ATR 相对于历史中位数的偏差来判断状态
    """
    def __init__(self):
        self.vol_history = []
        
    def detect_regime(self, features):
        try:
            atr = float(features[16]) if len(features) > 16 else 0.0
            self.vol_history.append(atr)
            if len(self.vol_history) > 100:
                self.vol_history.pop(0)
            if len(self.vol_history) < 10:
                return "TREND"
            med_vol = np.median(self.vol_history)
            if atr > med_vol * 1.2:
                return "TREND"
            elif atr < med_vol * 0.8:
                return "CHOP"
            else:
                return "CHOP_HIGH_VOL"
        except:
            return "TREND"


class TransformerProxy:
    """
    信号预测器 - 治本版
    
    核心升级：趋势成熟度惩罚（Exhaustion Penalty）
    
    对于一个刚刚启动的妖币（价格在低位，涨幅小）：
      - price_vs_range ≈ 0.2, change_24h_pct ≈ 0.05
      - exhaustion_penalty ≈ 0 → 信号完全不受压制
    
    对于一个已经涨完的妖币（价格在高位，涨幅巨大，比如 SNXX）：
      - price_vs_range ≈ 0.95, change_24h_pct ≈ 0.80
      - exhaustion_penalty ≈ 0.76 → 多头概率从 0.95 压制到约 0.23
      - 这意味着系统绝对不会追顶开仓
    """
    def __init__(self):
        pass
        
    def predict(self, features, regime, price_vs_range, change_24h_pct, oi_change_pct, liq_imbalance, taker_buy_ratio, distance_to_high):
        if regime == "CHOP":
            return 0.5, 0.5

        try:
            # L2 吃单失衡度（最重要的微观结构信号）
            ofi = float(features[-1]) if len(features) > 10 else 0.0
            
            # 15分钟动量
            ret = float(features[0]) if len(features) > 0 else 0.0

            # === 核心原始信号 ===
            edge = (ofi * 2.0) + (ret * 5.0)
            raw_prob_long = 0.5 + edge
            raw_prob_long = max(0.01, min(0.99, raw_prob_long))

            # === 治本层1：趋势成熟度惩罚（防追顶）===
            position_penalty = max(0.0, (price_vs_range - 0.6)) * 1.25
            move_penalty = min(0.6, max(0.0, (change_24h_pct - 0.2) * 1.5))
            exhaustion_penalty = max(position_penalty, move_penalty)
            prob_long = raw_prob_long * (1.0 - exhaustion_penalty)

            # === 治本层2：OI 变化率（区分真实建仓 vs 空头假突破）===
            # OI 上升（主力在加仓做多）→ 放大多头概率
            # OI 下降（空头被平仓）→ 上涨虚假，缩减多头概率
            if oi_change_pct > 0.02:  # OI 增加超过2%：真实趋势，增强信号
                oi_boost = min(0.15, oi_change_pct * 2.0)
                prob_long = min(0.99, prob_long + oi_boost)
            elif oi_change_pct < -0.01:  # OI 减少：空头被强平引起的假突破，削弱信号
                oi_penalty = min(0.3, abs(oi_change_pct) * 5.0)
                prob_long = prob_long * (1.0 - oi_penalty)

            # === 治本层3：强平踩踏流 (Liquidation Imbalance) ===
            # liq_imbalance = (空头爆仓量 - 多头爆仓量) / 总爆仓量
            # > 0.5 说明全市场在疯狂爆空头，产生机械被动买单，是极高确定性的多头动量
            if liq_imbalance > 0.5:
                prob_long = min(0.99, prob_long + 0.3 * liq_imbalance)
            elif liq_imbalance < -0.5: # 连环爆多头
                prob_long = prob_long * 0.5

            prob_short = 1.0 - prob_long

            # 对称处理做空
            if prob_short > prob_long:
                short_pos_penalty = max(0.0, (1.0 - price_vs_range - 0.6)) * 1.25
                short_move_penalty = min(0.6, max(0.0, ((-change_24h_pct) - 0.2) * 1.5))
                short_exhaustion = max(short_pos_penalty, short_move_penalty)
                prob_short = prob_short * (1.0 - short_exhaustion)
                
                # 做空方向上的强平动量
                if liq_imbalance < -0.5:
                    prob_short = min(0.99, prob_short + 0.3 * abs(liq_imbalance))
                elif liq_imbalance > 0.5:
                    prob_short = prob_short * 0.5
                    
                # 狙击手级惩罚1：假突破诱多识别
                if taker_buy_ratio < 0.45:
                    prob_long = prob_long * (taker_buy_ratio / 0.5)  # 削弱做多
                elif taker_buy_ratio > 0.55:
                    prob_short = prob_short * (0.5 / taker_buy_ratio) # 削弱做空
                    
                # 狙击手级惩罚2：多周期共振（强弩之末拒绝接盘）
                # 如果距离高点很近（< 2%）且涨幅巨大，大幅度削减做多
                if distance_to_high < 0.02 and change_24h_pct > 0.15:
                    prob_long *= 0.5
                    
                prob_long = max(0.01, min(0.99, prob_long))
                prob_short = max(0.01, min(0.99, prob_short))
                
                # Re-calculate after manual penalties
                prob_long_adj = prob_long / (prob_long + prob_short)
                prob_short_adj = prob_short / (prob_long + prob_short)
                
                prob_long = prob_long_adj
                prob_short = prob_short_adj

            # 归一化
            s = prob_long + prob_short
            prob_long = prob_long / s
            prob_short = prob_short / s
            
            return float(prob_long), float(prob_short)

        except Exception as e:
            return 0.5, 0.5


def main():
    context = zmq.Context()
    socket = context.socket(zmq.REP)
    socket.bind("tcp://127.0.0.1:5556")
    print("🚀 [AI ENGINE V4.1 治本版] Transformer/HMM 已启动，端口 5556")
    print("   ✅ 新增：趋势成熟度惩罚因子 (Exhaustion Penalty)")
    print("   ✅ 新增：价格位置感知 (price_vs_range)")
    print("   ✅ 新增：24H涨幅惩罚 (change_24h_pct)")

    hmm = HMMProxy()
    transformer = TransformerProxy()

    while True:
        try:
            message = socket.recv_string()
            data = json.loads(message)
            symbol = data.get("symbol", "UNKNOWN")
            features = data.get("features", [])
            
            # 新增宏观上下文特征（Rust 层传入）
            price_vs_range = float(data.get("price_vs_range", 0.5))
            change_24h_pct = float(data.get("change_24h_pct", 0.0))
            oi_change_pct  = float(data.get("oi_change_pct", 0.0))
            liq_imbalance  = float(data.get("liq_imbalance", 0.0))
            taker_buy_ratio = float(data.get("taker_buy_ratio", 0.5))
            distance_to_high = float(data.get("distance_to_high", 0.0))
            
            regime = hmm.detect_regime(features)
            p_long, p_short = transformer.predict(features, regime, price_vs_range, change_24h_pct, oi_change_pct, liq_imbalance, taker_buy_ratio, distance_to_high)
            
            if regime == "CHOP":
                p_long = 0.4
                p_short = 0.4
            
            # 诊断日志
            if price_vs_range > 0.8 or change_24h_pct > 0.3 or abs(oi_change_pct) > 0.02 or abs(liq_imbalance) > 0.5 or taker_buy_ratio < 0.45 or taker_buy_ratio > 0.55:
                print(f"🔍 [{symbol}] pos={price_vs_range:.2f} TB_Ratio={taker_buy_ratio:.2f} dist_H={distance_to_high:.1%} 涨幅={change_24h_pct:.1%} OI_Δ={oi_change_pct:+.2%} LIQ_IMB={liq_imbalance:+.2f} → p_long={p_long:.3f}")
            
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
