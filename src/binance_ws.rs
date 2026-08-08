use std::sync::Arc;
use dashmap::DashMap;
use futures::{StreamExt, SinkExt};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing::{info, warn, error};
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

#[derive(Deserialize)]
struct AggTradeMsg {
    stream: String,
    data: AggTradeData,
}

#[derive(Deserialize)]
struct AggTradeData {
    q: String, // Quantity
    m: bool,   // Is the buyer the market maker? (true = seller initiated/market sell, false = buyer initiated/market buy)
}

pub struct BinanceWs {
    ws_url: String,
    price_map: Arc<DashMap<String, (f64, f64, u64)>>,
}

impl BinanceWs {
    pub fn new(env: &str, price_map: Arc<DashMap<String, (f64, f64, u64)>>) -> Self {
        let ws_url = if env == "mainnet" {
            "wss://fstream.binance.com/ws".to_string()
        } else {
            "wss://stream.binancefuture.com/ws".to_string()
        };
        Self { ws_url, price_map }
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
                let ts = msg.e.unwrap_or_else(|| {
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
                });
                self.price_map.insert(msg.s, (bid, ask, ts));
            }
        }
    }
}

// Dedicated struct for maintaining L2 Order Flow Imbalance
pub struct BinanceWsDepth {
    stream_url: String,
    ofi_map: Arc<DashMap<String, f64>>,
    cvd_map: Arc<DashMap<String, f64>>,
}

impl BinanceWsDepth {
    pub fn new(env: &str, ofi_map: Arc<DashMap<String, f64>>, cvd_map: Arc<DashMap<String, f64>>) -> Self {
        let base_url = if env == "mainnet" {
            "wss://fstream.binance.com/stream"
        } else {
            "wss://stream.binancefuture.com/stream"
        };
        Self { stream_url: base_url.to_string(), ofi_map, cvd_map }
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
            let streams: Vec<String> = current_symbols.iter().flat_map(|s| {
                vec![
                    format!("{}@depth20@100ms", s.to_lowercase()),
                    format!("{}@aggTrade", s.to_lowercase())
                ]
            }).collect();
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
                                    self.handle_msg(&text);
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

    fn handle_msg(&self, text: &str) {
        // Fast path check to distinguish messages
        if text.contains("@depth20@100ms") {
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
        } else if text.contains("@aggTrade") {
            if let Ok(msg) = serde_json::from_str::<AggTradeMsg>(text) {
                let sym = msg.stream.split('@').next().unwrap_or("").to_uppercase();
                if !sym.is_empty() {
                    if let Ok(qty) = msg.data.q.parse::<f64>() {
                        // m = true indicates maker is buyer, meaning this is a market sell (negative CVD)
                        // m = false indicates maker is seller, meaning this is a market buy (positive CVD)
                        let delta = if msg.data.m { -qty } else { qty };
                        
                        // Decay old CVD slightly and add new delta to keep it a rolling metric
                        let mut entry = self.cvd_map.entry(sym).or_insert(0.0);
                        *entry = (*entry * 0.99) + delta; 
                    }
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

#[derive(Deserialize, Debug)]
pub struct TickerMsg {
    pub s: String,
    pub P: String, // Price change percent
    pub h: String, // High price 24h
    pub l: String, // Low price 24h
    pub v: String, // Volume 24h
}

pub struct BinanceWsTicker {
    ws_url: String,
    pub ticker_map: Arc<DashMap<String, crate::models::TickerData>>,
}

impl BinanceWsTicker {
    pub fn new(env: &str, ticker_map: Arc<DashMap<String, crate::models::TickerData>>) -> Self {
        let ws_url = if env == "mainnet" {
            "wss://fstream.binance.com/ws/!ticker@arr".to_string()
        } else {
            "wss://stream.binancefuture.com/ws/!ticker@arr".to_string()
        };
        Self { ws_url, ticker_map }
    }

    pub async fn start(&self) {
        loop {
            info!("🔗 Connecting to Ticker WS: {}", self.ws_url);
            match connect_async(&self.ws_url).await {
                Ok((mut ws_stream, _)) => {
                    info!("✅ Connected to Ticker WS (!ticker@arr)!");
                    loop {
                        match tokio::time::timeout(std::time::Duration::from_secs(30), ws_stream.next()).await {
                            Ok(Some(msg)) => {
                                if let Ok(Message::Text(text)) = msg {
                                    if let Ok(tickers) = serde_json::from_str::<Vec<TickerMsg>>(&text) {
                                        for t in tickers {
                                            if let (Ok(pct), Ok(high), Ok(low), Ok(vol)) = (
                                                t.P.parse::<f64>(),
                                                t.h.parse::<f64>(),
                                                t.l.parse::<f64>(),
                                                t.v.parse::<f64>(),
                                            ) {
                                                self.ticker_map.insert(t.s, crate::models::TickerData {
                                                    price_change_pct: pct,
                                                    high_24h: high,
                                                    low_24h: low,
                                                    volume_24h: vol,
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                            Ok(None) => break,
                            Err(_) => break, // Timeout
                        }
                    }
                }
                Err(e) => error!("Failed to connect to Ticker WS: {}", e),
            }
            warn!("Ticker WS disconnected. Reconnecting in 2 seconds...");
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }
}
