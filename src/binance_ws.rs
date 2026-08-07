use std::sync::Arc;
use dashmap::DashMap;
use futures::{StreamExt, SinkExt};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing::{info, warn, error};
use serde_json::Value;
use serde::Deserialize;

#[derive(Deserialize)]
struct BookTickerMsg {
    s: String,
    b: String,
    a: String,
    #[serde(rename = "E")]
    e: Option<u64>,
}

#[derive(Deserialize)]
struct DepthUpdateMsg {
    stream: String,
    data: DepthData,
}

#[derive(Deserialize)]
struct DepthData {
    bids: Vec<[String; 2]>,
    asks: Vec<[String; 2]>,
}

pub struct BinanceWs {
    ws_url: String,
    price_map: Arc<DashMap<String, (f64, u64)>>,
    ofi_map: Arc<DashMap<String, f64>>, // Order Flow Imbalance Map
    last_log_time: std::sync::atomic::AtomicU64,
}

impl BinanceWs {
    pub fn new(env: &str, price_map: Arc<DashMap<String, (f64, u64)>>, ofi_map: Arc<DashMap<String, f64>>) -> Self {
        let ws_url = if env == "mainnet" {
            "wss://fstream.binance.com/ws".to_string()
        } else {
            "wss://stream.binancefuture.com/ws".to_string()
        };
        Self { ws_url, price_map, ofi_map, last_log_time: std::sync::atomic::AtomicU64::new(0) }
    }

    pub async fn start(&self) {
        let url = format!("{}/!bookTicker", self.ws_url);
        let mut first_msg_logged = false;
        
        loop {
            info!("Connecting to Binance WS: {}", url);
            match connect_async(&url).await {
                Ok((mut ws_stream, _)) => {
                    info!("Connected to Binance WS (!bookTicker)!");
                    loop {
                        match tokio::time::timeout(std::time::Duration::from_secs(15), ws_stream.next()).await {
                            Ok(Some(msg)) => {
                                match msg {
                                    Ok(Message::Text(text)) => {
                                        if !first_msg_logged {
                                            info!("📡 [WS FIRST MSG] len={} sample={:.300}", text.len(), &text);
                                            first_msg_logged = true;
                                        }
                                        self.handle_message(&text);
                                    }
                                    Ok(Message::Ping(ping)) => {
                                        if let Err(e) = ws_stream.send(Message::Pong(ping)).await {
                                            error!("Failed to send Pong: {}", e);
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        error!("WS Error: {}", e);
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                            Ok(None) => {
                                warn!("WS stream closed cleanly by server.");
                                break;
                            }
                            Err(_) => {
                                warn!("⚠️ [WS Timeout] No data received for 15s. Forcing reconnect.");
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to connect to Binance WS: {}", e);
                }
            }
            warn!("WS disconnected. Reconnecting in 2 seconds...");
            first_msg_logged = false;
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }

    fn handle_message(&self, text: &str) {
        if let Ok(msg) = serde_json::from_str::<BookTickerMsg>(text) {
            if let (Ok(bid), Ok(ask)) = (msg.b.parse::<f64>(), msg.a.parse::<f64>()) {
                let price = (bid + ask) / 2.0;
                let ts = msg.e.unwrap_or_else(|| {
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
                });
                self.price_map.insert(msg.s, (price, ts));
            }
        }
    }
}

// Dedicated struct for maintaining L2 Order Flow Imbalance
pub struct BinanceWsDepth {
    stream_url: String,
    ofi_map: Arc<DashMap<String, f64>>,
}

impl BinanceWsDepth {
    pub fn new(env: &str, ofi_map: Arc<DashMap<String, f64>>) -> Self {
        let base_url = if env == "mainnet" {
            "wss://fstream.binance.com/stream"
        } else {
            "wss://stream.binancefuture.com/stream"
        };
        Self { stream_url: base_url.to_string(), ofi_map }
    }

    pub async fn start(&self, hot_coins: Arc<tokio::sync::RwLock<Vec<String>>>) {
        let mut current_symbols: Vec<String> = Vec::new();
        
        loop {
            let latest_symbols = hot_coins.read().await.clone();
            if latest_symbols.is_empty() {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }
            
            if latest_symbols != current_symbols {
                info!("🔄 L2 WS: Hot coins changed. Updating subscriptions.");
                current_symbols = latest_symbols.clone();
            }
            
            let streams: Vec<String> = current_symbols.iter().map(|s| format!("{}@depth20@100ms", s.to_lowercase())).collect();
            let query = streams.join("/");
            let url = format!("{}?streams={}", self.stream_url, query);
            
            info!("Connecting to L2 Depth WS: {} streams", streams.len());
            match connect_async(&url).await {
                Ok((mut ws_stream, _)) => {
                    info!("Connected to L2 Depth WS!");
                    loop {
                        // Check if hot coins changed
                        if *hot_coins.read().await != current_symbols {
                            warn!("🔥 Hot coins updated! Forcing L2 WS reconnect...");
                            break; // break inner loop to reconnect
                        }

                        match tokio::time::timeout(std::time::Duration::from_secs(15), ws_stream.next()).await {
                            Ok(Some(msg)) => {
                                if let Ok(Message::Text(text)) = msg {
                                    self.handle_depth_msg(&text);
                                }
                            }
                            Ok(None) => break,
                            Err(_) => break, // Timeout
                        }
                    }
                }
                Err(e) => error!("Failed to connect to L2 Depth WS: {}", e),
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }

    fn handle_depth_msg(&self, text: &str) {
        if let Ok(msg) = serde_json::from_str::<DepthUpdateMsg>(text) {
            // Compute OFI: (Bid Vol - Ask Vol) / (Bid Vol + Ask Vol)
            let mut bid_vol = 0.0;
            let mut ask_vol = 0.0;
            
            for b in msg.data.bids {
                if let Ok(v) = b[1].parse::<f64>() { bid_vol += v; }
            }
            for a in msg.data.asks {
                if let Ok(v) = a[1].parse::<f64>() { ask_vol += v; }
            }
            
            let total_vol = bid_vol + ask_vol;
            if total_vol > 0.0 {
                let ofi = (bid_vol - ask_vol) / total_vol;
                // stream name is like btcusdt@depth20@100ms. Extract symbol
                let sym = msg.stream.split('@').next().unwrap_or("").to_uppercase();
                if !sym.is_empty() {
                    self.ofi_map.insert(sym, ofi);
                }
            }
        }
    }
}

// ==========================================
// 强平流 (Liquidation / ForceOrder)
// ==========================================
#[derive(Deserialize)]
struct ForceOrderMsg {
    #[serde(rename = "o")]
    order: ForceOrderData,
}

#[derive(Deserialize)]
struct ForceOrderData {
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "S")]
    side: String,
    #[serde(rename = "q")]
    qty: String,
    #[serde(rename = "p")]
    price: String,
}

pub struct BinanceWsForceOrder {
    ws_url: String,
    liq_map: Arc<DashMap<String, (f64, f64)>>, // (long_liq_vol, short_liq_vol)
}

impl BinanceWsForceOrder {
    pub fn new(env: &str, liq_map: Arc<DashMap<String, (f64, f64)>>) -> Self {
        let ws_url = if env == "mainnet" {
            "wss://fstream.binance.com/ws/!forceOrder@arr".to_string()
        } else {
            "wss://stream.binancefuture.com/ws/!forceOrder@arr".to_string()
        };
        Self { ws_url, liq_map }
    }

    pub async fn start(&self) {
        loop {
            info!("🔗 Connecting to ForceOrder WS: {}", self.ws_url);
            match connect_async(&self.ws_url).await {
                Ok((mut ws_stream, _)) => {
                    info!("✅ Connected to ForceOrder WS (全市场爆仓流)!");
                    loop {
                        match tokio::time::timeout(std::time::Duration::from_secs(60), ws_stream.next()).await {
                            Ok(Some(msg)) => {
                                if let Ok(Message::Text(text)) = msg {
                                    self.handle_message(&text);
                                } else if let Ok(Message::Ping(ping)) = msg {
                                    let _ = ws_stream.send(Message::Pong(ping)).await;
                                }
                            }
                            Ok(None) => break,
                            Err(_) => break, // Timeout
                        }
                    }
                }
                Err(e) => error!("Failed to connect to ForceOrder WS: {}", e),
            }
            warn!("⚠️ ForceOrder WS disconnected. Reconnecting in 2 seconds...");
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }

    fn handle_message(&self, text: &str) {
        if let Ok(msg) = serde_json::from_str::<ForceOrderMsg>(text) {
            let sym = msg.order.symbol;
            if let (Ok(qty), Ok(price)) = (msg.order.qty.parse::<f64>(), msg.order.price.parse::<f64>()) {
                let liq_vol = qty * price;
                
                let mut current_long_liq = 0.0;
                let mut current_short_liq = 0.0;
                
                if let Some(v) = self.liq_map.get(&sym) {
                    current_long_liq = v.0;
                    current_short_liq = v.1;
                }
                
                // Binance's 'side' in forceOrder indicates the side of the forced order.
                // SELL means a LONG position was liquidated.
                // BUY means a SHORT position was liquidated.
                if msg.order.side == "SELL" {
                    current_long_liq += liq_vol;
                } else if msg.order.side == "BUY" {
                    current_short_liq += liq_vol;
                }
                
                self.liq_map.insert(sym, (current_long_liq, current_short_liq));
            }
        }
    }
}
