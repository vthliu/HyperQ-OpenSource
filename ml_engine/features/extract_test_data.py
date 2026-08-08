import sys
import os
import json
import requests
import pandas as pd
import numpy as np

# ensure we can import processor
sys.path.append(os.path.dirname(os.path.abspath(__file__)))
from processor import transform

def fetch_klines(symbol='BTCUSDT', interval='1h', limit=200):
    url = f'https://fapi.binance.com/fapi/v1/klines?symbol={symbol}&interval={interval}&limit={limit}'
    response = requests.get(url).json()
    
    klines = []
    for k in response:
        klines.append({
            'open_time': int(k[0]),
            'open': float(k[1]),
            'high': float(k[2]),
            'low': float(k[3]),
            'close': float(k[4]),
            'volume': float(k[5]),
            'close_time': int(k[6]),
            'quote_asset_volume': float(k[7]),
            'number_of_trades': int(k[8]),
            'taker_buy_quote_asset_volume': float(k[10]),
        })
        
    df = pd.DataFrame(klines)
    df['timestamp'] = df['open_time']
    # Add dummy taker_buy_base for completeness if needed
    df['taker_buy_base_asset_volume'] = [float(k[9]) for k in response]
    return df, klines

def main():
    print("Fetching klines...")
    df, klines_json = fetch_klines()
    
    print("Computing features...")
    features_df = transform(df)
    
    # We only care about the last row of features for the inference logic
    last_row = features_df.iloc[-1].replace([np.inf, -np.inf], np.nan).fillna(0.0)
    
    # We need the exact 47 features that XGBoost uses. 
    # Let's extract them in order using the known list.
    feature_names = [
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
      "returns_lag_3", "returns_lag_5", "high_low_ratio", "high_low_ratio_lag_1"
    ]
    
    last_features = {}
    for f in feature_names:
        # Note: pandas_ta might use slightly different column names for BBands in output, e.g. BBL_20_2.0
        last_features[f] = float(last_row.get(f, 0.0))
        
    output = {
        "klines": klines_json,
        "features": last_features
    }
    
    out_path = '/home/vth/Vth/hyperq-rs/test_vectors.json'
    with open(out_path, 'w') as f:
        json.dump(output, f, indent=2)
        
    print(f"Saved test vectors to {out_path}")

if __name__ == "__main__":
    main()
