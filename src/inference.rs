use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use zeromq::{Socket, SocketRecv, SocketSend, ReqSocket};
use crate::features::FeatureEngine;
use crate::models_signal::Signal;
use crate::rest_api::RestApi;
use uuid::Uuid;

pub struct InferenceEngine {
    socket_addr: String,
}

impl InferenceEngine {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            socket_addr: "tcp://127.0.0.1:5556".to_string(),
        })
    }

    pub async fn predict_with_context(&self, symbol: &str, features_15m: Vec<f64>, ofi: f64, price_vs_range: f64, change_24h_pct: f64, oi_change_pct: f64, liq_imbalance: f64, taker_buy_ratio: f64, distance_to_high: f64) -> Result<(f32, f32, String), String> {
        // 将 ofi 追加到 15m 特征的末尾（因为 15m 是微观特征，跟 orderbook 最相关）
        let mut f_15m = features_15m.clone();
        f_15m.push(ofi);
        
        let req_json = serde_json::json!({
            "symbol": symbol,
            "features_15m": f_15m,
            "price_vs_range": price_vs_range,
            "change_24h_pct": change_24h_pct,
            "oi_change_pct": oi_change_pct,
            "liq_imbalance": liq_imbalance,
            "taker_buy_ratio": taker_buy_ratio,
            "distance_to_high": distance_to_high,
        });
        
        let mut socket = ReqSocket::new();
        socket.connect(&self.socket_addr).await.map_err(|e| e.to_string())?;
        
        socket.send(req_json.to_string().into()).await.map_err(|e| e.to_string())?;
        
        let repl = match tokio::time::timeout(std::time::Duration::from_millis(500), socket.recv()).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(e.to_string()),
            Err(_) => return Err("ZMQ Timeout".to_string()),
        };
        
        let text = String::from_utf8_lossy(repl.get(0).unwrap());
        let resp: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        if resp["status"] != "success" {
            return Err(resp["message"].as_str().unwrap_or("Unknown error").to_string());
        }
        
        let p_long = resp["prob_long"].as_f64().unwrap_or(0.0) as f32;
        let p_short = resp["prob_short"].as_f64().unwrap_or(0.0) as f32;
        let regime = resp["regime"].as_str().unwrap_or("CHOP_HIGH_VOL").to_string();
        
        Ok((p_long, p_short, regime))
    }
}

pub async fn start_inference_loop(
    rest_api: Arc<RestApi>,
    tx_signal: Sender<Signal>,
    asset_tiers: crate::config::AssetTiersConfig,
    ofi_map: Arc<dashmap::DashMap<String, f64>>,
    position_map: Arc<dashmap::DashMap<String, crate::models::MockPosition>>,
    hot_coins: Arc<tokio::sync::RwLock<Vec<String>>>,
    oi_map: Arc<dashmap::DashMap<String, (f64, f64)>>,
    liq_map: Arc<dashmap::DashMap<String, (f64, f64)>>,
    _prev_prob_map: Arc<dashmap::DashMap<String, (f32, f32)>>,
) {
    let engine1 = InferenceEngine::new().ok();
    let engine2 = InferenceEngine::new().ok();
    let engine3 = InferenceEngine::new().ok();

    loop {
        tracing::info!("Starting inference cycle...");
        let dynamic_hot_coins = hot_coins.read().await.clone();
        
        // 提取当前所有持仓的币种，确保它们无论如何都会被 AI 监控
        let mut held_symbols = Vec::new();
        for entry in position_map.iter() {
            held_symbols.push(entry.key().clone());
        }
        
        let tiers: Vec<(&str, Vec<String>, &Option<InferenceEngine>, &str)> = vec![
            ("layer1", dynamic_hot_coins, &engine1, "1h"),
            ("layer2", asset_tiers.layer2.clone(), &engine2, "1h"),
            ("layer3", asset_tiers.layer3.clone(), &engine3, "4h"),
            ("held_positions", held_symbols, &engine1, "1h"), // 强制独立梯队：持仓专属体检
        ];
        
        // 每个推理周期只允许发送一个最高胜率信号
        let mut best_signal: Option<Signal> = None;

        for (tier_name, symbols, engine_opt, _) in tiers {
            if let Some(engine) = engine_opt {

                for sym in symbols {
                    // 刺客模式：仅需请求 15m 级别 K 线，大幅降低网络延迟
                    let f_15m = rest_api.get_klines(&sym, "15m", 200);
                    
                    if let Ok(klines_15m) = f_15m.await {
                        if klines_15m.len() >= 96 {
                            let ofi = ofi_map.get(&sym).map(|r| *r).unwrap_or(0.0);
                            
                            // 宏观计算 (使用 15m 计算过去 24 小时的最高最低价，即过去 96 根 15m K线)
                            let (high_24h, low_24h) = klines_15m.iter().rev().take(96).fold((f64::MIN, f64::MAX), |(h, l), k| {
                                (h.max(k.high), l.min(k.low))
                            });
                                let last_close = klines_15m.last().map(|k| k.close).unwrap_or(0.0);
                                let range_24h = high_24h - low_24h;
                                let price_vs_range = if range_24h > 0.0 { (last_close - low_24h) / range_24h } else { 0.5 };
                                let change_24h_pct = if low_24h > 0.0 { (last_close - low_24h) / low_24h } else { 0.0 };
                                
                                let oi_change_pct = if let Some(oi) = oi_map.get(&sym) {
                                    let (prev, curr) = *oi;
                                    if prev > 0.0 { (curr - prev) / prev } else { 0.0 }
                                } else { 0.0 };
                                
                                let liq_imbalance = if let Some(liq) = liq_map.get(&sym) {
                                    let (long_liq, short_liq) = *liq;
                                    let total_liq = long_liq + short_liq;
                                    if total_liq > 0.0 { (short_liq - long_liq) / total_liq } else { 0.0 }
                                } else { 0.0 };
                                
                                let last_k = klines_15m.last().unwrap();
                                let taker_buy_ratio = if last_k.quote_asset_volume > 0.0 {
                                    last_k.taker_buy_quote / last_k.quote_asset_volume
                                } else { 0.5 };
                                
                                let distance_to_high = if high_24h > 0.0 {
                                    (high_24h - last_close) / high_24h
                                } else { 0.0 };
                                
                                // 刺客模式：仅计算 15m 动量
                                let features_15m = FeatureEngine::compute_features(&klines_15m);
                                
                                // 注意：我们依然将 15m 的 ATR 传递给后续系统用来算止损线，因为它反映微观波动
                                let atr = features_15m[16];

                                if let Ok((prob_long, prob_short, regime)) = engine.predict_with_context(
                                    &sym, features_15m, 
                                    ofi, price_vs_range, change_24h_pct, oi_change_pct, liq_imbalance, taker_buy_ratio, distance_to_high
                                ).await {
                                    let is_long = prob_long > prob_short;
                                    let prob = if is_long { prob_long } else { prob_short };

                                    // Spread 过滤：多空分歧必须显著 (≥ 0.20)
                                    let spread = (prob_long - prob_short).abs();
                                    if spread < 0.20 {
                                        tracing::debug!("[VETO: Spread] {} spread {:.3} < 0.20, ignored.", sym, spread);
                                        continue;
                                    }

                                    let candidate = Signal {
                                        msg_id: Uuid::new_v4().to_string(),
                                        timestamp: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64,
                                        symbol: sym.clone(),
                                        price: last_close,
                                        atr_24h: atr,
                                        is_long,
                                        raw_score: prob as f64,
                                        prob: prob as f64,
                                        tier: Some(tier_name.to_string()),
                                        is_new_symbol: None,
                                        regime: Some(regime),
                                    };

                                    // 如果当前币种已经有持仓，直接发送信号进行持续的趋势监控（用于反转平仓）
                                    if position_map.contains_key(&sym) {
                                        tracing::info!("[POSITION MONITOR] {} prob={:.3} is_long={}", sym, prob, is_long);
                                        let _ = tx_signal.send(candidate).await;
                                        continue; // 不参与 best_signal 的竞争
                                    }

                                    // 对于非持仓币种，只保留当前周期内胜率最高的信号
                                    match &best_signal {
                                        None => best_signal = Some(candidate),
                                        Some(prev) if candidate.prob > prev.prob => best_signal = Some(candidate),
                                        _ => {}
                                    }
                                }
                            }
                        } else {
                            tracing::warn!("Failed to fetch klines for {}", sym);
                        }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }

        // 每个推理周期发送最高质量的一个信号
        if let Some(sig) = best_signal {
            tracing::info!("[BEST SIGNAL] {} prob={:.3} is_long={}", sig.symbol, sig.prob, sig.is_long);
            
            // --- Pin-Bar VETO 防线 ---
            let mut vetoed = false;
            // 极速定向抓取：只针对这唯一的待开仓币种，抓取实时 5m K线进行插针验明
            if let Ok(klines_5m) = rest_api.get_klines(&sig.symbol, "5m", 2).await {
                if let Some(last_k) = klines_5m.last() {
                    if sig.is_long {
                        let drop = (last_k.high - last_k.close) / last_k.close;
                        if drop > 0.008 {
                            tracing::warn!("[VETO: Pin-Bar] 🛑 拒绝做多 {} | 实时 5m 回撤幅度: {:.2}% > 0.8% | 规避画门", sig.symbol, drop * 100.0);
                            vetoed = true;
                        }
                    } else {
                        let bounce = (last_k.close - last_k.low) / last_k.low;
                        if bounce > 0.008 {
                            tracing::warn!("[VETO: Pin-Bar] 🛑 拒绝做空 {} | 实时 5m 反弹幅度: {:.2}% > 0.8% | 规避深V", sig.symbol, bounce * 100.0);
                            vetoed = true;
                        }
                    }
                }
            }
            
            if !vetoed {
                let _ = tx_signal.send(sig).await;
            }
        }
        
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}
