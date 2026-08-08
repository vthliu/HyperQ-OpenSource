use std::sync::Arc;
use dashmap::DashMap;
use tracing::{info, warn};

use crate::models::MockPosition;
use crate::rest_api::RestApi;

pub struct Reconciler {
    positions: Arc<DashMap<String, MockPosition>>,
    api: Arc<RestApi>,
}

impl Reconciler {
    pub fn new(positions: Arc<DashMap<String, MockPosition>>, api: Arc<RestApi>) -> Self {
        Self { positions, api }
    }

    pub async fn run(&self) {
        info!("Starting Reconciler task (10 min interval)...");
        loop {
            self.reconcile().await;
            tokio::time::sleep(std::time::Duration::from_secs(600)).await;
        }
    }

    async fn reconcile(&self) {
        info!("Running 10-minute position reconciliation...");
        if let Ok(positions_val) = self.api.get_position_risk().await {
            let mut remote_symbols = std::collections::HashSet::new();
            
            for pos_val in positions_val {
                if let (Some(sym), Some(amt_str), Some(entry_str), Some(lev_str)) = (
                    pos_val.get("symbol").and_then(|v| v.as_str()),
                    pos_val.get("positionAmt").and_then(|v| v.as_str()),
                    pos_val.get("entryPrice").and_then(|v| v.as_str()),
                    pos_val.get("leverage").and_then(|v| v.as_str()),
                ) {
                    let amt: f64 = amt_str.parse().unwrap_or(0.0);
                    let lev: u8 = lev_str.parse().unwrap_or(1);
                    if amt != 0.0 {
                        remote_symbols.insert(sym.to_string());
                        
                        if let Some(mut local_pos) = self.positions.get_mut(sym) {
                            let entry: f64 = entry_str.parse().unwrap_or(0.0);
                            
                            if entry != 0.0 && (local_pos.entry_price - entry).abs() / entry > 0.001 {
                                warn!("[WARN] Position Drift Corrected for {}: Local={}, Remote={}", sym, local_pos.entry_price, entry);
                                local_pos.entry_price = entry;
                            }
                            
                            if (local_pos.position_amt - amt).abs() > f64::EPSILON {
                                warn!("[WARN] Position Amount Drift Corrected for {}", sym);
                                local_pos.position_amt = amt;
                            }
                            
                            if local_pos.leverage != lev {
                                local_pos.leverage = lev;
                            }
                        } else {
                            let entry: f64 = entry_str.parse().unwrap_or(0.0);
                            warn!("👻 [WARN] 发现幽灵仓位 (远端存在但本地缺失): {} (Amt: {}). 已强制拉取至本地风控接管!", sym, amt);
                            self.positions.insert(
                                sym.to_string(),
                                crate::models::MockPosition::new(
                                    sym.to_string(), entry, amt, lev, crate::risk_guard::current_time_ms(), None, None, 0.0, false
                                )
                            );
                        }
                    }
                }
            }
            
            let local_symbols: Vec<String> = self.positions.iter().map(|kv| kv.key().clone()).collect();
            for sym in local_symbols {
                if !remote_symbols.contains(&sym) {
                    warn!("[WARN] Local position {} does not exist remotely. Removing zombie record.", sym);
                    self.positions.remove(&sym);
                }
            }
        }
    }
}
