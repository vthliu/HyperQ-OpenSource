import os
import sys
import yaml
import pandas as pd
import numpy as np
from scipy.stats import entropy

# Add parent path
sys.path.append(os.path.dirname(os.path.dirname(__file__)))

from data.storage import load_raw_klines
from features.processor import transform

def compute_kl_divergence(p, q, bins=50):
    """Compute KL Divergence between two distributions."""
    # Create histogram to estimate probabilities
    min_val = min(np.min(p), np.min(q))
    max_val = max(np.max(p), np.max(q))
    
    p_hist, _ = np.histogram(p, bins=bins, range=(min_val, max_val), density=True)
    q_hist, _ = np.histogram(q, bins=bins, range=(min_val, max_val), density=True)
    
    # Add small epsilon to avoid division by zero or log(0)
    p_hist = p_hist + 1e-10
    q_hist = q_hist + 1e-10
    
    # Normalize to probabilities
    p_hist /= np.sum(p_hist)
    q_hist /= np.sum(q_hist)
    
    return entropy(p_hist, q_hist)

def check_feature_drift(config):
    """Check feature drift for the last 7 days compared to training data."""
    print("Running Feature Drift Analysis (KL Divergence)...")
    
    symbols = config['data']['symbols']
    timeframe = config['data']['timeframe']
    raw_dir = os.path.join(os.path.dirname(os.path.dirname(__file__)), config['data']['raw_dir'])
    
    train_end = pd.to_datetime(config['training']['train_end'], utc=True)
    
    all_alerts = []
    
    for symbol in symbols:
        df = load_raw_klines(symbol, timeframe, raw_dir)
        if df is None:
            continue
            
        df = transform(df).dropna()
        df['timestamp'] = pd.to_datetime(df['timestamp'], unit='ms', utc=True)
        
        # Split into reference (train) and target (recent 7 days)
        ref_df = df[df['timestamp'] <= train_end]
        
        recent_cutoff = df['timestamp'].max() - pd.Timedelta(days=7)
        target_df = df[df['timestamp'] >= recent_cutoff]
        
        if len(target_df) < 20:
            print(f"[{symbol}] Not enough recent data for drift detection.")
            continue
            
        # Check drift on a few key features
        key_features = ['returns_lag_1', 'RSI_14', 'MACD_12_26_9', 'high_low_ratio']
        
        for feature in key_features:
            if feature not in ref_df.columns:
                continue
                
            kl = compute_kl_divergence(target_df[feature].values, ref_df[feature].values)
            
            if kl > 0.5:  # Arbitrary alert threshold for KL Divergence
                alert = f"🚨 [DRIFT ALERT] {symbol} | Feature: {feature} | KL Divergence: {kl:.3f} (>0.5)"
                all_alerts.append(alert)
                print(alert)
            else:
                print(f"✅ [NORMAL] {symbol} | Feature: {feature} | KL Divergence: {kl:.3f}")
                
    if all_alerts:
        print("\n!!! ACTION REQUIRED: Significant data drift detected. Consider retraining the XGBoost model !!!")
    else:
        print("\nAll key features look stable. No retraining required.")

if __name__ == "__main__":
    config_path = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'config.yaml')
    with open(config_path, 'r') as f:
        config = yaml.safe_load(f)
        
    check_feature_drift(config)
