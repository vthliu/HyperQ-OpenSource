use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::{Mutex, oneshot};
use dashmap::DashMap;
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use serde_json::{Value, json};
use tracing::{info, warn, error, debug};
use std::time::Instant;

use crate::rest_api::RestApi;

pub struct WsApi {
    rest: Arc<RestApi>,
    ws_tx: Arc<Mutex<Option<futures::channel::mpsc::UnboundedSender<String>>>>,
    pending_requests: Arc<DashMap<String, oneshot::Sender<Value>>>,
    req_counter: std::sync::atomic::AtomicU64,
    tick_sizes: Arc<DashMap<String, f64>>,
}

impl WsApi {
    pub fn new(rest: Arc<RestApi>, tick_sizes: Arc<DashMap<String, f64>>) -> Arc<Self> {
        Arc::new(Self {
            rest,
            ws_tx: Arc::new(Mutex::new(None)),
            pending_requests: Arc::new(DashMap::new()),
            req_counter: std::sync::atomic::AtomicU64::new(1),
            tick_sizes,
        })
    }
    
    pub async fn connect(&self, env: &str) -> Result<(), String> {
        let ws_url = if env == "mainnet" {
            "wss://ws-fapi.binance.com/ws-fapi/v1".to_string()
        } else {
            "wss://testnet.binancefuture.com/ws-fapi/v1".to_string()
        };
        
        let (ws_stream, _) = connect_async(&ws_url).await.map_err(|e| e.to_string())?;
        info!("🔌 Connected to Binance WS API for orders at {}", ws_url);
        
        let ws_tx = self.ws_tx.clone();
        let pending = self.pending_requests.clone();
        
        tokio::spawn(async move {
            let mut current_stream = ws_stream;
            
            loop {
                let (mut write, mut read) = current_stream.split();
                let (tx, mut rx) = futures::channel::mpsc::unbounded::<String>();
                
                *ws_tx.lock().await = Some(tx);
                let pending_inner = pending.clone();
                
                // Writer task
                let writer_handle = tokio::spawn(async move {
                    while let Some(msg) = rx.next().await {
                        if let Err(e) = write.send(Message::Text(msg)).await {
                            error!("WS API Send error: {}", e);
                            break;
                        }
                    }
                });
                
                // Reader task inline (wait for disconnect)
                while let Some(msg) = read.next().await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            if let Ok(json) = serde_json::from_str::<Value>(&text) {
                                if let Some(id) = json.get("id").and_then(|v| v.as_str()) {
                                    if let Some((_, sender)) = pending_inner.remove(id) {
                                        let _ = sender.send(json);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            error!("WS API Read error: {}", e);
                            break;
                        }
                        _ => {}
                    }
                }
                
                warn!("WS API connection dropped. Reconnecting in 3 seconds...");
                *ws_tx.lock().await = None;
                writer_handle.abort();
                pending.clear();
                
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                
                loop {
                    match connect_async(&ws_url).await {
                        Ok((new_stream, _)) => {
                            info!("🔌 Reconnected to Binance WS API");
                            current_stream = new_stream;
                            break;
                        }
                        Err(e) => {
                            error!("WS API Reconnect failed: {}. Retrying in 5s...", e);
                            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        }
                    }
                }
            }
        });
        
        Ok(())
    }
    
    pub async fn request(&self, method: &str, mut params: BTreeMap<String, String>) -> Result<Value, String> {
        let id = format!("{}", self.req_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        
        // Inject API Key and Timestamp
        params.insert("apiKey".to_string(), self.rest.get_api_key());
        let ts = chrono::Utc::now().timestamp_millis().to_string();
        params.insert("timestamp".to_string(), ts);
        
        // Sign the payload (The query string format of the params is signed)
        let query_string = self.rest.sign_request(params.clone());
        let signature = query_string.split("&signature=").nth(1).unwrap_or("").to_string();
        
        params.insert("signature".to_string(), signature);
        
        let payload = json!({
            "id": id,
            "method": method,
            "params": params
        });
        
        let payload_str = payload.to_string();
        
        let (tx, rx) = oneshot::channel();
        self.pending_requests.insert(id.clone(), tx);
        
        let mut tx_guard = self.ws_tx.lock().await;
        if let Some(ws_tx) = tx_guard.as_mut() {
            if ws_tx.unbounded_send(payload_str).is_err() {
                self.pending_requests.remove(&id);
                return Err("Failed to send to WS API channel".to_string());
            }
        } else {
            self.pending_requests.remove(&id);
            return Err("WS API not connected".to_string());
        }
        
        match tokio::time::timeout(std::time::Duration::from_secs(5), rx).await {
            Ok(Ok(resp)) => {
                if let Some(err) = resp.get("error") {
                    error!("WS API Error: {}", err);
                    return Err(format!("API Error: {}", err));
                }
                if let Some(result) = resp.get("result") {
                    Ok(result.clone())
                } else {
                    Ok(resp)
                }
            }
            _ => {
                self.pending_requests.remove(&id);
                Err("WS API request timed out".to_string())
            }
        }
    }
    
    pub async fn place_order(&self, symbol: &str, side: &str, order_type: &str, qty: f64, price: Option<f64>, reduce_only: bool) -> Result<Value, String> {
        let mut params = BTreeMap::new();
        params.insert("symbol".to_string(), symbol.to_string());
        params.insert("side".to_string(), side.to_string());
        params.insert("type".to_string(), order_type.to_string());
        
        let clean_qty = (qty * 100000.0).round() / 100000.0;
        params.insert("quantity".to_string(), clean_qty.to_string());
        
        if let Some(p) = price {
            let clean_price_str = if let Some(tick_size) = self.tick_sizes.get(symbol) {
                let ts = *tick_size;
                let decimals = if ts > 0.0 { (-ts.log10()).max(0.0).round() as usize } else { 5 };
                // 解决 Rust 浮点精度可能导致 .999999 的问题，通过格式化直接截断
                let rounded = (p / ts).round() * ts;
                format!("{:.*}", decimals, rounded)
            } else {
                let clean_price = (p * 100000.0).round() / 100000.0;
                clean_price.to_string()
            };
            params.insert("price".to_string(), clean_price_str);
            if order_type == "LIMIT" {
                params.insert("timeInForce".to_string(), "GTX".to_string()); // Post-Only
            } else if order_type == "LIMIT_IOC" {
                params.insert("timeInForce".to_string(), "IOC".to_string()); // Immediate Or Cancel
                params.insert("type".to_string(), "LIMIT".to_string());
            }
        }
        
        if reduce_only {
            params.insert("reduceOnly".to_string(), "true".to_string());
        }
        
        self.request("order.place", params).await
    }
    
    pub async fn cancel_order(&self, symbol: &str, order_id: u64) -> Result<Value, String> {
        let mut params = BTreeMap::new();
        params.insert("symbol".to_string(), symbol.to_string());
        params.insert("orderId".to_string(), order_id.to_string());
        
        self.request("order.cancel", params).await
    }
}
