#![allow(dead_code)]

use std::sync::Arc;
use dashmap::DashMap;
use tracing::{info, warn};

use crate::models::MockPosition;
use crate::executor::{Executor, OrderType};

pub struct RiskGuard {
    positions: Arc<DashMap<String, MockPosition>>,
    executor: Arc<Executor>,
    price_map: Arc<DashMap<String, (f64, u64)>>,
    config: crate::config::RiskGuardConfig,
    asset_tiers: crate::config::AssetTiersConfig,
}

impl RiskGuard {
    pub fn new(positions: Arc<DashMap<String, MockPosition>>, executor: Arc<Executor>, price_map: Arc<DashMap<String, (f64, u64)>>, config: crate::config::RiskGuardConfig, asset_tiers: crate::config::AssetTiersConfig) -> Self {
        Self { positions, executor, price_map, config, asset_tiers }
    }

    /// Bug 3 Fix: Fast cycle (every 5s) — ONLY updates price/ROE/MFE/MAE so Sentinel
    /// always has fresh data, even before the first 60-second macro cycle runs.
    pub async fn update_roe_cycle(&self) {
        let mut orders_to_execute = Vec::new();

        for mut entry in self.positions.iter_mut() {
            let symbol = entry.key().clone();
            let pos = entry.value_mut();

            let price = if let Some(p) = self.price_map.get(&symbol) {
                p.0
            } else {
                continue; // No price yet — skip rather than using stale entry_price
            };

            let side_mult = if pos.position_amt > 0.0 { 1.0 } else { -1.0 };
            let roe = (price - pos.entry_price) / pos.entry_price * 100.0 * (pos.leverage as f64) * side_mult;
            let new_ema_roe = roe * 0.25 + pos.ema_roe * 0.75;

            pos.unrealized_roe = roe;
            pos.ema_roe = new_ema_roe;

            if roe > pos.max_favorable_excursion { pos.max_favorable_excursion = roe; }
            if roe < pos.max_adverse_excursion   { pos.max_adverse_excursion   = roe; }
            if new_ema_roe > pos.peak_ema_roe    { pos.peak_ema_roe = new_ema_roe; }

            // Phase 1: 用 MFE（真实最高浮盈）来激活追踪止盈
            // 中长线策略：MFE 必须达到 15% ROE 才激活，避免正常震荡被早早止出
            if pos.max_favorable_excursion > 15.0 && !pos.alert_flag {
                pos.alert_flag = true;
                warn!("🟡 [RISK GUARD] 追踪止盈激活 (Trailing Stop) for {} — MFE={:.2}%", symbol, pos.max_favorable_excursion);
            }
            
            // Phase 2: 极速追踪止盈：一旦触碰回撤线，5秒内立刻斩仓
            if pos.alert_flag {
                let peak = pos.max_favorable_excursion;
                let current = pos.unrealized_roe;
                
                let mut trigger_stop = false;
                
                let stop_level = if peak < 20.0 {
                    peak * 0.5 // 保本区：回撤超过峰值的50%触发
                } else {
                    peak * 0.65 // 主升浪区：保留峰值的65%利润
                };

                if current < stop_level {
                    trigger_stop = true;
                }

                if trigger_stop {
                    warn!("🔴 [RISK GUARD] Trailing Stop triggered for {}: MFE={:.2}% → Current ROE={:.2}% → StopLevel={:.2}%",
                        symbol, peak, current, stop_level);
                    if pos.try_lock_for_close() {
                        orders_to_execute.push(OrderType::MarketClose {
                            symbol: symbol.clone(),
                            qty: pos.position_amt,
                            expected_price: price,
                            reason: format!("Trailing Stop (MFE={:.2}%, Now={:.2}%)", peak, current)
                        });
                    }
                }
            }
        }
        
        for order in orders_to_execute {
            self.executor.execute_order(order).await;
        }
    }

    /// Slow cycle (every 60s) — PnL logging + trailing stop logic.
    pub async fn run_macro_cycle(&self) {
        info!("Running RiskGuard V2 Macro Cycle...");
        
        let mut orders_to_execute = Vec::new();
        
        for mut entry in self.positions.iter_mut() {
            let symbol = entry.key().clone();
            let pos = entry.value_mut();

            let price = if let Some(p) = self.price_map.get(&symbol) {
                p.0
            } else {
                pos.entry_price
            };
            
            // Info log so user can see it's tracking profitability
            info!("📊 [PnL Tracking] {}: Price={:.4}, ROE={:.2}%, EMA={:.2}%, Peak={:.2}%, MFE={:.2}%, MAE={:.2}%",
                symbol, price, pos.unrealized_roe, pos.ema_roe, pos.peak_ema_roe,
                pos.max_favorable_excursion, pos.max_adverse_excursion);
            
            // Output to PnL JSONL
            let pnl_log = serde_json::json!({
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "symbol": symbol,
                "unrealized_roe": pos.unrealized_roe,
                "ema_roe": pos.ema_roe,
                "peak_ema_roe": pos.peak_ema_roe,
                "entry_price": pos.entry_price,
                "mark_price": price
            });
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open("/var/log/hyperq/pnl.jsonl") {
                let _ = std::io::Write::write_fmt(&mut file, format_args!("{}\n", pnl_log));
            }
            
            // 这里的 Phase 1 和 Phase 2 代码已经被抽离到 update_roe_cycle 中进行 5 秒极速巡检
            // 宏观周期仅保留日志记录和 PnL 文件写入，不再负责执行止盈单
        }
        
        for order in orders_to_execute {
            self.executor.execute_order(order).await;
        }
    }
}

pub fn current_time_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
}
