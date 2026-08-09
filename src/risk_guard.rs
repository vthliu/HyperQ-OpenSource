#![allow(dead_code)]

use std::sync::Arc;
use dashmap::DashMap;
use tracing::{info, warn};

use crate::models::MockPosition;
use crate::executor::{Executor, OrderType};

pub struct RiskGuard {
    positions: Arc<DashMap<String, MockPosition>>,
    executor: Arc<Executor>,
    price_map: Arc<DashMap<String, (f64, f64, u64)>>,
    config: crate::config::RiskGuardConfig,
    asset_tiers: crate::config::AssetTiersConfig,
    cvd_map: Arc<DashMap<String, f64>>,
}

impl RiskGuard {
    pub fn new(positions: Arc<DashMap<String, MockPosition>>, executor: Arc<Executor>, price_map: Arc<DashMap<String, (f64, f64, u64)>>, config: crate::config::RiskGuardConfig, asset_tiers: crate::config::AssetTiersConfig, cvd_map: Arc<DashMap<String, f64>>) -> Self {
        Self { positions, executor, price_map, config, asset_tiers, cvd_map }
    }

    /// Bug 3 Fix: Fast cycle (every 5s) — ONLY updates price/ROE/MFE/MAE so Sentinel
    /// always has fresh data, even before the first 60-second macro cycle runs.
    pub async fn update_roe_cycle(&self) {
        let mut orders_to_execute = Vec::new();

        for mut entry in self.positions.iter_mut() {
            let symbol = entry.key().clone();
            let pos = entry.value_mut();

            let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
            
            let price = if let Some(p) = self.price_map.get(&symbol) {
                if now_ms > p.2 && (now_ms - p.2) > 5000 {
                    // WS stale > 5 seconds, fallback to REST
                    if let Ok((bid, ask)) = self.executor.api.get_book_ticker(&symbol).await {
                        let pr = (bid + ask) / 2.0;
                        tracing::warn!("📡 [RADAR] WS 行情延迟 > 5秒 ({}ms)! 强制切换 REST API 抓取 {} 最新价格: {}", now_ms - p.2, symbol, pr);
                        pr
                    } else {
                        (p.0 + p.1) / 2.0
                    }
                } else {
                    (p.0 + p.1) / 2.0
                }
            } else {
                continue; // No price yet
            };

            let side_mult = if pos.position_amt > 0.0 { 1.0 } else { -1.0 };
            let roe = (price - pos.entry_price) / pos.entry_price * 100.0 * (pos.leverage as f64) * side_mult;
            let new_ema_roe = roe * 0.25 + pos.ema_roe * 0.75;

            pos.unrealized_roe = roe;
            pos.ema_roe = new_ema_roe;

            if roe > pos.max_favorable_excursion { pos.max_favorable_excursion = roe; }
            if roe < pos.max_adverse_excursion   { pos.max_adverse_excursion   = roe; }
            if new_ema_roe > pos.peak_ema_roe    { pos.peak_ema_roe = new_ema_roe; }

            let cvd = self.cvd_map.get(&symbol).map(|v| *v).unwrap_or(0.0);
            
            // Phase -1: Hard Stop Loss
            if roe < -12.0 {
                warn!("💀 [HARD STOP] 无条件硬止损触发 for {}! ROE={:.2}%", symbol, roe);
                if pos.try_lock_for_close() {
                    orders_to_execute.push(OrderType::MarketClose {
                        symbol: symbol.clone(),
                        qty: pos.position_amt,
                        expected_price: price,
                        reason: format!("Hard Stop Loss (ROE={:.2}%)", roe)
                    });
                }
                continue;
            }

            // Phase 0: CVD (L3 AggTrade) 微观预判止损 (Assassin Mode)
            // 如果浮亏大于 -5%，且微观吃单动量 (CVD) 严重背离，立刻斩仓，不等 -20%
            if roe < -5.0 {
                let is_long = pos.position_amt > 0.0;
                let c_thresh = 150000.0; // 假设 CVD 阈值，需根据实际调参
                let cvd_fatal = if is_long { cvd < -c_thresh } else { cvd > c_thresh };
                
                if cvd_fatal {
                    warn!("🔥 [ASSASSIN] CVD L3 崩塌预判止损 for {}! ROE={:.2}%, CVD={:.0}", symbol, roe, cvd);
                    if pos.try_lock_for_close() {
                        orders_to_execute.push(OrderType::MarketClose {
                            symbol: symbol.clone(),
                            qty: pos.position_amt,
                            expected_price: price,
                            reason: "Assassin L3 Cut".to_string()
                        });
                    }
                    continue; // Skip other phase checks
                }
            }

            // Phase M: Momentum Trade Wide Trailing Stop
            if pos.is_momentum_trade {
                let peak = pos.max_favorable_excursion;
                // Give it extreme breathing room: -15% absolute dropdown from peak
                // The hard stop at -12% earlier will catch the initial failure.
                if peak > 10.0 {
                    let drawdown = peak - roe;
                    if drawdown > 15.0 {
                        warn!("🌊 [MOMENTUM EXIT] 主升浪结束！趋势单平仓 for {}! MFE={:.2}%, Drawdown={:.2}%", symbol, peak, drawdown);
                        if pos.try_lock_for_close() {
                            orders_to_execute.push(OrderType::MarketClose {
                                symbol: symbol.clone(),
                                qty: pos.position_amt,
                                expected_price: price,
                                reason: format!("Momentum Trail Cut (Drawdown={:.1}%)", drawdown)
                            });
                        }
                    }
                }
                continue; // Skip the rest of Mean-Reversion RiskGuard logic
            }

            // Phase 0.5: 保本平仓 (Breakeven Stop)
            // 如果历史最高浮盈突破过 8%，则绝对不允许这单亏钱，回撤到 +1.5% 直接保本出局
            if pos.max_favorable_excursion > 8.0 && roe < 1.5 && !pos.alert_flag {
                warn!("🛡️ [BREAKEVEN] 保本平仓触发 for {}! MFE={:.2}%, Current ROE={:.2}%", symbol, pos.max_favorable_excursion, roe);
                if pos.try_lock_for_close() {
                    orders_to_execute.push(OrderType::MarketClose {
                        symbol: symbol.clone(),
                        qty: pos.position_amt,
                        expected_price: price,
                        reason: "Breakeven Stop (MFE>8%, Now<1.5%)".to_string()
                    });
                }
                continue;
            }

            // Phase 1: 用 MFE（真实最高浮盈）来激活追踪止盈
            // 中长线策略：MFE 必须达到 10% ROE 才激活，避免利润被反噬
            if pos.max_favorable_excursion > 10.0 && !pos.alert_flag {
                pos.alert_flag = true;
                warn!("🟡 [RISK GUARD] 追踪止盈激活 (Trailing Stop) for {} — MFE={:.2}%", symbol, pos.max_favorable_excursion);
            }
            
            // Phase 2: 极速追踪止盈：一旦触碰回撤线，5秒内立刻斩仓
            if pos.alert_flag {
                let peak = pos.max_favorable_excursion;
                let current = pos.unrealized_roe;
                
                let mut trigger_stop = false;
                
                let stop_level = if peak < 20.0 {
                    peak * 0.60 // 阶段 A (10-20% ROE): 允许 40% 回撤，保留 60% 利润
                } else if peak < 40.0 {
                    peak * 0.75 // 阶段 B (20-40% ROE): 允许 25% 回撤，保留 75% 利润
                } else {
                    peak * 0.85 // 阶段 C (>40% ROE): 允许 15% 回撤，保留 85% 利润
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
        
        let orders_to_execute = Vec::new();
        
        for mut entry in self.positions.iter_mut() {
            let symbol = entry.key().clone();
            let pos = entry.value_mut();

            let price = if let Some(p) = self.price_map.get(&symbol) {
                (p.0 + p.1) / 2.0
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
