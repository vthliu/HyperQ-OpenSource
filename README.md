# HyperQ: Industrial-Grade Quantitative Trading Engine

[中文说明 (Chinese Version)](#中文说明)

HyperQ is a high-frequency, institutional-grade execution engine for cryptocurrency perpetual futures, featuring a **Rust + Python** dual-engine architecture. It is designed to capture high-certainty momentum breakouts while strictly managing risk with HFT-level (High-Frequency Trading) precision.

## 🚀 Key Features

- **0.1s Real-time Trailing Stop**: The `RiskGuard V2` module in Rust iterates through memory-mapped WebSocket data every 100 milliseconds, ensuring profits are locked in instantly without API latency.
- **Strict -20% ROE Hard Stop (Sentinel)**: The `Sentinel` thread acts as a circuit breaker, clamping maximum losses dynamically. It prevents cascading liquidations by brutally cutting losses exactly at the predefined mathematical boundary.
- **Multi-Timeframe (MTF) Momentum**: The Python AI inference server analyzes 4H, 1H, and 15m order flow (OFI) concurrently.
- **Nuclear Momentum Override**: In cases of extreme liquidations or massive whale activity, the system ignores macro trends and overrides standard rules to capture V-shaped reversals.

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

- **0.1秒极速追踪止盈**：底层的 `RiskGuard V2` 模块每 100 毫秒扫描一次内存中的 WebSocket 订单流。一旦触发止盈，瞬间市价斩仓，彻底杜绝“利润回吐”。
- **-20% ROE 铁血防线 (Sentinel)**：哨兵微线程作为物理级熔断器，无论行情如何插针，强制把单笔最大亏损死死锁在数学极限内。
- **多周期动能共振 (MTF)**：Python 端并发处理 4H、1H 宏观大势与 15m 微观订单流失衡 (OFI)。
- **动能穿透特权 (Nuclear Override)**：在全网爆仓踩踏或巨鲸暴力扫盘时，系统会无视宏观趋势，发动特权级买入，捕捉深V反转。

## ⚠️ 知识产权与防 Alpha 衰减声明

本仓库仅开源**“工程骨架与风控引擎”**。为了防范核心策略在实盘中因过度拥挤而失效（Alpha衰减），**核心的 XGBoost/HMM 模型权重以及分层资产池配置不予开源。** 

您需要自行训练 AI 模型，或通过 ZeroMQ 接入您自己的策略交易信号。
