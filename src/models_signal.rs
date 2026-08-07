#![allow(dead_code)]

#[derive(Debug, Clone)]
pub struct Signal {
    pub msg_id: String,
    pub timestamp: u64,
    pub symbol: String,
    pub price: f64,
    pub atr_24h: f64,
    pub is_long: bool,
    pub raw_score: f64,
    pub prob: f64,
    pub tier: Option<String>,
    pub is_new_symbol: Option<bool>,
    pub regime: Option<String>,
}
