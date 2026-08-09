# HyperQ: Industrial-Grade Quantitative Trading Engine

[中文说明 (Chinese Version)](#中文说明)

HyperQ is a high-frequency, institutional-grade execution engine for cryptocurrency perpetual futures, featuring a **Rust + Python** dual-engine architecture. It is designed to capture high-certainty momentum breakouts while strictly managing risk with HFT-level (High-Frequency Trading) precision.

## 🚀 Key Features

- **V7.0 Triple-Confluence Momentum FLIP Engine**: During micro 5m breakouts that confirm macro 24h trend reversals with Order Flow Imbalance (OFI), the Rust CTA engine overrides the AI's mean-reversion signals and dynamically flips to momentum trend-following trades. It passes over fakeouts to retain the AI's high win-rate mean-reversion chops.
- **0.1s Real-time Trailing Stop & L3 CVD Risk Guard**: The `RiskGuard V7.0` module in Rust iterates through memory-mapped WebSocket data every 100 milliseconds, ensuring profits are locked in instantly. It also uses L3 `@aggTrade` CVD (Cumulative Volume Delta) to detect whale dumping, triggering preemptive stop-loss instantly.
- **-12% Zero-Slippage Hard Stop Execution**: Upgraded in V6.2, all stop-loss orders are executed as instantaneous `cancel_all_orders` and `MARKET_CLOSE` to rigidly cap massive black-swan drawdowns, fully eliminating Maker-order slippage blind spots.
- **Pure 15m Momentum (Zero Network IO)**: Upgraded to 'Assassin Mode', the Python AI inference server analyzes pure 15m order flow (OFI) momentum, completely eliminating 4H/1H REST API network delay.

## 🛠️ Architecture

- `hyperq-rs` (Rust): Handles Binance WebSocket streams, local L2 order book construction, PnL math, and order execution.
- `transformer_hmm_server_remote.py` (Python): Processes historical K-lines and emits trading probabilities (`prob_long`, `prob_short`) via ZeroMQ to the Rust engine.

## ⚠️ Proprietary Notice (Anti-Alpha Decay)

This repository provides the **Framework and Execution Engine**. To protect against Alpha decay in live markets, the core **XGBoost/HMM Model Weights** and specific Asset Pool (`symbol_registry.json`) are **NOT** open-sourced.

You must build and train your own Machine Learning models or connect your own signal generator to the ZeroMQ socket defined in the Python script.

## 📦 Quick Start

1. Install Rust (`rustup`) and Python 3.
2. Rename `config.example.toml` to `config.toml` and fill in your Binance API keys.
3. Start the Rust Engine:
   ```bash
   cargo run --release
   ```
4. Start your Signal Server:
   ```bash
   python3 transformer_hmm_server_remote.py
   ```

---

# 中文说明

HyperQ 是一套专为加密货币永续合约打造的工业级量化执行引擎，采用 **Rust + Python** 双擎架构。它专门用于捕捉高确定性的主升浪，并以极端的纪律和 HFT（高频交易）级别的精度进行风控。

## 🚀 核心卖点

- **V7.0 三重共振动能翻转引擎 (FLIP)**：在发生“微观爆发 + 宏观破位 + 盘口验证”的三重共振时，Rust CTA 动能推土机会强行接管 AI 的均值回归摸顶信号，翻转为顺势的动能追击单，死死咬住单边大趋势。同时放行假突破洗盘，保留 AI 的抄底高胜率。
- **0.1秒极速追踪止盈 & L3 CVD 风控**：底层的 `RiskGuard V7.0` 模块每 100 毫秒扫描一次内存中的 WebSocket 订单流。结合 L3 `@aggTrade` 的真实买卖差 (CVD)，一旦发现主力砸盘，瞬间市价抢跑逃命。
- **-12% 零滑点市价断头台 (V6.2)**：彻底重构的物理级风控。一旦触发止损，系统瞬间执行撤单并以市价（Market Order）强平，刚性切断黑天鹅级别的暴跌亏损，完全消除了旧版限价单死等的风控盲区。
- **纯 15m 极速动能 (零网络 IO 延迟)**：升级为“刺客模式”，砍掉大周期 REST API 请求，Python 端专注于纯粹的 15m 微观订单流失衡 (OFI) 爆发，实现毫秒级“零等待”信号生成。

## ⚠️ 知识产权与防 Alpha 衰减声明

本仓库仅开源**“工程骨架与风控引擎”**。为了防范核心策略在实盘中因过度拥挤而失效（Alpha衰减），**核心的 XGBoost/HMM 模型权重以及分层资产池配置不予开源。** 

您需要自行训练 AI 模型，或通过 ZeroMQ 接入您自己的策略交易信号。
