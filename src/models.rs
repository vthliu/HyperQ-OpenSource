#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Debug, Clone, Default)]
pub struct Kline {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub quote_asset_volume: f64,
    pub number_of_trades: u64,
    pub taker_buy_quote: f64,
}

#[derive(Debug)]
pub struct MockPosition {
    pub symbol: String,
    pub entry_price: f64,
    pub position_amt: f64,
    pub leverage: u8,
    pub entry_time: u64,
    pub tier: Option<String>,
    pub regime: Option<String>,
    pub unrealized_roe: f64,
    pub ema_roe: f64,
    pub max_favorable_excursion: f64,
    pub max_adverse_excursion: f64,
    pub time_barrier_notified: AtomicBool,
    pub is_closing: AtomicBool,
    pub closing_started_at: AtomicU64, // 0 = not closing; nonzero = ms timestamp when close was locked
    pub alert_flag: bool,
    pub peak_ema_roe: f64,             // Tracks highest EMA ROE for trailing stop
    pub atr_24h: f64,                  // Tracks the asset volatility at entry time
}

impl MockPosition {
    pub fn new(
        symbol: String,
        entry_price: f64,
        position_amt: f64,
        leverage: u8,
        entry_time: u64,
        tier: Option<String>,
        regime: Option<String>,
        atr_24h: f64,
    ) -> Self {
        Self {
            symbol,
            entry_price,
            position_amt,
            leverage,
            entry_time,
            tier,
            regime,
            unrealized_roe: 0.0,
            ema_roe: 0.0,
            max_favorable_excursion: 0.0,
            max_adverse_excursion: 0.0,
            time_barrier_notified: AtomicBool::new(false),
            is_closing: AtomicBool::new(false),
            closing_started_at: AtomicU64::new(0),
            alert_flag: false,
            peak_ema_roe: 0.0,
            atr_24h,
        }
    }

    pub fn try_lock_for_close(&self) -> bool {
        if self.is_closing
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            // Bug 5 fix: record timestamp so Sentinel can detect close timeout
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            self.closing_started_at.store(now_ms, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    pub fn unlock_close(&self) {
        self.closing_started_at.store(0, Ordering::SeqCst);
        self.is_closing.store(false, Ordering::SeqCst);
    }
}

