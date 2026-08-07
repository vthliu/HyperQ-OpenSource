#![allow(dead_code)]

use reqwest::{Client, StatusCode, header};
use std::sync::Arc;
use tokio::sync::Mutex;
use std::time::{Instant, Duration};
use tracing::{error, warn};
use hmac::{Hmac, Mac, KeyInit};
use sha2::Sha256;
use std::collections::BTreeMap;
use serde_json::Value;

use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::RsaPrivateKey;
use rsa::signature::{Signer, SignatureEncoding};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use urlencoding::encode as url_encode;

type HmacSha256 = Hmac<Sha256>;

pub struct RestApi {
    client: Client,
    base_url: String,
    api_key: String,
    api_secret: String,
    rsa_key: Option<SigningKey<rsa::sha2::Sha256>>,
    rate_limiter: Arc<Mutex<RateLimiter>>,
    global_pause_until: Arc<Mutex<Option<Instant>>>,
}

pub struct RateLimiter {
    capacity: usize,
    tokens: f64,
    last_refill: Instant,
    refill_rate_per_sec: f64,
}

impl RateLimiter {
    pub fn new(capacity: usize, refill_per_min: usize) -> Self {
        Self {
            capacity,
            tokens: capacity as f64,
            last_refill: Instant::now(),
            refill_rate_per_sec: refill_per_min as f64 / 60.0,
        }
    }

    pub fn consume(&mut self, weight: usize) -> Result<(), ()> {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        let add = elapsed * self.refill_rate_per_sec;
        
        if add > 0.0 {
            self.tokens = (self.tokens + add).min(self.capacity as f64);
            self.last_refill = now;
        }

        if self.tokens >= weight as f64 {
            self.tokens -= weight as f64;
            Ok(())
        } else {
            Err(())
        }
    }
}

impl RestApi {
    pub fn new(env: &str, api_key: String, api_secret: String) -> Self {
        let base_url = if env == "mainnet" {
            "https://fapi.binance.com".to_string()
        } else {
            "https://testnet.binancefuture.com".to_string()
        };
        
        let mut headers = header::HeaderMap::new();
        headers.insert("X-MBX-APIKEY", header::HeaderValue::from_str(&api_key).unwrap());

        let mut rsa_key = None;
        if api_secret.ends_with(".pem") && std::path::Path::new(&api_secret).exists() {
            match RsaPrivateKey::read_pkcs8_pem_file(&api_secret) {
                Ok(private_key) => {
                    rsa_key = Some(SigningKey::<rsa::sha2::Sha256>::new(private_key));
                    tracing::info!("🔑 Loaded RSA private key for API authentication.");
                }
                Err(e) => {
                    tracing::error!("💀 Failed to load RSA private key from {}: {}", api_secret, e);
                }
            }
        } else {
            tracing::info!("🔑 Using HMAC API authentication.");
        }

        Self {
            client: Client::builder().default_headers(headers).build().unwrap(),
            base_url,
            api_key,
            api_secret,
            rsa_key,
            rate_limiter: Arc::new(Mutex::new(RateLimiter::new(1200, 1200))),
            global_pause_until: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn check_limit_and_pause(&self, weight: usize, is_close_order: bool) -> Result<(), &'static str> {
        let mut pause_guard = self.global_pause_until.lock().await;
        if let Some(pause_until) = *pause_guard {
            if Instant::now() < pause_until {
                if !is_close_order {
                    return Err("API is paused due to 429 Rate Limit.");
                } else {
                    warn!("API is globally paused, but allowing close order to proceed.");
                }
            } else {
                *pause_guard = None;
            }
        }
        
        let mut limiter = self.rate_limiter.lock().await;
        if limiter.consume(weight).is_err() {
            error!("💀 [FATAL] Token bucket exhausted locally.");
            return Err("Rate limit exceeded locally");
        }
        Ok(())
    }

    pub async fn handle_response(&self, status: StatusCode) -> Result<(), &'static str> {
        if status == StatusCode::TOO_MANY_REQUESTS {
            error!("💀 [FATAL] API Rate Limit Exceeded (429 Received)");
            let mut pause = self.global_pause_until.lock().await;
            *pause = Some(Instant::now() + Duration::from_secs(60));
            return Err("429 Too Many Requests");
        } else if status == StatusCode::UNAUTHORIZED {
            error!("💀 [FATAL] API Key Permission Denied");
            std::process::exit(1);
        } else if !status.is_success() {
            error!("⚠️ [API ERROR] Received HTTP {}", status);
            return Err("HTTP Request Failed");
        }
        Ok(())
    }

    pub fn get_api_key(&self) -> String {
        self.api_key.clone()
    }

    pub async fn get_exchange_info(&self) -> Result<dashmap::DashMap<String, f64>, String> {
        let url = format!("{}/fapi/v1/exchangeInfo", self.base_url);
        let resp = self.client.get(&url).send().await.map_err(|e| e.to_string())?;
        let data: Value = resp.json().await.map_err(|e| e.to_string())?;
        let map = dashmap::DashMap::new();
        
        if let Some(symbols) = data.get("symbols").and_then(|v| v.as_array()) {
            for symbol_info in symbols {
                if let Some(symbol) = symbol_info.get("symbol").and_then(|v| v.as_str()) {
                    if let Some(filters) = symbol_info.get("filters").and_then(|v| v.as_array()) {
                        for filter in filters {
                            if filter.get("filterType").and_then(|v| v.as_str()) == Some("PRICE_FILTER") {
                                if let Some(tick_size) = filter.get("tickSize").and_then(|v| v.as_str()).and_then(|s| s.parse::<f64>().ok()) {
                                    map.insert(symbol.to_string(), tick_size);
                                }
                            }
                        }
                    }
                }
            }
            Ok(map)
        } else {
            Err("Failed to parse exchangeInfo symbols array".to_string())
        }
    }

    pub async fn get_server_time(&self) -> Result<u64, String> {
        let url = format!("{}/fapi/v1/time", self.base_url);
        let resp = self.client.get(&url).send().await.map_err(|e| e.to_string())?;
        let data: Value = resp.json().await.map_err(|e| e.to_string())?;
        if let Some(t) = data.get("serverTime").and_then(|v| v.as_u64()) {
            Ok(t)
        } else {
            Err("Failed to parse serverTime".to_string())
        }
    }

    pub async fn get_24hr_tickers(&self) -> Result<Vec<(String, f64, f64)>, String> {
        let url = format!("{}/fapi/v1/ticker/24hr", self.base_url);
        let resp = self.client.get(&url).send().await.map_err(|e| e.to_string())?;
        let data: Value = resp.json().await.map_err(|e| e.to_string())?;
        let mut tickers = Vec::new();
        if let Some(arr) = data.as_array() {
            for item in arr {
                if let Some(symbol) = item.get("symbol").and_then(|v| v.as_str()) {
                    if !symbol.ends_with("USDT") { continue; } // Only USDT pairs
                    let quote_vol = item.get("quoteVolume").and_then(|v| v.as_str()).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
                    let price_change_pct = item.get("priceChangePercent").and_then(|v| v.as_str()).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
                    tickers.push((symbol.to_string(), quote_vol, price_change_pct));
                }
            }
            Ok(tickers)
        } else {
            Err("Failed to parse 24hr tickers array".to_string())
        }
    }

    /// Get mark price for a single symbol. No auth required. Weight=1.
    pub async fn get_mark_price(&self, symbol: &str) -> Result<f64, String> {
        let url = format!("{}/fapi/v1/premiumIndex?symbol={}", self.base_url, symbol);
        let resp = self.client.get(&url).send().await.map_err(|e| e.to_string())?;
        let data: Value = resp.json().await.map_err(|e| e.to_string())?;
        data.get("markPrice")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .ok_or_else(|| format!("No markPrice for {}", symbol))
    }

    pub async fn get_book_ticker(&self, symbol: &str) -> Result<(f64, f64), String> {
        let url = format!("{}/fapi/v1/ticker/bookTicker?symbol={}", self.base_url, symbol);
        let resp = self.client.get(&url).send().await.map_err(|e| e.to_string())?;
        let data: Value = resp.json().await.map_err(|e| e.to_string())?;
        let bid = data.get("bidPrice").and_then(|v| v.as_str()).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        let ask = data.get("askPrice").and_then(|v| v.as_str()).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        Ok((bid, ask))
    }

    fn current_time_ms() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
    }

    pub fn sign_request(&self, mut params: BTreeMap<String, String>) -> String {
        params.insert("timestamp".to_string(), Self::current_time_ms().to_string());
        
        let mut query = String::new();
        for (k, v) in &params {
            if !query.is_empty() {
                query.push('&');
            }
            query.push_str(k);
            query.push('=');
            query.push_str(v);
        }
        
        let signature = if let Some(ref rsa) = self.rsa_key {
            let sig = rsa.sign(query.as_bytes());
            let b64 = BASE64_STANDARD.encode(sig.to_bytes());
            url_encode(&b64).into_owned()
        } else {
            let mut mac = HmacSha256::new_from_slice(self.api_secret.as_bytes()).unwrap();
            mac.update(query.as_bytes());
            hex::encode(mac.finalize().into_bytes())
        };
        
        tracing::debug!("🔗 Pre-sign Query: {}", query);
        tracing::debug!("🔗 Signature: {}", signature);
        
        format!("{}&signature={}", query, signature)
    }

    pub async fn get_position_risk(&self) -> Result<Vec<Value>, String> {
        self.check_limit_and_pause(5, false).await.map_err(|e| e.to_string())?;
        
        let query_string = self.sign_request(BTreeMap::new());
        let url = format!("{}/fapi/v2/positionRisk?{}", self.base_url, query_string);
        
        let resp = self.client.get(&url).send().await.map_err(|e| e.to_string())?;
        self.handle_response(resp.status()).await.map_err(|e| e.to_string())?;
        
        let data: Vec<Value> = resp.json().await.map_err(|e| e.to_string())?;
        Ok(data)
    }

    pub async fn get_usdt_balance(&self) -> Result<f64, String> {
        self.check_limit_and_pause(5, false).await.map_err(|e| e.to_string())?;
        
        let query_string = self.sign_request(BTreeMap::new());
        let url = format!("{}/fapi/v2/balance?{}", self.base_url, query_string);
        
        let resp = self.client.get(&url).send().await.map_err(|e| e.to_string())?;
        self.handle_response(resp.status()).await.map_err(|e| e.to_string())?;
        
        let data: Vec<Value> = resp.json().await.map_err(|e| e.to_string())?;
        for asset in data {
            if asset.get("asset").and_then(|v| v.as_str()) == Some("USDT") {
                // 使用账户总余额 (balance) 而不是可用余额 (availableBalance)
                // 这样无论当前开了多少单，单笔仓位的计算基准始终一致，不会因为保证金被占用而导致后续开仓急剧缩小
                if let Some(balance_str) = asset.get("balance").and_then(|v| v.as_str()) {
                    return Ok(balance_str.parse::<f64>().unwrap_or(0.0));
                }
            }
        }
        Ok(0.0)
    }

    pub async fn get_open_orders(&self) -> Result<Vec<Value>, String> {
        self.check_limit_and_pause(3, false).await.map_err(|e| e.to_string())?;
        
        let query_string = self.sign_request(BTreeMap::new());
        let url = format!("{}/fapi/v1/openOrders?{}", self.base_url, query_string);
        
        let resp = self.client.get(&url).send().await.map_err(|e| e.to_string())?;
        self.handle_response(resp.status()).await.map_err(|e| e.to_string())?;
        
        let data: Vec<Value> = resp.json().await.map_err(|e| e.to_string())?;
        Ok(data)
    }

    pub async fn cancel_all_orders(&self, symbol: &str) -> Result<(), String> {
        self.check_limit_and_pause(1, true).await.map_err(|e| e.to_string())?;
        
        let mut params = BTreeMap::new();
        params.insert("symbol".to_string(), symbol.to_string());
        let query_string = self.sign_request(params);
        
        let url = format!("{}/fapi/v1/allOpenOrders?{}", self.base_url, query_string);
        let resp = self.client.delete(&url).send().await.map_err(|e| e.to_string())?;
        self.handle_response(resp.status()).await.map_err(|e| e.to_string())?;
        
        Ok(())
    }

    pub async fn cancel_order(&self, symbol: &str, order_id: u64) -> Result<(), String> {
        self.check_limit_and_pause(1, false).await.map_err(|e| e.to_string())?;
        
        let mut params = BTreeMap::new();
        params.insert("symbol".to_string(), symbol.to_string());
        params.insert("orderId".to_string(), order_id.to_string());
        let query_string = self.sign_request(params);
        
        let url = format!("{}/fapi/v1/order?{}", self.base_url, query_string);
        let resp = self.client.delete(&url).send().await.map_err(|e| e.to_string())?;
        self.handle_response(resp.status()).await.map_err(|e| e.to_string())?;
        
        Ok(())
    }

    pub async fn set_margin_type(&self, symbol: &str, margin_type: &str) -> Result<(), String> {
        self.check_limit_and_pause(1, false).await.map_err(|e| e.to_string())?;
        
        let mut params = BTreeMap::new();
        params.insert("symbol".to_string(), symbol.to_string());
        params.insert("marginType".to_string(), margin_type.to_string());
        let query_string = self.sign_request(params);
        
        let url = format!("{}/fapi/v1/marginType?{}", self.base_url, query_string);
        let resp = self.client.post(&url).send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let body_bytes = resp.bytes().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            let body_str = String::from_utf8_lossy(&body_bytes);
            if body_str.contains("-4046") {
                // Benign error: margin type is already what we want
                return Ok(());
            }
            error!("⚠️ [API ERROR] HTTP {} on set_margin_type: {}", status, body_str);
            return Err("HTTP Request Failed".to_string());
        }
        
        Ok(())
    }

    pub async fn set_leverage(&self, symbol: &str, leverage: u8) -> Result<(), String> {
        self.check_limit_and_pause(1, false).await.map_err(|e| e.to_string())?;
        
        let mut params = BTreeMap::new();
        params.insert("symbol".to_string(), symbol.to_string());
        params.insert("leverage".to_string(), leverage.to_string());
        let query_string = self.sign_request(params);
        
        let url = format!("{}/fapi/v1/leverage?{}", self.base_url, query_string);
        let resp = self.client.post(&url).send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let body_bytes = resp.bytes().await.map_err(|e| e.to_string())?;
        
        if !status.is_success() {
            let body_str = String::from_utf8_lossy(&body_bytes);
            error!("⚠️ [API ERROR] HTTP {} on set_leverage: {}", status, body_str);
            return Err("HTTP Request Failed".to_string());
        }
        
        Ok(())
    }

    pub async fn place_order(&self, symbol: &str, side: &str, order_type: &str, qty: f64, price: Option<f64>, reduce_only: bool) -> Result<serde_json::Value, String> {
        self.check_limit_and_pause(1, reduce_only).await.map_err(|e| e.to_string())?;
        
        let mut params = BTreeMap::new();
        params.insert("symbol".to_string(), symbol.to_string());
        params.insert("side".to_string(), side.to_string());
        params.insert("type".to_string(), order_type.to_string());
        
        let clean_qty = (qty * 100000.0).round() / 100000.0;
        params.insert("quantity".to_string(), clean_qty.to_string());
        
        if let Some(p) = price {
            let clean_price = (p * 100000.0).round() / 100000.0;
            params.insert("price".to_string(), clean_price.to_string());
            if order_type == "LIMIT" {
                params.insert("timeInForce".to_string(), "GTX".to_string());
            } else {
                params.insert("timeInForce".to_string(), "GTC".to_string());
            }
        }
        
        if reduce_only {
            params.insert("reduceOnly".to_string(), "true".to_string());
        }
        
        let query_string = self.sign_request(params);
        let url = format!("{}/fapi/v1/order?{}", self.base_url, query_string);
        let resp = self.client.post(&url).send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let body_bytes = resp.bytes().await.map_err(|e| e.to_string())?;
        
        if !status.is_success() {
            let body_str = String::from_utf8_lossy(&body_bytes);
            error!("⚠️ [API ERROR] HTTP {} on place_order: {}", status, body_str);
            return Err("HTTP Request Failed".to_string());
        }
        
        let data: Value = serde_json::from_slice(&body_bytes).map_err(|e| e.to_string())?;
        Ok(data)
    }

    pub async fn get_order(&self, symbol: &str, order_id: u64) -> Result<serde_json::Value, String> {
        self.check_limit_and_pause(1, false).await.map_err(|e| e.to_string())?;
        
        let mut params = BTreeMap::new();
        params.insert("symbol".to_string(), symbol.to_string());
        params.insert("orderId".to_string(), order_id.to_string());
        
        let query_string = self.sign_request(params);
        let url = format!("{}/fapi/v1/order?{}", self.base_url, query_string);
        let resp = self.client.get(&url).send().await.map_err(|e| e.to_string())?;
        self.handle_response(resp.status()).await.map_err(|e| e.to_string())?;
        
        let data: Value = resp.json().await.map_err(|e| e.to_string())?;
        Ok(data)
    }

    pub async fn get_funding_rates(&self) -> Result<std::collections::HashMap<String, (f64, u64)>, String> {
        self.check_limit_and_pause(1, false).await.map_err(|e| e.to_string())?;
        let url = format!("{}/fapi/v1/premiumIndex", self.base_url);
        let resp = self.client.get(&url).send().await.map_err(|e| e.to_string())?;
        self.handle_response(resp.status()).await.map_err(|e| e.to_string())?;
        
        let data: Value = resp.json().await.map_err(|e| e.to_string())?;
        let mut map = std::collections::HashMap::new();
        
        if let Some(arr) = data.as_array() {
            for item in arr {
                if let (Some(sym), Some(fr_str), Some(nxt)) = (
                    item.get("symbol").and_then(|s| s.as_str()),
                    item.get("lastFundingRate").and_then(|s| s.as_str()),
                    item.get("nextFundingTime").and_then(|n| n.as_u64())
                ) {
                    if let Ok(fr) = fr_str.parse::<f64>() {
                        map.insert(sym.to_string(), (fr, nxt));
                    }
                }
            }
        }
        Ok(map)
    }

    pub async fn get_klines(&self, symbol: &str, interval: &str, limit: usize) -> Result<Vec<crate::models::Kline>, String> {
        self.check_limit_and_pause(1, false).await.map_err(|e| e.to_string())?;
        
        let url = format!("{}/fapi/v1/klines?symbol={}&interval={}&limit={}", self.base_url, symbol, interval, limit);
        let resp = self.client.get(&url).send().await.map_err(|e| e.to_string())?;
        self.handle_response(resp.status()).await.map_err(|e| e.to_string())?;
        
        let data: Value = resp.json().await.map_err(|e| e.to_string())?;
        let mut klines = Vec::new();
        
        if let Some(arr) = data.as_array() {
            for item in arr {
                if let Some(k) = item.as_array() {
                    if k.len() >= 11 {
                        let kline = crate::models::Kline {
                            open: k[1].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                            high: k[2].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                            low: k[3].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                            close: k[4].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                            volume: k[5].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                            quote_asset_volume: k[7].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                            number_of_trades: k[8].as_u64().unwrap_or(0),
                            taker_buy_quote: k[10].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                        };
                        klines.push(kline);
                    }
                }
            }
        }
        Ok(klines)
    }

    /// 获取单个合约的当前持仓量 (Open Interest)
    pub async fn get_open_interest(&self, symbol: &str) -> Result<f64, String> {
        self.check_limit_and_pause(1, false).await.map_err(|e| e.to_string())?;
        let url = format!("{}/fapi/v1/openInterest?symbol={}", self.base_url, symbol);
        let resp = self.client.get(&url).send().await.map_err(|e| e.to_string())?;
        self.handle_response(resp.status()).await.map_err(|e| e.to_string())?;
        let data: Value = resp.json().await.map_err(|e| e.to_string())?;
        let oi = data.get("openInterest")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        Ok(oi)
    }
}
