use std::sync::Arc;
use std::sync::atomic::Ordering;
use dashmap::DashMap;
use tracing::{info, warn, error};


use crate::models::MockPosition;
use crate::executor::{Executor, OrderType};

pub struct Sentinel {
    positions: Arc<DashMap<String, MockPosition>>,
    executor: Arc<Executor>,
    funding_map: Arc<DashMap<String, (f64, u64)>>,
    config: crate::config::SentinelConfig,
    dry_run: bool,
    time_stop: crate::config::TimeStopConfig,
    penalty_map: Arc<DashMap<String, (f64, u64)>>,
}

impl Sentinel {
    pub fn new(positions: Arc<DashMap<String, MockPosition>>, executor: Arc<Executor>, funding_map: Arc<DashMap<String, (f64, u64)>>, config: crate::config::SentinelConfig, dry_run: bool, time_stop: crate::config::TimeStopConfig, penalty_map: Arc<DashMap<String, (f64, u64)>>) -> Self {
        Self { positions, executor, funding_map, config, dry_run, time_stop, penalty_map }
    }

    pub fn start(self: Arc<Self>) {
        info!("Starting Sentinel micro-thread ({}ms interval)...", self.config.check_interval_ms);
        
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async move {
                let mut last_heartbeat = std::time::Instant::now();
                loop {
                    self.check_positions().await;
                    
                    if last_heartbeat.elapsed() > std::time::Duration::from_secs(10) {
                        info!("💓 [SENTINEL] 心跳: 正在监控 {} 个仓位", self.positions.len());
                        last_heartbeat = std::time::Instant::now();
                    }
                    
                    tokio::time::sleep(std::time::Duration::from_millis(self.config.check_interval_ms)).await;
                }
            });
        });
    }

    async fn check_positions(&self) {
        let mut orders_to_execute = Vec::new();
        
        for entry in self.positions.iter() {
            let symbol = entry.key();
            let pos = entry.value();
            let current_time = crate::risk_guard::current_time_ms();

            if pos.is_closing.load(Ordering::SeqCst) {
                // Bug 5 fix: correctly read AtomicU64 close timestamp
                let start_ts = pos.closing_started_at.load(Ordering::SeqCst);
                if start_ts > 0 && crate::risk_guard::current_time_ms() - start_ts > self.config.closing_timeout_ms {
                    warn!("⚠️ 平仓超时 for {}, 强制解锁", symbol);
                    pos.unlock_close();
                }
                continue;
            }

            // 7.2 Hard Stop Conditions
            let mut trigger_close = false;
            let mut reason = String::new();

            // Calculate Dynamic ATR Stop Loss
            let atr_roe_drop = if pos.atr_24h > 0.0 && pos.entry_price > 0.0 {
                (pos.atr_24h / pos.entry_price) * 2.0 * (pos.leverage as f64) * 100.0 // 2x ATR for medium term
            } else {
                25.0 // Fallback for medium term strategy
            };
            
            let mut current_hard_stop = -atr_roe_drop.clamp(10.0, 20.0); // 中长线极速止损 (最大允许亏损 20% ROE)

            if let Some(reg) = &pos.regime {
                if current_time % 60_000 < 500 { // Roughly every minute
                    info!("🛡️ [REGIME] {} 仓位当前大盘状态: {}", pos.symbol, reg);
                }
                
                if reg == "CHOP_HIGH_VOL" {
                    current_hard_stop *= 0.8; // Tighten stop in choppy regime
                }
            }
            
            if pos.unrealized_roe < current_hard_stop {
                trigger_close = true;
                reason = format!("ATR Stop Loss ({:.2}%) [Defense]", current_hard_stop);
            } else {
                let tier_str = pos.tier.as_deref().unwrap_or("layer3");
                let max_holding_hours = match tier_str {
                    "layer1" => self.time_stop.layer1_max_holding_hours,
                    "layer2" => self.time_stop.layer2_max_holding_hours,
                    _ => self.time_stop.layer3_max_holding_hours,
                };
                let max_holding_ms = (max_holding_hours * 3600_000.0) as u64;
                
                if current_time > pos.entry_time && current_time - pos.entry_time > max_holding_ms {
                    let dynamic_profit_threshold = 3.0;
                    if pos.unrealized_roe > dynamic_profit_threshold {
                        if pos.time_barrier_notified.compare_exchange(false, true, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst).is_ok() {
                            warn!("🟢 [SENTINEL] Time Barrier Exceeded for {}, but ROE {:.2}% > {:.2}%. Letting profit run.", 
                                symbol, pos.unrealized_roe, dynamic_profit_threshold);
                        }
                    } else {
                        trigger_close = true;
                        reason = format!("Time Barrier Exceeded (>{:.2}H, ROE: {:.2}%)", 
                            max_holding_hours, pos.unrealized_roe);
                    }
                }
            }
            
            // Funding Rate Defense
            if !trigger_close {
                if let Some(funding) = self.funding_map.get(symbol) {
                    let (rate, next_time) = *funding;
                    let now = crate::risk_guard::current_time_ms();
                    let time_to_funding = (next_time.saturating_sub(now)) as f64 / 60000.0; // minutes
                    
                    if time_to_funding < 10.0 { // Less than 10 minutes to funding
                        let is_long = pos.position_amt > 0.0;
                        if (is_long && rate > 0.0005) || (!is_long && rate < -0.0005) {
                            trigger_close = true;
                            reason = format!("Funding Defense (Rate: {:.4}%, Time: {:.1}m)", rate * 100.0, time_to_funding);
                            warn!("🚨 [FUNDING DEFENSE] {} 即将结算巨额资金费，强制逃顶平仓!", symbol);
                        }
                    }
                }
            }

            if trigger_close {
                if pos.try_lock_for_close() {
                    if self.dry_run {
                        warn!("🧪 [DRY-RUN] 本应触发平仓: {} at ROE={:.2}% - Reason: {}", symbol, pos.unrealized_roe, reason);
                        pos.unlock_close(); // Unlock so it doesn't get stuck in closing state during dry run
                    } else {
                        error!("🔥 [SENTINEL] EXECUTING MARKET SELL for {} - Reason: {}", symbol, reason);
                        
                        let side_mult = if pos.position_amt > 0.0 { 1.0 } else { -1.0 };
                        let expected_price = pos.entry_price * (1.0 + pos.unrealized_roe / 100.0 / (pos.leverage as f64) * side_mult);
                        orders_to_execute.push(OrderType::MarketClose { symbol: symbol.clone(), qty: pos.position_amt, expected_price, reason: reason.clone() });
                    }
                }
            }
        }
        
        for order in orders_to_execute {
            self.executor.execute_order(order).await;
        }
    }
}
