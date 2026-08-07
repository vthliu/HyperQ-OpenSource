mod config;
mod models;
mod logging;
mod rest_api;
mod binance_ws;
mod ws_api;
mod executor;
mod sentinel;
mod risk_guard;
mod metrics;
mod signal_zmq;
mod models_signal;
mod trend_processor;
mod reconciler;
mod features;
mod inference;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use std::sync::Arc;
use dashmap::DashMap;
use dotenv::dotenv;
use tracing::{info, warn};

use models::MockPosition;
use config::AppConfig;
use rest_api::RestApi;
use binance_ws::BinanceWs;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();
    
    // Load config
    let config = AppConfig::load().expect("Failed to load config");
    
    // Initialize logging
    logging::init_logging(&config.logging.level, &config.logging.format);
    
    info!("🚀 Starting HyperQ V4.1 (Environment: {})", config.binance.env);

    // Initialize Global State
    let price_map = Arc::new(DashMap::<String, (f64, u64)>::new());
    let position_map = Arc::new(DashMap::<String, MockPosition>::new());

    // Initialize REST API
    let rest_api = Arc::new(RestApi::new(
        &config.binance.env,
        config.binance.api_key.clone(),
        config.binance.api_secret.clone(),
    ));

    // Phase 1: Startup Self-Healing (Patch 1)
    info!("=== DIAGNOSTIC INFO ===");
    info!("ENV: {}", config.binance.env);
    if config.binance.api_key.len() >= 4 {
        info!("API_KEY_PREFIX: {}", &config.binance.api_key[..4]);
    }
    info!("SECRET_LEN: {}", config.binance.api_secret.len());
    match rest_api.get_server_time().await {
        Ok(t) => {
            let local_t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
            info!("Server time: {}, Local time: {}, Diff ms: {}", t, local_t, (t as i64 - local_t as i64).abs());
        },
        Err(e) => info!("Server time error: {}", e),
    }
    info!("========================");

    info!("执行启动自愈与 Binance 持仓同步...");
    
    if let Ok(positions_val) = rest_api.get_position_risk().await {
        let mut active_count = 0;
        for pos_val in positions_val {
            if let (Some(sym), Some(amt_str), Some(entry_str), Some(lev_str), Some(margin_str)) = (
                pos_val.get("symbol").and_then(|v| v.as_str()),
                pos_val.get("positionAmt").and_then(|v| v.as_str()),
                pos_val.get("entryPrice").and_then(|v| v.as_str()),
                pos_val.get("leverage").and_then(|v| v.as_str()),
                pos_val.get("isolatedMargin").and_then(|v| v.as_str()),
            ) {
                let amt: f64 = amt_str.parse().unwrap_or(0.0);
                if amt != 0.0 {
                    active_count += 1;
                    let entry: f64 = entry_str.parse().unwrap_or(0.0);
                    let lev: u8 = lev_str.parse().unwrap_or(1);
                    let _margin: f64 = margin_str.parse().unwrap_or(0.0);
                    let mut entry_time = pos_val.get("updateTime")
                        .and_then(|v| v.as_u64())
                        .unwrap_or_else(|| crate::risk_guard::current_time_ms());
                    if entry_time == 0 {
                        warn!("⚠️ Position updateTime is 0 for {}, falling back to current time.", sym);
                        entry_time = crate::risk_guard::current_time_ms();
                    }
                    
                    position_map.insert(sym.to_string(), MockPosition::new(
                        sym.to_string(), entry, amt, lev, entry_time, None, None, 0.0
                    ));
                }
            }
        }
        info!("✅ 持仓同步完成: {} 个活跃仓位", active_count);
    }
    
    if let Ok(open_orders) = rest_api.get_open_orders().await {
        let mut symbols_to_cancel = std::collections::HashSet::new();
        for order in open_orders {
            if let Some(sym) = order.get("symbol").and_then(|v| v.as_str()) {
                symbols_to_cancel.insert(sym.to_string());
            }
        }
        
        let canceled_count = symbols_to_cancel.len();
        for sym in symbols_to_cancel {
            let _ = rest_api.cancel_all_orders(&sym).await;
        }
        info!("✅ 清理挂单完成: {} 个币种被撤销挂单", canceled_count);
    }
    
    info!("自愈同步完成。");

    // === Reconciler: Auto-heal Ghost Positions ===
    {
        let reconciler = crate::reconciler::Reconciler::new(
            position_map.clone(),
            rest_api.clone(),
        );
        tokio::spawn(async move {
            reconciler.run().await;
        });
    }

    // === Binance WS: High Frequency Price Stream ===
    // Enabled for ultra-low latency price tracking.
    let ofi_map = Arc::new(dashmap::DashMap::new());
    let ws = Arc::new(crate::binance_ws::BinanceWs::new(&config.binance.env, price_map.clone(), ofi_map.clone()));
    let ws_clone = ws.clone();
    tokio::spawn(async move {
        ws_clone.start().await;
    });

    let ofi_map_depth = ofi_map.clone();
    let env_clone = config.binance.env.clone();
    
    // === HOT-COIN RADAR ===
    let hot_coins = Arc::new(tokio::sync::RwLock::new(Vec::new()));
    let hot_coins_clone = hot_coins.clone();
    let rest_api_hot = rest_api.clone();
    
    tokio::spawn(async move {
        loop {
            tracing::info!("📡 [HOT-COIN RADAR] Scanning global market for top active coins...");
            if let Ok(tickers) = rest_api_hot.get_24hr_tickers().await {
                let mut filtered: Vec<_> = tickers.into_iter()
                    .filter(|(_, quote_vol, _)| *quote_vol > 50_000_000.0) // Require at least $50M 24h volume
                    .collect();
                
                // Sort by absolute price change (Volatility / Momentum)
                filtered.sort_by(|a, b| b.2.abs().partial_cmp(&a.2.abs()).unwrap_or(std::cmp::Ordering::Equal));
                
                let top_20: Vec<String> = filtered.into_iter().take(20).map(|(sym, _, _)| sym).collect();
                tracing::info!("🔥 [HOT-COIN RADAR] Top 20 Hot Coins Extracted: {:?}", top_20);
                
                let mut wl = hot_coins_clone.write().await;
                *wl = top_20;
            }
            tokio::time::sleep(std::time::Duration::from_secs(300)).await; // Update every 5 mins
        }
    });

    let hot_coins_ws = hot_coins.clone();
    let env_force = env_clone.clone();
    tokio::spawn(async move {
        let depth_ws = crate::binance_ws::BinanceWsDepth::new(&env_clone, ofi_map_depth);
        depth_ws.start(hot_coins_ws).await;
    });

    // === ForceOrder / 强平流 (Liquidations) ===
    let liq_map: Arc<DashMap<String, (f64, f64)>> = Arc::new(DashMap::new());
    let liq_map_ws = liq_map.clone();
    tokio::spawn(async move {
        let force_ws = crate::binance_ws::BinanceWsForceOrder::new(&env_force, liq_map_ws);
        force_ws.start().await;
    });

    // 强平流衰减任务（每分钟衰减20%，避免历史爆仓影响当前决策）
    let liq_map_decay = liq_map.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            for mut entry in liq_map_decay.iter_mut() {
                let (long_liq, short_liq) = *entry.value();
                *entry.value_mut() = (long_liq * 0.8, short_liq * 0.8);
            }
        }
    });


    // Start Metrics Server
    tokio::spawn(metrics::start_metrics_server(config.metrics.prometheus_port));

    // Fetch exchange info for tick sizes
    info!("🔄 Fetching Exchange Info for Tick Sizes...");
    let tick_sizes = Arc::new(match rest_api.get_exchange_info().await {
        Ok(map) => {
            info!("✅ Exchange Info loaded. {} symbols.", map.len());
            map
        }
        Err(e) => {
            tracing::error!("Failed to fetch exchange info: {}. Exiting.", e);
            std::process::exit(1);
        }
    });

    // Initialize WS API for fast execution
    let ws_api = crate::ws_api::WsApi::new(rest_api.clone(), tick_sizes.clone());
    if let Err(e) = ws_api.connect(&config.binance.env).await {
        tracing::error!("Failed to connect to Binance WS API: {}", e);
    } else {
        info!("✅ WebSocket API connection established.");
    }

    // Initialize Funding Rates Map and Updater Task
    let funding_map = Arc::new(DashMap::new());
    let funding_map_clone = funding_map.clone();
    let rest_api_funding = rest_api.clone();
    tokio::spawn(async move {
        loop {
            match rest_api_funding.get_funding_rates().await {
                Ok(rates) => {
                    for (sym, (rate, next_time)) in rates {
                        funding_map_clone.insert(sym, (rate, next_time));
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to fetch funding rates: {}", e);
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(300)).await; // Every 5 mins
        }
    });

    // === OI (持仓量) 轮询任务 ===
    // 每5分钟扫描一次热门妖币的持仓量变化，用于区分真实建仓 vs 空头爆仓假突破
    let oi_map: Arc<DashMap<String, (f64, f64)>> = Arc::new(DashMap::new()); // (prev_oi, curr_oi)
    let oi_map_poller = oi_map.clone();
    let rest_api_oi = rest_api.clone();
    let hot_coins_oi = hot_coins.clone();
    tokio::spawn(async move {
        loop {
            let coins = hot_coins_oi.read().await.clone();
            for sym in &coins {
                if let Ok(curr_oi) = rest_api_oi.get_open_interest(sym).await {
                    let prev_oi = oi_map_poller.get(sym).map(|v| v.1).unwrap_or(curr_oi);
                    oi_map_poller.insert(sym.clone(), (prev_oi, curr_oi));
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await; // 避免频繁限速
            }
            tracing::info!("📈 [OI RADAR] 持仓量扫描完成，共 {} 个妖币", coins.len());
            tokio::time::sleep(std::time::Duration::from_secs(300)).await; // 每5分钟
        }
    });

    // Initialize Executor & Sentinel
    let penalty_map: Arc<DashMap<String, (f64, u64)>> = Arc::new(DashMap::new());
    
    let executor = Arc::new(crate::executor::Executor::new(
        rest_api.clone(),
        ws_api.clone(),
        position_map.clone(),
        funding_map.clone(),
        config.executor.clone(),
        config.dry_run,
        config.position.max_leverage,
    ));

    let sentinel = Arc::new(crate::sentinel::Sentinel::new(
        position_map.clone(),
        executor.clone(),
        funding_map.clone(),
        config.sentinel.clone(),
        config.dry_run,
        config.time_stop.clone(),
        penalty_map.clone(),
    ));
    sentinel.start();

    let (tx_signal, rx_signal) = tokio::sync::mpsc::channel(256);
    let rest_api_inference = rest_api.clone();
    let asset_tiers = config.asset_tiers.clone();
    let ofi_map_inference = ofi_map.clone();
    let hot_coins_inference = hot_coins.clone();
    let oi_map_inference = oi_map.clone();
    let liq_map_inference = liq_map.clone();
    let penalty_map_inference = penalty_map.clone();
    let prev_prob_map: Arc<DashMap<String, (f32, f32)>> = Arc::new(DashMap::new());
    let prev_prob_map_inference = prev_prob_map.clone();
    let position_map_inference = position_map.clone();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        

        rt.block_on(async move {
            crate::inference::start_inference_loop(
                rest_api_inference, 
                tx_signal, 
                asset_tiers, 
                ofi_map_inference, 
                position_map_inference,
                hot_coins_inference, 
                oi_map_inference,
                liq_map_inference,
                penalty_map_inference,
                prev_prob_map_inference,
            ).await;
        });
    });

    let mut trend_processor = crate::trend_processor::TrendProcessor::new(
        rx_signal,
        position_map.clone(),
        executor.clone(),
        config.position.max_positions,
        config.position.rwa_risk_multiplier,
        config.position.max_leverage as f64,
        config.dry_run,
        config.prob_threshold.clone(),
        config.defense_mode.clone(),
        config.position.dynamic_sizing.clone(),
        penalty_map.clone(),
    );

    let risk_guard = Arc::new(crate::risk_guard::RiskGuard::new(position_map.clone(), executor.clone(), price_map.clone(), config.risk_guard.clone(), config.asset_tiers.clone()));
    
    // Run Fast RiskGuard Cycle (100ms) for real-time ROE tracking and trailing stop triggers
    let risk_guard_fast = risk_guard.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            risk_guard_fast.update_roe_cycle().await;
        }
    });

    // Run Macro RiskGuard Cycle (60s) for state transitions and PnL logging
    let risk_guard_macro = risk_guard.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            risk_guard_macro.run_macro_cycle().await;
        }
    });

    // Main Loop
    info!("Starting Main Control Loop...");
    loop {
        if let Some(signal) = trend_processor.rx.recv().await {
            trend_processor.process_signal(signal).await;
        } else {
            break;
        }
    }

    Ok(())
}
