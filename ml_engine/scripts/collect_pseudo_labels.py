import os
import time
import requests
import pandas as pd
import pandas_ta as ta
import numpy as np

BTC_CSV = "/home/vth/Vth/ml_engine/data/BTCUSDT_1h.csv"
OUTPUT_CSV = "/home/vth/Vth/ml_engine/data/regime_pseudo_labels.csv"

def fetch_btc_historical():
    """Fetches past 2 years of 1h K-lines for BTCUSDT if not exists."""
    if os.path.exists(BTC_CSV):
        print(f"Loading existing data from {BTC_CSV}")
        return pd.read_csv(BTC_CSV)
        
    print("Fetching BTCUSDT 1h K-lines from Binance API...")
    url = "https://fapi.binance.com/fapi/v1/klines"
    
    # 2 years = ~17520 hours
    # limit = 1500 per request
    limit = 1500
    all_data = []
    end_time = int(time.time() * 1000)
    
    for i in range(12): # approx 2 years
        params = {
            "symbol": "BTCUSDT",
            "interval": "1h",
            "limit": limit,
            "endTime": end_time
        }
        res = requests.get(url, params=params).json()
        if not res or type(res) != list or len(res) == 0:
            break
        all_data = res + all_data
        end_time = res[0][0] - 1
        time.sleep(0.5)
        
    df = pd.DataFrame(all_data, columns=["timestamp", "open", "high", "low", "close", "volume", "close_time", "qav", "num_trades", "taker_base", "taker_quote", "ignore"])
    df = df[["timestamp", "open", "high", "low", "close", "volume"]]
    for col in ["open", "high", "low", "close", "volume"]:
        df[col] = df[col].astype(float)
    df["timestamp"] = pd.to_datetime(df["timestamp"], unit="ms")
    df.drop_duplicates(subset="timestamp", inplace=True)
    df.sort_values("timestamp", inplace=True)
    
    os.makedirs(os.path.dirname(BTC_CSV), exist_ok=True)
    df.to_csv(BTC_CSV, index=False)
    return df

def generate_pseudo_labels(df):
    """Applies RegimeDetector logic to generate labels."""
    print("Calculating Technical Indicators (ADX, EMA, Bollinger Bands)...")
    df.ta.adx(length=14, append=True)
    df.ta.ema(length=20, append=True)
    df.ta.ema(length=50, append=True)
    df.ta.bbands(length=20, std=2.0, append=True)
    
    df.dropna(inplace=True)
    
    # Map column names
    adx_col = "ADX_14"
    ema20_col = "EMA_20"
    ema50_col = "EMA_50"
    bbb_col = [c for c in df.columns if c.startswith("BBB_")][0] # Bandwidth
    
    print("Simulating RegimeDetector state machine...")
    
    regime_labels = []
    
    # State lock variables
    current_regime = "CHOP_HIGH_VOL" # Initial safe state
    candidate_regime = None
    candidate_count = 0
    
    def get_raw_regime(row):
        adx = row[adx_col]
        ema20 = row[ema20_col]
        ema50 = row[ema50_col]
        bbw = row[bbb_col]
        
        if adx > 25:
            if ema20 > ema50:
                return "BULL_TREND"
            elif ema20 < ema50:
                return "BEAR_TREND"
        else:
            if bbw > 5.0:
                return "CHOP_HIGH_VOL"
            else:
                return "CHOP_LOW_VOL"
        return "CHOP_HIGH_VOL"
        
    regime_map = {
        "BULL_TREND": 0,
        "BEAR_TREND": 1,
        "CHOP_HIGH_VOL": 2,
        "CHOP_LOW_VOL": 3
    }
        
    for i, row in df.iterrows():
        raw_regime = get_raw_regime(row)
        
        if raw_regime == candidate_regime:
            candidate_count += 1
            if candidate_count >= 2:
                current_regime = raw_regime
        else:
            candidate_regime = raw_regime
            candidate_count = 1
            
        regime_labels.append(regime_map[current_regime])
        
    df["regime_label"] = regime_labels
    
    print("Label distribution:")
    print(df["regime_label"].value_counts(normalize=True) * 100)
    
    df.to_csv(OUTPUT_CSV, index=False)
    print(f"Saved {len(df)} pseudo-labels to {OUTPUT_CSV}")

if __name__ == "__main__":
    df = fetch_btc_historical()
    generate_pseudo_labels(df)
