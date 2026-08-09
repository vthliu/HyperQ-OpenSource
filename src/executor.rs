use std::sync::Arc;
use tracing::{info, warn, error};
use dashmap::DashMap;

use crate::models::MockPosition;
use crate::rest_api::RestApi;

#[derive(Debug, Clone)]
pub enum OrderType {
    MarketOpen { symbol: String, is_long: bool, qty: f64, expected_price: f64, tier: Option<String>, regime: Option<String>, atr_24h: f64, is_momentum_trade: bool },
    MarketClose { symbol: String, qty: f64, expected_price: f64, reason: String },
}

pub struct Executor {
    pub api: Arc<RestApi>,
    pub ws: Arc<crate::ws_api::WsApi>,
    positions: Arc<DashMap<String, MockPosition>>,
    funding_map: Arc<DashMap<String, (f64, u64)>>,
    price_map: Arc<DashMap<String, (f64, f64, u64)>>,
    cvd_map: Arc<DashMap<String, f64>>,
    ofi_map: Arc<DashMap<String, f64>>,
    config: crate::config::ExecutorConfig,
    dry_run: bool,
    leverage: u8,
}

impl Executor {
    pub fn new(
        api: Arc<RestApi>,
        ws: Arc<crate::ws_api::WsApi>,
        positions: Arc<DashMap<String, MockPosition>>,
        funding_map: Arc<DashMap<String, (f64, u64)>>,
        price_map: Arc<DashMap<String, (f64, f64, u64)>>,
        cvd_map: Arc<DashMap<String, f64>>,
        ofi_map: Arc<DashMap<String, f64>>,
        config: crate::config::ExecutorConfig,
        dry_run: bool,
        leverage: u8,
    ) -> Self {
        Self {
            api,
            ws,
            positions,
            funding_map,
            price_map,
            cvd_map,
            ofi_map,
            config,
            dry_run,
            leverage,
        }
    }

    pub async fn execute_order(&self, order_type: OrderType) {
        if self.dry_run {
            warn!("🧪 [DRY-RUN] Suppressing execution of order");
            return;
        }

        let sym = match &order_type {
            OrderType::MarketOpen { symbol, .. } => symbol.clone(),
            OrderType::MarketClose { symbol, .. } => symbol.clone(),
        };
        
        let mut spread_log = 0.0;
        if let Some(p) = self.price_map.get(&sym) {
            let bid = p.0;
            let ask = p.1;
            if bid > 0.0 { spread_log = (ask - bid) / bid; }
        }
        let cvd_log = self.cvd_map.get(&sym).map(|v| *v).unwrap_or(0.0);
        let ofi_log = self.ofi_map.get(&sym).map(|v| *v).unwrap_or(0.0);

        match order_type {
            OrderType::MarketOpen { symbol, is_long, qty, expected_price, tier, regime, atr_24h, is_momentum_trade } => {
                let mut is_duplicate = false;
                if let Some(pos) = self.positions.get(&symbol) {
                    let is_pos_long = pos.position_amt > 0.0;
                    if is_pos_long == is_long {
                        is_duplicate = true;
                    }
                }
                
                if is_duplicate {
                    warn!("🚫 [REJECT] 重复开仓信号: {} 已经存在同向持仓，拒绝执行", symbol);
                    return;
                }
                
                // Funding Rate Check
                if let Some(funding) = self.funding_map.get(&symbol) {
                    let (rate, next_time) = *funding;
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
                    let time_to_funding = (next_time.saturating_sub(now)) as f64 / 3600000.0; // hours
                    
                    // If funding rate is against us (> 0.05% for Long, < -0.05% for Short) and funding is within 2 hours
                    if time_to_funding < 2.0 {
                        if (is_long && rate > 0.0005) || (!is_long && rate < -0.0005) {
                            warn!("🚫 [REJECT] 资金费率过高: {} (做多: {}, 费率: {:.4}%, 剩余: {:.1}h)，拒绝开仓", 
                                symbol, is_long, rate * 100.0, time_to_funding);
                            return;
                        }
                    }
                }
                
                // Spread Filter (V5.3)
                if let Some(p) = self.price_map.get(&symbol) {
                    let bid = p.0;
                    let ask = p.1;
                    if bid > 0.0 {
                        let spread = (ask - bid) / bid;
                        if spread > 0.0015 { // 0.15% spread limit
                            warn!("🚫 [REJECT] 点差过高 (Spread Filter): {} (Bid: {}, Ask: {}, Spread: {:.2}%)", 
                                symbol, bid, ask, spread * 100.0);
                            return;
                        }
                    }
                }
                
                info!("Executing MARKET OPEN for {} (Long: {}): Qty={}, ExpectedPrice={}", symbol, is_long, qty, expected_price);
                
                // 1. Set Margin Type to ISOLATED first
                if let Err(e) = self.api.set_margin_type(&symbol, "ISOLATED").await {
                    warn!("Failed to set margin type for {}: {}", symbol, e);
                    // Continue anyway, as it might return an error if it's already ISOLATED
                }
                
                // 1.5 Set Leverage
                if let Err(e) = self.api.set_leverage(&symbol, self.leverage).await {
                    warn!("Failed to set leverage for {}: {}", symbol, e);
                }
                
                // 2. Place Order (Maker-then-Taker Logic)
                let side = if is_long { "BUY" } else { "SELL" };
                let is_layer1 = tier.as_deref() == Some("layer1");
                
                let final_fill_price;
                let mut final_qty = qty;
                
                let mut limit_filled_qty = 0.0;
                let mut limit_avg_price = 0.0;
                let mut remaining_qty = qty;
                
                if let Ok((bid, ask)) = self.api.get_book_ticker(&symbol).await {
                    let spread_pct = (ask - bid) / bid * 100.0;
                    let target_price = if is_long { bid } else { ask }; // Hang at best bid/ask
                    
                    if spread_pct <= 0.25 && target_price > 0.0 {
                        info!("🛡️ [MAKER PRIORITY] Placing LIMIT GTX for {}: Qty={}, Price={}", symbol, qty, target_price);
                        match self.ws.place_order(&symbol, side, "LIMIT", qty, Some(target_price), false).await {
                            Ok(resp) => {
                                if let Some(order_id) = resp.get("orderId").and_then(|v| v.as_u64()) {
                                    tokio::time::sleep(std::time::Duration::from_millis(3500)).await; // Wait 3.5 seconds
                                    
                                    // Attempt to cancel. If it fails, it might be already fully filled, which is fine.
                                    let _ = self.ws.cancel_order(&symbol, order_id).await;
                                    
                                    // Check status
                                    if let Ok(status_resp) = self.api.get_order(&symbol, order_id).await {
                                        limit_filled_qty = status_resp.get("executedQty").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                                        limit_avg_price = status_resp.get("avgPrice").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()).unwrap_or(target_price);
                                        remaining_qty -= limit_filled_qty;
                                        info!("⌛ [MAKER STATUS] {} Filled: {}, Remaining: {}", symbol, limit_filled_qty, remaining_qty);
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("⚠️ [MAKER FAIL] Failed LIMIT GTX for {}: {}. Falling back to MARKET.", symbol, e);
                            }
                        }
                    } else {
                        warn!("⚠️ [MAKER SKIP] {} spread {:.3}% too wide or invalid. Direct to MARKET.", symbol, spread_pct);
                    }
                }
                
                // Fallback / Completion with MARKET
                if remaining_qty > qty * 0.001 { // Account for floating point residuals
                    info!("🚀 [TAKER FALLBACK] Executing MARKET for {} Qty: {}", symbol, remaining_qty);
                    match self.ws.place_order(&symbol, side, "MARKET", remaining_qty, None, false).await {
                        Ok(resp) => {
                            let market_avg_price = resp.get("avgPrice")
                                .and_then(|v| v.as_str())
                                .and_then(|s| s.parse::<f64>().ok())
                                .unwrap_or(expected_price);
                                
                            if limit_filled_qty > 0.0 {
                                final_fill_price = (limit_filled_qty * limit_avg_price + remaining_qty * market_avg_price) / qty;
                            } else {
                                final_fill_price = market_avg_price;
                            }
                        }
                        Err(e) => {
                            error!("🔥 [FATAL] MARKET FALLBACK failed for {}: {}", symbol, e);
                            if limit_filled_qty == 0.0 {
                                return; // Entire order failed
                            }
                            // Partially filled by Maker, we just accept what we got
                            final_fill_price = limit_avg_price;
                            final_qty = limit_filled_qty;
                        }
                    }
                } else if limit_filled_qty > 0.0 {
                    final_fill_price = limit_avg_price;
                    final_qty = limit_filled_qty;
                } else {
                    return; // Should not reach here
                }

                let slippage_pct = if expected_price > 0.0 {
                    if is_long {
                        (final_fill_price - expected_price) / expected_price * 100.0
                    } else {
                        (expected_price - final_fill_price) / expected_price * 100.0
                    }
                } else { 0.0 };
                
                if slippage_pct > self.config.market_order_slippage_tolerance * 100.0 {
                    warn!("⚠️ [SLIPPAGE] Slippage {:.3}% exceeded tolerance {:.3}% for {}", slippage_pct, self.config.market_order_slippage_tolerance * 100.0, symbol);
                }

                info!("✅ ORDER SUCCESS: {} @ {} (Slippage: {:.3}%)", symbol, final_fill_price, slippage_pct);

                let trade_log = serde_json::json!({
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                    "symbol": symbol,
                    "side": side,
                    "type": if is_layer1 { "MIXED_OPEN" } else { "MARKET_OPEN" },
                    "qty": final_qty,
                    "expected_price": expected_price,
                    "fill_price": final_fill_price,
                    "slippage_pct": slippage_pct,
                    "spread_pct": spread_log * 100.0,
                    "cvd": cvd_log,
                    "ofi": ofi_log
                });
                
                if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open("/var/log/hyperq/trades.jsonl") {
                    let _ = std::io::Write::write_fmt(&mut file, format_args!("{}\n", trade_log));
                }
                
                let amt = if is_long { final_qty } else { -final_qty };
                self.positions.insert(symbol.clone(), crate::models::MockPosition::new(
                    symbol.clone(), final_fill_price, amt, self.leverage as u8, crate::risk_guard::current_time_ms(), tier, regime, atr_24h, is_momentum_trade
                ));
            }
            OrderType::MarketClose { symbol, qty, expected_price, reason } => {
                // Remove duplicate log. Sentinel already logged this action.
                
                // Retrieve the actual position to get side. For simplicity we assume if it's in pos map it has amt
                // A positive qty means we are LONG, so we must SELL to close.
                // If it's short (negative qty), we must BUY to close.
                let side = if qty > 0.0 { "SELL" } else { "BUY" };
                let abs_qty = qty.abs();
                let mut mfe = 0.0;
                let mut mae = 0.0;
                let mut roe = 0.0;
                let mut entry_time = 0;
                let mut entry_price = 0.0;
                
                if let Some(pos) = self.positions.get(&symbol) {
                    mfe = pos.max_favorable_excursion;
                    mae = pos.max_adverse_excursion;
                    roe = pos.unrealized_roe;
                    entry_time = pos.entry_time;
                    entry_price = pos.entry_price;
                }
                let is_hard_stop = reason.contains("Hard Stop Loss");
                let mut limit_filled_qty = 0.0;
                let mut limit_avg_price = 0.0;
                let mut remaining_qty = abs_qty;
                
                // 强制清理所有挂单，防止占用 ReduceOnly 额度导致平仓失败
                info!("🧹 [CLOSE PREP] Canceling all pending orders for {} to release ReduceOnly quota", symbol);
                let _ = self.api.cancel_all_orders(&symbol).await;
                
                // Try MAKER LIMIT order first for closing, ONLY if it's not a Hard Stop
                if !is_hard_stop {
                if let Ok((bid, ask)) = self.api.get_book_ticker(&symbol).await {
                    let spread_pct = (ask - bid) / bid * 100.0;
                    // To close a LONG, we SELL, so we place limit at ASK. To close a SHORT, we BUY, so we place at BID.
                    let target_price = if side == "SELL" { ask } else { bid }; 
                    
                    if spread_pct <= 0.30 && target_price > 0.0 {
                        info!("🛡️ [MAKER CLOSE] Placing LIMIT for {}: Qty={}, Price={}", symbol, abs_qty, target_price);
                        match self.ws.place_order(&symbol, side, "LIMIT", abs_qty, Some(target_price), true).await {
                            Ok(resp) => {
                                if let Some(order_id) = resp.get("orderId").and_then(|v| v.as_u64()) {
                                    tokio::time::sleep(std::time::Duration::from_millis(3500)).await; // Wait 3.5 seconds
                                    
                                    // Cancel remaining
                                    let _ = self.ws.cancel_order(&symbol, order_id).await;
                                    
                                    if let Ok(status_resp) = self.api.get_order(&symbol, order_id).await {
                                        limit_filled_qty = status_resp.get("executedQty").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                                        limit_avg_price = status_resp.get("avgPrice").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()).unwrap_or(target_price);
                                        remaining_qty -= limit_filled_qty;
                                        info!("⌛ [MAKER CLOSE STATUS] {} Filled: {}, Remaining: {}", symbol, limit_filled_qty, remaining_qty);
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("⚠️ [MAKER CLOSE FAIL] Failed LIMIT for {}: {}", symbol, e);
                            }
                        }
                    }
                }
                }
                
                // Fallback / Completion with MARKET (must use ReduceOnly to avoid reversing position if local state is stale)
                if remaining_qty > abs_qty * 0.001 {
                    info!("🚀 [TAKER CLOSE FALLBACK] Executing MARKET close for {} Qty: {}", symbol, remaining_qty);
                    match self.ws.place_order(&symbol, side, "MARKET", remaining_qty, None, true).await {
                        Ok(resp) => {
                            let market_avg_price = resp.get("avgPrice")
                                .and_then(|v| v.as_str())
                                .and_then(|s| s.parse::<f64>().ok())
                                .unwrap_or(expected_price);

                            let mut final_fill_price = market_avg_price;
                            if limit_filled_qty > 0.0 {
                                final_fill_price = (limit_filled_qty * limit_avg_price + remaining_qty * market_avg_price) / abs_qty;
                            }

                            let slippage_pct = if final_fill_price > 0.0 && expected_price > 0.0 {
                                (final_fill_price - expected_price).abs() / expected_price
                            } else { 0.0 };

                            info!("✅ [CLOSE SUCCESS] {} filled at {}, slippage: {:.4}%", symbol, final_fill_price, slippage_pct * 100.0);

                            let trade_log = serde_json::json!({
                                "timestamp": chrono::Utc::now().to_rfc3339(),
                                "symbol": symbol,
                                "side": side,
                                "type": "MARKET_CLOSE",
                                "qty": abs_qty,
                                "expected_price": expected_price,
                                "fill_price": final_fill_price,
                                "slippage_pct": slippage_pct,
                                "spread_pct": spread_log * 100.0,
                                "cvd": cvd_log,
                                "ofi": ofi_log,
                                "unrealized_roe": roe,
                                "max_favorable_excursion": mfe,
                                "max_adverse_excursion": mae,
                                "entry_time": entry_time,
                                "entry_price": entry_price,
                                "exit_reason": reason
                            });

                            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open("/var/log/hyperq/trades.jsonl") {
                                let _ = std::io::Write::write_fmt(&mut file, format_args!("{}\n", trade_log));
                            }

                            self.positions.remove(&symbol);
                        }
                        Err(e) => {
                            error!("🔥 [FATAL] MARKET CLOSE failed for {}: {}", symbol, e);
                            if let Some(pos) = self.positions.get(&symbol) {
                                pos.unlock_close();
                            }
                        }
                    }
                } else {
                    // Fully filled by Maker
                    let slippage_pct = if limit_avg_price > 0.0 && expected_price > 0.0 {
                        (limit_avg_price - expected_price).abs() / expected_price
                    } else { 0.0 };
                    
                    info!("✅ [MAKER CLOSE SUCCESS] {} fully filled at {}, slippage: {:.4}%", symbol, limit_avg_price, slippage_pct * 100.0);
                    
                    let trade_log = serde_json::json!({
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                        "symbol": symbol,
                        "side": side,
                        "type": "LIMIT_CLOSE",
                        "qty": limit_filled_qty,
                        "expected_price": expected_price,
                        "fill_price": limit_avg_price,
                        "slippage_pct": slippage_pct,
                        "spread_pct": spread_log * 100.0,
                        "cvd": cvd_log,
                        "ofi": ofi_log,
                        "unrealized_roe": roe,
                        "max_favorable_excursion": mfe,
                        "max_adverse_excursion": mae,
                        "entry_time": entry_time,
                        "entry_price": entry_price,
                        "exit_reason": reason
                    });
                    
                    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open("/var/log/hyperq/trades.jsonl") {
                        let _ = std::io::Write::write_fmt(&mut file, format_args!("{}\n", trade_log));
                    }
                    self.positions.remove(&symbol);
                }
            }
        }
    }
}
