#![allow(dead_code)]

use std::sync::Arc;
use tokio::sync::mpsc::Receiver;
use tracing::{info, warn};
use dashmap::DashMap;

use crate::models_signal::Signal;
use crate::models::MockPosition;
use crate::executor::{Executor, OrderType};

pub struct TrendProcessor {
    pub rx: Receiver<Signal>,
    positions: Arc<DashMap<String, MockPosition>>,
    executor: Arc<Executor>,
    max_positions: usize,
    rwa_risk_multiplier: f64,
    leverage: f64,
    dry_run: bool,
    prob_threshold: crate::config::ProbThresholdConfig,
    defense_config: crate::config::DefenseConfig,
    dynamic_sizing: crate::config::DynamicSizingConfig,
    penalty_map: Arc<DashMap<String, (f64, u64)>>,
}

impl TrendProcessor {
    pub fn new(
        rx: Receiver<Signal>,
        positions: Arc<DashMap<String, MockPosition>>,
        executor: Arc<Executor>,
        max_positions: usize,
        rwa_risk_multiplier: f64,
        leverage: f64,
        dry_run: bool,
        prob_threshold: crate::config::ProbThresholdConfig,
        defense_config: crate::config::DefenseConfig,
        dynamic_sizing: crate::config::DynamicSizingConfig,
        penalty_map: Arc<DashMap<String, (f64, u64)>>,
    ) -> Self {
        Self {
            rx,
            positions,
            executor,
            max_positions,
            rwa_risk_multiplier,
            leverage,
            dry_run,
            prob_threshold,
            defense_config,
            dynamic_sizing,
            penalty_map,
        }
    }

    pub async fn run(&mut self) {
        info!("Starting TrendProcessor...");
        
        while let Some(signal) = self.rx.recv().await {
            self.process_signal(signal).await;
        }
    }

    pub async fn process_signal(&self, signal: Signal) {
        let mut target_threshold = match signal.tier.as_deref() {
            Some("layer1") => self.prob_threshold.layer1,
            Some("layer2") => self.prob_threshold.layer2,
            Some("layer3") => self.prob_threshold.layer3,
            _ => self.prob_threshold.layer3,
        };

        // --- Smart Reversal Exit Logic ---
        let mut reversal_order = None;
        if let Some(pos) = self.positions.get(&signal.symbol) {
            let is_pos_long = pos.position_amt > 0.0;
            if is_pos_long != signal.is_long {
                if signal.prob >= target_threshold {
                    if pos.try_lock_for_close() {
                        warn!("🔄 [REVERSAL DETECTED] AI signal opposing position {} (Prob: {:.2}%). Triggering Market Close!", signal.symbol, signal.prob * 100.0);
                        let side_mult = if is_pos_long { 1.0 } else { -1.0 };
                        let expected_price = pos.entry_price * (1.0 + pos.unrealized_roe / 100.0 / (pos.leverage as f64) * side_mult);
                        reversal_order = Some(OrderType::MarketClose {
                            symbol: signal.symbol.clone(),
                            qty: pos.position_amt,
                            expected_price,
                            reason: format!("AI Reversal (Prob: {:.1}%)", signal.prob * 100.0),
                        });
                    }
                } else {
                    info!("AI opposing signal for {} (Prob: {:.2}%) below threshold, ignoring.", signal.symbol, signal.prob * 100.0);
                }
            } else {
                info!("Already holding {} in the same direction. Ignoring.", signal.symbol);
            }
        }

        if let Some(order) = reversal_order {
            self.executor.execute_order(order).await;
            return; // Don't open a new position immediately
        }

        // If we are already holding, we should not open another one
        if self.positions.contains_key(&signal.symbol) {
            return;
        }

        let current_positions = self.positions.len();
        if current_positions >= self.max_positions {
            // Note: Should filter by is_long / is_short specifically as per spec, 
            // but simplified here to total positions for skeleton
            warn!("Max positions reached ({}). Ignoring signal for {}", current_positions, signal.symbol);
            return;
        }

        if signal.prob < target_threshold {
            warn!("Signal prob {} for {} is below its tier threshold {}. Ignoring.", signal.prob, signal.symbol, target_threshold);
            return;
        }

        info!("Received strong signal for {} (Prob: {} >= {}). Executing...", signal.symbol, signal.prob, target_threshold);
        
        // Defense Mode Leverage Check
        let mut target_leverage = self.leverage;
        if let Some(reg) = &signal.regime {
            if reg == "CHOP_HIGH_VOL" && self.defense_config.enabled {
                target_leverage = (self.leverage * self.defense_config.leverage_multiplier).max(1.0);
                warn!("🛡️ [DEFENSE MODE] 大盘高波洗盘, 目标杠杆由 {}x 降至 {}x", self.leverage, target_leverage);
            }
        }
        
        // 1. Determine Tier and Sizing (Dynamic based on Probability)
        let mut alloc_pct = 0.20; // Default fallback
        if self.dynamic_sizing.enabled {
            let progress = (signal.prob - target_threshold) / (1.0 - target_threshold);
            let progress = progress.clamp(0.0, 1.0);
            alloc_pct = self.dynamic_sizing.min_alloc_pct + progress * (self.dynamic_sizing.max_alloc_pct - self.dynamic_sizing.min_alloc_pct);
        }
        
        let target_notional = match self.executor.api.get_usdt_balance().await {
            Ok(bal) => {
                let alloc = bal * alloc_pct * target_leverage;
                info!("💰 USDT Balance: {}, Allocating {:.1}% as Margin * {}x Leverage: {}", bal, alloc_pct * 100.0, target_leverage, alloc);
                if alloc < 6.0 { 6.0 } else { alloc } // Binance minimum is ~$5, padding to $6 to be safe
            },
            Err(e) => {
                warn!("⚠️ Failed to fetch balance, using default $50 Notional: {}", e);
                50.0
            }
        };
        let mut final_qty = target_notional / signal.price;
        
        // Quick heuristics for Binance step sizes
        if signal.price > 1000.0 {
            final_qty = (final_qty * 1000.0).round() / 1000.0;
        } else if signal.price > 100.0 {
            final_qty = (final_qty * 100.0).round() / 100.0;
        } else if signal.price > 10.0 {
            final_qty = (final_qty * 10.0).round() / 10.0;
        } else {
            final_qty = final_qty.round();
        }
        
        if final_qty <= 0.0 { final_qty = 1.0; }
        if let Some(tier) = &signal.tier {
            if tier == "layer3" {
                final_qty *= self.rwa_risk_multiplier;
                // Bug 4 fix: ensure qty is still >= 1.0 (Binance minimum) after multiplier
                final_qty = final_qty.max(1.0);
                info!("Layer 3 Asset detected! Applying RWA risk multiplier. Qty: {}", final_qty);
            }
        }
        
        // Execute the top signal
        let order = OrderType::MarketOpen {
            symbol: signal.symbol.clone(),
            is_long: signal.is_long,
            qty: final_qty, // Adjusted for RWA
            expected_price: signal.price,
            tier: signal.tier.clone(),
            regime: signal.regime.clone(),
            atr_24h: signal.atr_24h,
        };
        
        if self.dry_run {
            warn!("🧪 [DRY-RUN] 模拟开仓: {}, 概率 {}%, Qty: {}", signal.symbol, signal.prob * 100.0, final_qty);
            return;
        }

        self.executor.execute_order(order).await;
    }
}
