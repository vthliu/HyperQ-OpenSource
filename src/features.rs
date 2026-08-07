/// Feature engineering module - fully replicates ml_engine/features/processor.py
/// including pandas_ta quirks (e.g. CCI operator-precedence bug).
use crate::models::Kline;

pub struct FeatureEngine;

impl FeatureEngine {
    /// Computes the same 47-dimensional feature vector as processor.py:transform().
    /// Feature order must match the XGBoost model's feature_names exactly.
    pub fn compute_features(klines: &[Kline]) -> Vec<f64> {
        let n = klines.len();
        if n < 50 {
            return vec![0.0; 47];
        }

        let closes: Vec<f64> = klines.iter().map(|k| k.close).collect();
        let highs: Vec<f64>  = klines.iter().map(|k| k.high).collect();
        let lows: Vec<f64>   = klines.iter().map(|k| k.low).collect();
        let opens: Vec<f64>  = klines.iter().map(|k| k.open).collect();
        let volumes: Vec<f64> = klines.iter().map(|k| k.volume).collect();
        let qvols: Vec<f64>  = klines.iter().map(|k| k.quote_asset_volume).collect();
        let n_trades: Vec<f64> = klines.iter().map(|k| k.number_of_trades as f64).collect();
        let tbq: Vec<f64>    = klines.iter().map(|k| k.taker_buy_quote).collect();

        // ── 0. SMA ─────────────────────────────────────────────────────────
        let sma5  = sma(&closes, 5);
        let sma20 = sma(&closes, 20);
        let sma50 = sma(&closes, 50);

        // ── 1. EMA ─────────────────────────────────────────────────────────
        // pandas_ta EMA: seed = mean of first `length` values (adjust=False)
        let ema12 = ema_series(&closes, 12);
        let ema26 = ema_series(&closes, 26);

        // ── 2. MACD (12, 26, 9) ────────────────────────────────────────────
        let macd_line   = sub_series(&ema12, &ema26);
        let signal_line = ema_series(&macd_line, 9);
        let histogram   = sub_series(&macd_line, &signal_line);

        let last_macd  = *macd_line.last().unwrap_or(&0.0);
        let last_hist  = *histogram.last().unwrap_or(&0.0);
        let last_sig   = *signal_line.last().unwrap_or(&0.0);

        // ── 3. RSI(14) ─────────────────────────────────────────────────────
        let rsi14 = rsi_series(&closes, 14);
        let last_rsi = *rsi14.last().unwrap_or(&50.0);

        // ── 4. StochRSI(14, 14, 3, 3) ──────────────────────────────────────
        let (stochrsi_k, stochrsi_d) = stochrsi(&closes, 14, 14, 3, 3);

        // ── 5. Bollinger Bands (20, 2.0) ───────────────────────────────────
        let (bbl, bbm, bbu, bbb, bbp) = bbands(&closes, 20, 2.0);

        // ── 6. ATR(14) — pandas_ta uses RMA (Wilder's MA) ─────────────────
        let atr14 = atr_series(&highs, &lows, &closes, 14);
        let last_atr = *atr14.last().unwrap_or(&0.0);

        // ── 7. ADX(14), ADXR, DMP, DMN ────────────────────────────────────
        let (adx, adxr, dmp, dmn) = adx_series(&highs, &lows, &closes, 14);

        // ── 8. CCI(20, 0.015) — replicates pandas_ta operator-precedence bug ─
        let last_cci = cci_pandas_ta_buggy(&highs, &lows, &closes, 20, 0.015);

        // ── 9. OBV ─────────────────────────────────────────────────────────
        let last_obv = obv(&closes, &volumes);

        // ── 10. MFI(14) ────────────────────────────────────────────────────
        let last_mfi = mfi(&highs, &lows, &closes, &volumes, 14);

        // ── 11. ROC(10) ────────────────────────────────────────────────────
        let last_roc = roc(&closes, 10);

        // ── 12. WILLR(14) ──────────────────────────────────────────────────
        let last_willr = willr(&highs, &lows, &closes, 14);

        // ── 13. VWAP_D ─────────────────────────────────────────────────────
        let last_vwap = vwap_daily(&highs, &lows, &closes, &volumes);

        // ── 14. CMF(20) ────────────────────────────────────────────────────
        let last_cmf = cmf(&highs, &lows, &closes, &volumes, 20);

        // ── Custom features ────────────────────────────────────────────────
        let curr_close = closes[n - 1];
        let curr_high  = highs[n - 1];
        let curr_low   = lows[n - 1];
        let curr_open  = opens[n - 1];
        let curr_qvol  = qvols[n - 1];
        let curr_trades = n_trades[n - 1];
        let curr_tbq   = tbq[n - 1];

        let dist_sma_20 = if sma20 != 0.0 { (curr_close - sma20) / sma20 } else { 0.0 };
        let dist_ema_26 = if ema26.last().copied().unwrap_or(0.0) != 0.0 {
            (curr_close - ema26.last().copied().unwrap_or(0.0)) / ema26.last().copied().unwrap_or(0.0)
        } else { 0.0 };

        // Volume surge ratio
        let vol_sma20: f64 = volumes[n.saturating_sub(20)..n].iter().sum::<f64>() / 20.0;
        let volume_surge_ratio = volumes[n - 1] / (vol_sma20 + 1e-9);

        // Pump exhaustion index
        let low_12_min = lows[n.saturating_sub(12)..n].iter().copied().fold(f64::INFINITY, f64::min);
        let pump_exhaustion_index = (curr_close - low_12_min) / (low_12_min + 1e-9);

        // BBand overextension — uses the last BBU
        let bband_overextension = if bbu != 0.0 { (curr_close - bbu) / (bbu + 1e-9) } else { 0.0 };

        // Wick ratios
        let candle_range  = curr_high - curr_low + 1e-9;
        let body_top    = curr_open.max(curr_close);
        let body_bottom = curr_open.min(curr_close);
        let upper_wick_ratio = (curr_high - body_top) / candle_range;
        let lower_wick_ratio = (body_bottom - curr_low) / candle_range;

        // Price tick velocity (5-period)
        let close_5ago = closes[n.saturating_sub(6)];
        let price_tick_velocity = (curr_close - close_5ago) / (close_5ago + 1e-9) / 5.0;

        // Trade imbalance
        let taker_sell = curr_qvol - curr_tbq;
        let trade_imbalance = (curr_tbq - taker_sell) / (curr_qvol + 1e-9);

        // Avg trade size & whale alert score
        let avg_trade_size = curr_qvol / (curr_trades + 1e-9);
        // whale_alert_score = avg_trade_size / EMA20(avg_trade_size)
        let avg_ts_series: Vec<f64> = (0..n).map(|i| qvols[i] / (n_trades[i] + 1e-9)).collect();
        let avg_ts_ema20 = ema_series(&avg_ts_series, 20);
        let last_ema20_ats = avg_ts_ema20.last().copied().unwrap_or(1.0);
        let whale_alert_score = avg_trade_size / (last_ema20_ats + 1e-9);

        // Liquidity score
        let liquidity_score = (curr_qvol + 1.0).ln();

        // Returns and lags
        let returns      = if n >= 2 { (closes[n-1] - closes[n-2]) / closes[n-2] } else { 0.0 };
        let returns_lag1 = if n >= 3 { (closes[n-2] - closes[n-3]) / closes[n-3] } else { 0.0 };
        let returns_lag2 = if n >= 4 { (closes[n-3] - closes[n-4]) / closes[n-4] } else { 0.0 };
        let returns_lag3 = if n >= 5 { (closes[n-4] - closes[n-5]) / closes[n-5] } else { 0.0 };
        let returns_lag5 = if n >= 7 { (closes[n-6] - closes[n-7]) / closes[n-7] } else { 0.0 };

        let high_low_ratio      = curr_high / (curr_low + 1e-9);
        let high_low_ratio_lag1 = if n >= 2 { highs[n-2] / (lows[n-2] + 1e-9) } else { 0.0 };

        vec![
            sma5, sma20, sma50,
            ema12.last().copied().unwrap_or(0.0),
            ema26.last().copied().unwrap_or(0.0),
            last_macd, last_hist, last_sig,
            last_rsi,
            stochrsi_k, stochrsi_d,
            bbl, bbm, bbu, bbb, bbp,
            last_atr,
            adx, adxr, dmp, dmn,
            last_cci,
            last_obv,
            last_mfi,
            last_roc,
            last_willr,
            last_vwap,
            last_cmf,
            dist_sma_20, dist_ema_26,
            volume_surge_ratio,
            pump_exhaustion_index,
            bband_overextension,
            upper_wick_ratio, lower_wick_ratio,
            price_tick_velocity,
            trade_imbalance,
            avg_trade_size,
            whale_alert_score,
            liquidity_score,
            returns, returns_lag1, returns_lag2, returns_lag3, returns_lag5,
            high_low_ratio, high_low_ratio_lag1,
        ]
    }
}

// ── Helper: Simple Moving Average (last value) ───────────────────────────────
fn sma(data: &[f64], period: usize) -> f64 {
    let n = data.len();
    if n < period { return 0.0; }
    data[n - period..].iter().sum::<f64>() / period as f64
}

// ── Helper: EMA series — seed = SMA of first `period` values (pandas_ta default) ──
pub fn ema_series(data: &[f64], period: usize) -> Vec<f64> {
    let n = data.len();
    let mut out = vec![0.0; n];
    if n < period { return out; }

    let k = 2.0 / (period as f64 + 1.0);
    // Seed: SMA of first `period` elements
    let seed: f64 = data[..period].iter().sum::<f64>() / period as f64;
    out[period - 1] = seed;

    for i in period..n {
        out[i] = data[i] * k + out[i - 1] * (1.0 - k);
    }
    out
}

// ── Helper: Subtract two series element-wise ─────────────────────────────────
fn sub_series(a: &[f64], b: &[f64]) -> Vec<f64> {
    a.iter().zip(b.iter()).map(|(x, y)| x - y).collect()
}

// ── RSI series (Wilder's smoothing = RMA) ────────────────────────────────────
fn rsi_series(data: &[f64], period: usize) -> Vec<f64> {
    let n = data.len();
    let mut out = vec![50.0; n];
    if n <= period { return out; }

    let mut gains = vec![0.0; n];
    let mut losses = vec![0.0; n];
    for i in 1..n {
        let diff = data[i] - data[i - 1];
        gains[i]  = diff.max(0.0);
        losses[i] = (-diff).max(0.0);
    }

    // First avg: SMA
    let avg_gain0: f64 = gains[1..=period].iter().sum::<f64>() / period as f64;
    let avg_loss0: f64 = losses[1..=period].iter().sum::<f64>() / period as f64;
    let mut avg_gain = avg_gain0;
    let mut avg_loss = avg_loss0;
    out[period] = if avg_loss == 0.0 { 100.0 } else { 100.0 - 100.0 / (1.0 + avg_gain / avg_loss) };

    for i in (period + 1)..n {
        avg_gain = (avg_gain * (period as f64 - 1.0) + gains[i]) / period as f64;
        avg_loss = (avg_loss * (period as f64 - 1.0) + losses[i]) / period as f64;
        out[i] = if avg_loss == 0.0 { 100.0 } else { 100.0 - 100.0 / (1.0 + avg_gain / avg_loss) };
    }
    out
}

// ── StochRSI(rsi_len=14, stoch_len=14, k=3, d=3) ────────────────────────────
fn stochrsi(closes: &[f64], rsi_len: usize, stoch_len: usize, k_smooth: usize, d_smooth: usize) -> (f64, f64) {
    let rsi = rsi_series(closes, rsi_len);
    let n = rsi.len();
    if n < stoch_len { return (50.0, 50.0); }

    // Stochastic of RSI
    let mut stoch_rsi = vec![0.0; n];
    for i in stoch_len - 1..n {
        let window = &rsi[i + 1 - stoch_len..=i];
        let lo = window.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = window.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        stoch_rsi[i] = if hi - lo < 1e-12 { 0.0 } else { (rsi[i] - lo) / (hi - lo) * 100.0 };
    }

    // SMA smoothing for %K
    let stoch_k: Vec<f64> = (0..n).map(|i| {
        if i + 1 < k_smooth { return 0.0; }
        let start = i + 1 - k_smooth;
        stoch_rsi[start..=i].iter().sum::<f64>() / k_smooth as f64
    }).collect();

    // SMA smoothing for %D
    let last_k = *stoch_k.last().unwrap_or(&0.0);
    let last_d = if n >= d_smooth {
        stoch_k[n - d_smooth..].iter().sum::<f64>() / d_smooth as f64
    } else { 0.0 };

    (last_k, last_d)
}

// ── Bollinger Bands (SMA-based, std=population std) ──────────────────────────
fn bbands(data: &[f64], period: usize, std_mult: f64) -> (f64, f64, f64, f64, f64) {
    let n = data.len();
    if n < period { return (0.0, 0.0, 0.0, 0.0, 0.0); }
    let window = &data[n - period..];
    let mean = window.iter().sum::<f64>() / period as f64;
    // pandas_ta uses ddof=1 (sample std)
    let variance: f64 = window.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (period as f64 - 1.0);
    let std = variance.sqrt();
    let upper = mean + std_mult * std;
    let lower = mean - std_mult * std;
    // BBB = (upper - lower) / mean * 100
    let bbb = if mean != 0.0 { (upper - lower) / mean * 100.0 } else { 0.0 };
    // BBP = (close - lower) / (upper - lower)
    let last_close = data[n - 1];
    let bbp = if upper - lower != 0.0 { (last_close - lower) / (upper - lower) } else { 0.0 };
    (lower, mean, upper, bbb, bbp)
}

// ── ATR (Wilder's RMA) ───────────────────────────────────────────────────────
pub fn atr_series(highs: &[f64], lows: &[f64], closes: &[f64], period: usize) -> Vec<f64> {
    let n = highs.len();
    let mut out = vec![0.0; n];
    if n <= period { return out; }

    let mut tr_vals: Vec<f64> = vec![0.0; n];
    tr_vals[0] = highs[0] - lows[0];
    for i in 1..n {
        let hl = highs[i] - lows[i];
        let hpc = (highs[i] - closes[i - 1]).abs();
        let lpc = (lows[i] - closes[i - 1]).abs();
        tr_vals[i] = hl.max(hpc).max(lpc);
    }

    // Seed: simple mean of first `period` TRs
    let seed: f64 = tr_vals[1..=period].iter().sum::<f64>() / period as f64;
    out[period] = seed;
    for i in (period + 1)..n {
        out[i] = (out[i - 1] * (period as f64 - 1.0) + tr_vals[i]) / period as f64;
    }
    out
}

// ── ADX, ADXR, DMP, DMN ──────────────────────────────────────────────────────
fn adx_series(highs: &[f64], lows: &[f64], closes: &[f64], period: usize) -> (f64, f64, f64, f64) {
    let n = highs.len();
    if n <= period * 2 { return (20.0, 20.0, 20.0, 20.0); }

    let atr = atr_series(highs, lows, closes, period);

    // Directional Movement
    let mut dm_plus  = vec![0.0; n];
    let mut dm_minus = vec![0.0; n];
    for i in 1..n {
        let up   = highs[i] - highs[i - 1];
        let down = lows[i - 1] - lows[i];
        if up > down && up > 0.0 { dm_plus[i] = up; }
        if down > up && down > 0.0 { dm_minus[i] = down; }
    }

    // Smooth with Wilder's RMA
    let di_plus  = rma_div(&dm_plus, &atr, period);
    let di_minus = rma_div(&dm_minus, &atr, period);

    // DX = |DI+ - DI-| / (DI+ + DI-)
    let mut dx_vals = vec![0.0; n];
    for i in 0..n {
        let sum = di_plus[i] + di_minus[i];
        if sum != 0.0 {
            dx_vals[i] = (di_plus[i] - di_minus[i]).abs() / sum * 100.0;
        }
    }

    // ADX = RMA(DX, period)
    let adx_vals = rma_series(&dx_vals, period);

    let last_adx  = *adx_vals.last().unwrap_or(&20.0);
    let last_dmp  = *di_plus.last().unwrap_or(&20.0) * 100.0;
    let last_dmn  = *di_minus.last().unwrap_or(&20.0) * 100.0;

    // ADXR = (ADX + ADX[period/2 bars ago]) / 2  — pandas_ta uses period=2 for ADXR lag
    let adxr_lag = 2usize;
    let last_adxr = if adx_vals.len() > adxr_lag {
        (last_adx + adx_vals[adx_vals.len() - 1 - adxr_lag]) / 2.0
    } else { last_adx };

    (last_adx, last_adxr, last_dmp, last_dmn)
}

fn rma_series(data: &[f64], period: usize) -> Vec<f64> {
    let n = data.len();
    let mut out = vec![0.0; n];
    if n < period { return out; }
    let seed: f64 = data[..period].iter().sum::<f64>() / period as f64;
    out[period - 1] = seed;
    let alpha = 1.0 / period as f64;
    for i in period..n {
        out[i] = alpha * data[i] + (1.0 - alpha) * out[i - 1];
    }
    out
}

fn rma_div(dm: &[f64], atr: &[f64], period: usize) -> Vec<f64> {
    let smoothed = rma_series(dm, period);
    smoothed.iter().zip(atr.iter()).map(|(d, a)| if *a != 0.0 { d / a } else { 0.0 }).collect()
}

// ── CCI — CORRECT formula: (TP - SMA_TP) / (c * MAD) ────────────────────────
/// pandas_ta had an operator-precedence bug; now fixed in processor.py,
/// so models retrained after the fix will expect the correct CCI values.
fn cci_pandas_ta_buggy(highs: &[f64], lows: &[f64], closes: &[f64], period: usize, c: f64) -> f64 {
    let n = highs.len();
    if n < period { return 0.0; }
    let window_h = &highs[n - period..];
    let window_l = &lows[n - period..];
    let window_c = &closes[n - period..];

    let tp_last = (highs[n-1] + lows[n-1] + closes[n-1]) / 3.0;
    let tp_win: Vec<f64> = window_h.iter().zip(window_l.iter()).zip(window_c.iter())
        .map(|((h, l), cv)| (h + l + cv) / 3.0).collect();

    let sma_tp = tp_win.iter().sum::<f64>() / period as f64;
    let mad = tp_win.iter().map(|x| (x - sma_tp).abs()).sum::<f64>() / period as f64;

    if mad < 1e-12 { return 0.0; }
    // CORRECT formula: (TP - SMA_TP) / (c * MAD)
    (tp_last - sma_tp) / (c * mad)
}

// ── OBV ──────────────────────────────────────────────────────────────────────
fn obv(closes: &[f64], volumes: &[f64]) -> f64 {
    let n = closes.len();
    let mut obv = 0.0;
    for i in 1..n {
        if closes[i] > closes[i - 1] { obv += volumes[i]; }
        else if closes[i] < closes[i - 1] { obv -= volumes[i]; }
    }
    obv
}

// ── MFI(14) ──────────────────────────────────────────────────────────────────
fn mfi(highs: &[f64], lows: &[f64], closes: &[f64], volumes: &[f64], period: usize) -> f64 {
    let n = highs.len();
    if n <= period { return 50.0; }
    let mut pos_mf = 0.0;
    let mut neg_mf = 0.0;
    for i in (n - period)..n {
        let tp = (highs[i] + lows[i] + closes[i]) / 3.0;
        let mf = tp * volumes[i];
        if i > 0 {
            let tp_prev = (highs[i-1] + lows[i-1] + closes[i-1]) / 3.0;
            if tp > tp_prev { pos_mf += mf; } else { neg_mf += mf; }
        }
    }
    if neg_mf == 0.0 { return 100.0; }
    100.0 - 100.0 / (1.0 + pos_mf / neg_mf)
}

// ── ROC(10) ──────────────────────────────────────────────────────────────────
fn roc(closes: &[f64], period: usize) -> f64 {
    let n = closes.len();
    if n <= period { return 0.0; }
    let prev = closes[n - 1 - period];
    if prev == 0.0 { return 0.0; }
    (closes[n - 1] - prev) / prev * 100.0
}

// ── WILLR(14) ────────────────────────────────────────────────────────────────
fn willr(highs: &[f64], lows: &[f64], closes: &[f64], period: usize) -> f64 {
    let n = closes.len();
    if n < period { return -50.0; }
    let hh = highs[n - period..].iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let ll = lows[n - period..].iter().copied().fold(f64::INFINITY, f64::min);
    if hh - ll < 1e-12 { return -50.0; }
    (closes[n-1] - hh) / (hh - ll) * 100.0
}

// ── VWAP_D (daily cumulative VWAP — pandas_ta uses daily anchor) ─────────────
/// pandas_ta vwap with anchor='D' resets at midnight each day.
/// Since we work with a fixed kline window, we approximate using cumulative from
/// the first kline of the current "day" (last 24 bars for 1h timeframe).
fn vwap_daily(highs: &[f64], lows: &[f64], closes: &[f64], volumes: &[f64]) -> f64 {
    let n = highs.len();
    // Use the full dataset (treat all klines as same session for simplicity)
    // pandas_ta daily VWAP = cumsum(TP*Vol) / cumsum(Vol) since day start
    // For 1h bars: assume last 24 bars = today
    let session_len = n.min(24);
    let start = n - session_len;

    let mut cum_tp_vol = 0.0;
    let mut cum_vol = 0.0;
    for i in start..n {
        let tp = (highs[i] + lows[i] + closes[i]) / 3.0;
        cum_tp_vol += tp * volumes[i];
        cum_vol += volumes[i];
    }
    if cum_vol == 0.0 { closes[n - 1] } else { cum_tp_vol / cum_vol }
}

// ── CMF(20) ──────────────────────────────────────────────────────────────────
fn cmf(highs: &[f64], lows: &[f64], closes: &[f64], volumes: &[f64], period: usize) -> f64 {
    let n = closes.len();
    if n < period { return 0.0; }
    let mut sum_mfv = 0.0;
    let mut sum_vol = 0.0;
    for i in (n - period)..n {
        let hl = highs[i] - lows[i];
        let mfm = if hl != 0.0 { ((closes[i] - lows[i]) - (highs[i] - closes[i])) / hl } else { 0.0 };
        sum_mfv += mfm * volumes[i];
        sum_vol += volumes[i];
    }
    if sum_vol == 0.0 { 0.0 } else { sum_mfv / sum_vol }
}
