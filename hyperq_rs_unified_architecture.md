# HyperQ-RS V6.0 终极版：产品需求与全量架构说明书 (PRD & Architecture)

**文档版本**: V6.0 (双擎架构终极版)
**更新日期**: 2026-08-08
**系统定位**: 极致低延迟、基于微观盘口订单流 (L2/L3) 与 XGBoost/HMM 概率推理的 **“双擎智能高频/中频量化交易系统”**。

---

## 核心设计哲学 (Core Philosophy)

市场在绝大多数时间处于无序震荡（Chop），而在极少数时间处于单边爆发（Trend）。传统量化模型往往试图用一套逻辑打通吃尽，最终导致“赢4次震荡，死于1次单边”。

本系统采用**双擎架构 (Dual-Engine)**：
1. **AI 均值回归大脑 (Python)**：在震荡市中，利用盘口微观失衡 (OFI/CVD) 精准捕捉主力吸筹/派发的“均值回归”拐点，执行高抛低吸，抽干庄家的血。
2. **CTA 动能推土机 (Rust)**：在极端的单边突破市中，利用“唐奇安突破 (Donchian Breakout)”过滤器瞬间剥夺 AI 兵权，强制切换为趋势追踪模式，死咬 60% 甚至 100% 的主升浪/主跌浪。

---

## 一、 AI 推理大脑 (Python 侧)

Python 侧负责离线训练与实盘毫秒级在线推理，是整个系统的“参谋部”。

### 1. 核心特征工程 (Feature Engineering)
系统不依赖传统的 MACD/KDJ 等滞后指标，完全基于微观结构：
*   **OFI (Order Flow Imbalance)**: L2 深度盘口买卖挂单量的净失衡。捕捉“挂假单诱多/诱空”的动作。
*   **CVD (Cumulative Volume Delta)**: L3 逐笔成交的吃单净动量。捕捉“市价强吃”的真实资金意图。
*   **Liq-Imbalance (爆仓失衡)**: 监控币安全网爆仓流，捕捉散户集体爆仓带来的“流动性真空”与极速反转点。
*   **Price Action**: 15分钟级别 K 线实体动能、上下引线极值。

### 2. 模型架构
*   **基础预测层 (XGBoost / LightGBM)**：对上述因子进行非线性拟合，输出 `prob_long` (做多胜率) 和 `prob_short` (做空胜率)。
*   **隐马尔可夫状态层 (HMM)**：对全市场波动率进行聚类，输出当前的大盘 Regime (如 `LOW_VOL_CHOP`, `HIGH_VOL_TREND`)。

### 3. IPC 通信机制
*   为了极速响应，Python 与 Rust 之间完全放弃 HTTP 轮询，采用 **ZeroMQ (IPC/TCP)** 进行二进制毫秒级数据交互。

---

## 二、 底层极速执行器 (Rust 侧)

Rust 侧是整个系统的“一线作战部队”，负责直连交易所、毫秒级拦截、风控以及订单执行。

### 1. 全景深微秒雷达 (Data Ingestion)
Rust 进程在内存中维护全市场数百个币种的实时状态，零延迟查询：
*   `BinanceWsDepth`: 订阅 L2 盘口，计算实时点差 (Spread) 和 OFI。
*   `BinanceWsForceOrder`: 订阅全网爆仓，维护爆仓动能池。
*   **`BinanceWsTicker` (V6 新增)**: 订阅 24 小时极速行情流，实时计算 `price_change_pct`, `high_24h`, `low_24h`。

### 2. 信号拦截与红绿灯调度 (Veto & Regime Override)
当 Python 参谋部下达 `SHORT` 或 `LONG` 的概率指令后，Rust 作战部队会先过 4 道关卡：

*   **VETO 1: Spread 拦截**：若盘口点差 `> 0.15%`，绝对不开仓（防流动性干涸引发的滑点）。
*   **VETO 2: Pin-Bar 拦截**：提取最新 5m K线，若做空时发现正在深V反弹 `>0.8%`，无情拦截。
*   **VETO 3: 信号混淆拦截**：若 `|prob_long - prob_short| < 15%`，说明 AI 自己也很犹豫，直接丢弃信号。
*   **【V6.1 核心】VETO 4: Regime 宽域夺权 (Donchian Breakout Override)**：
    *   **触发条件**：若 24h 涨跌幅 `> 15%` **且** 当前价格处于 24h 最高点/最低点 `4%` 的火力区间内（防洗盘诱空）。
    *   **动作**：强制拦截 AI 可能发出的“摸顶做空”指令，无情反手满仓生成一个 `MOMENTUM_LONG (动能追多)` 或 `MOMENTUM_SHORT (破位追空)` 的独立订单，咬死趋势。

### 3. 多模态动态风控中枢 (Risk Guard V2)
系统执行极速循环（每 100 毫秒扫描一次账面盈亏），并根据订单是否带有 `is_momentum_trade` 标签采取两套截然不同的风控法则：

#### 【底线防线（无论模式）】
*   **Phase -1: 无条件硬止损 (-12% ROE)**：任何订单亏损触碰 12%，触发强制市价斩仓，保住本金。
*   **Phase 0: 刺客微观斩仓 (-5% ROE + CVD崩溃)**：无需等待 -12%，若亏损超 5% 且 L3 动量发现主力在疯狂跑路，提前微观斩仓。

#### 【模式 A：均值回归防线 (AI 发出的普通单)】
*   **Phase 0.5: 绝不亏钱 (Breakeven)**：只要历史最大浮盈 (MFE) 曾摸到 `+8%`，止损线自动上调至 `+1.5%`，锁死盈利。
*   **Phase 1 & 2: 阶梯绞肉机 (Trailing)**：MFE `> 10%` 启动。利润越大，回撤容忍度越小。当利润 `> 40%` 时，只给 15% 的利润回撤空间。防止高位震荡被反噬。

#### 【模式 B：推土机防线 (Regime 夺权后的动能单 - V6 新增)】
*   **Phase M: 宽体追踪止盈 (Wide Trailing)**：完全无视 Phase 0.5 和阶梯止盈。只要求 MFE `> 10%` 后，给予绝对 `-15%` 的宽体回撤空间。允许狗庄中途进行 10% 的插针洗盘，死死咬住可能高达 `+80%` 的巨浪。

---

## 三、 数据沉淀与闭环 (Data Logging)

所有真实在 AWS 发出的订单，在开平仓瞬间，Rust 会将当前的盘口微观环境同步写入 `/var/log/hyperq/trades.jsonl`：
*   包含 `spread_pct`, `cvd`, `ofi`, `entry_time`, `exit_time`, `unrealized_roe`, `is_momentum_trade` 等 20 余个字段。
*   **目的**：为下一代的 Python 强化学习模型 (RL) 提供最精准、带有滑点和真实盘口环境的 Reward 数据集，实现系统自我进化。

---
**THE END - "In volatility we trust."**
