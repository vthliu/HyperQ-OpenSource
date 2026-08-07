/// Feature precision verification tool.
/// Reads test_vectors.json and compares Rust FeatureEngine output vs Python baseline.
use std::fs;

fn main() {
    let json_str = fs::read_to_string("test_vectors.json").expect("Could not read test_vectors.json");
    let data: serde_json::Value = serde_json::from_str(&json_str).expect("Invalid JSON");

    // Parse klines
    let klines_json = data["klines"].as_array().expect("no klines");
    let mut klines: Vec<hyperq_rs::models::Kline> = Vec::new();
    for k in klines_json {
        klines.push(hyperq_rs::models::Kline {
            open:                k["open"].as_f64().unwrap_or(0.0),
            high:                k["high"].as_f64().unwrap_or(0.0),
            low:                 k["low"].as_f64().unwrap_or(0.0),
            close:               k["close"].as_f64().unwrap_or(0.0),
            volume:              k["volume"].as_f64().unwrap_or(0.0),
            quote_asset_volume:  k["quote_asset_volume"].as_f64().unwrap_or(0.0),
            number_of_trades:    k["number_of_trades"].as_u64().unwrap_or(0),
            taker_buy_quote:     k["taker_buy_quote_asset_volume"].as_f64().unwrap_or(0.0),
        });
    }

    println!("Loaded {} klines", klines.len());

    // Python baseline feature names in order
    let feature_names = vec![
        "SMA_5", "SMA_20", "SMA_50", "EMA_12", "EMA_26",
        "MACD_12_26_9", "MACDh_12_26_9", "MACDs_12_26_9",
        "RSI_14", "STOCHRSIk_14_14_3_3", "STOCHRSId_14_14_3_3",
        "BBL_20_2.0_2.0", "BBM_20_2.0_2.0", "BBU_20_2.0_2.0", "BBB_20_2.0_2.0", "BBP_20_2.0_2.0",
        "ATRr_14", "ADX_14", "ADXR_14_2", "DMP_14", "DMN_14",
        "CCI_20_0.015", "OBV", "MFI_14", "ROC_10", "WILLR_14",
        "VWAP_D", "CMF_20", "dist_sma_20", "dist_ema_26",
        "volume_surge_ratio", "pump_exhaustion_index", "bband_overextension",
        "upper_wick_ratio", "lower_wick_ratio", "price_tick_velocity",
        "trade_imbalance", "avg_trade_size", "whale_alert_score",
        "liquidity_score", "returns", "returns_lag_1", "returns_lag_2",
        "returns_lag_3", "returns_lag_5", "high_low_ratio", "high_low_ratio_lag_1",
    ];

    // Get Python baseline values
    let py_features = &data["features"];
    let py_vals: Vec<f64> = feature_names.iter().map(|name| {
        py_features[*name].as_f64().unwrap_or(0.0)
    }).collect();

    // Compute Rust features
    let rust_vals = hyperq_rs::features::FeatureEngine::compute_features(&klines);

    // Compare
    println!("\n{:<30} {:>18} {:>18} {:>15} {}", "Feature", "Python", "Rust", "Abs Err", "Status");
    println!("{}", "─".repeat(90));

    let mut max_err: f64 = 0.0;
    let mut max_err_feature = "";
    let mut pass_count = 0;
    let mut fail_count = 0;

    for (i, name) in feature_names.iter().enumerate() {
        let py = py_vals[i];
        let rs = rust_vals[i];
        let err = (py - rs).abs();
        let rel_err = if py.abs() > 1.0 { err / py.abs() } else { err };
        let status = if rel_err < 1e-4 { "✅ PASS" } else { "❌ FAIL" };

        if rel_err < 1e-4 { pass_count += 1; } else { fail_count += 1; }
        if err > max_err { max_err = err; max_err_feature = name; }

        println!("{:<30} {:>18.6} {:>18.6} {:>15.8} {}", name, py, rs, err, status);
    }

    println!("\n{}", "═".repeat(90));
    println!("Results: {} PASS, {} FAIL", pass_count, fail_count);
    println!("Max absolute error: {:.8} on '{}'", max_err, max_err_feature);
}
