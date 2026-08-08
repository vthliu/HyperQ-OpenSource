import time
import yaml
import os
import sys
import xgboost as xgb
import pandas as pd
import pickle
import ccxt
import json
import sqlite3
import numpy as np
import pytz
import threading
from datetime import datetime, timezone, timedelta

BEIJING_TZ = pytz.timezone('Asia/Shanghai')

# Add parent path
sys.path.append(os.path.dirname(os.path.dirname(__file__)))
DB_PATH = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'data', 'tracking.db')

from inference.feature_pipeline import get_latest_features
from inference.zmq_publisher import ZmqPublisher
from inference.regime_detector import RegimeDetector

def load_models_and_features():
    model_dir = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'training', 'models')
    
    models = {}
    features = {}
    
    for tier in ['layer1', 'layer2', 'layer3']:
        model_path_long = os.path.join(model_dir, f'model_{tier}_long.json')
        model_path_short = os.path.join(model_dir, f'model_{tier}_short.json')
        features_path = os.path.join(model_dir, f'feature_list_{tier}.pkl')
        
        if os.path.exists(model_path_long) and os.path.exists(model_path_short) and os.path.exists(features_path):
            model_long = xgb.Booster()
            model_long.load_model(model_path_long)
            
            model_short = xgb.Booster()
            model_short.load_model(model_path_short)
            
            models[tier] = {'long': model_long, 'short': model_short}
            
            with open(features_path, 'rb') as f:
                features[tier] = pickle.load(f)
            print(f"Loaded {tier} Long & Short models.")
        else:
            print(f"Warning: {tier} dual models not fully found.")
            
    if not models:
        raise FileNotFoundError("No tiered models found. Please run train_xgb.py --full first.")
        
    return models, features

def fetch_recent_klines(exchange, symbol, timeframe, limit=200):
    try:
        params = {'symbol': symbol, 'interval': exchange.timeframes[timeframe], 'limit': limit}
        ohlcv = exchange.fapiPublicGetKlines(params)
        
        parsed_ohlcv = [
            [
                int(c[0]),          # timestamp
                float(c[1]),        # open
                float(c[2]),        # high
                float(c[3]),        # low
                float(c[4]),        # close
                float(c[5]),        # volume
                float(c[7]),        # quote_asset_volume
                int(c[8]),          # number_of_trades
                float(c[10])        # taker_buy_quote_asset_volume
            ] for c in ohlcv
        ]
        
        df = pd.DataFrame(parsed_ohlcv, columns=[
            'timestamp', 'open', 'high', 'low', 'close', 'volume',
            'quote_asset_volume', 'number_of_trades', 'taker_buy_quote_asset_volume'
        ])
        return df
    except Exception as e:
        print(f"Error fetching data for {symbol}: {e}")
        return None

def main():
    config_path = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'config.yaml')
    with open(config_path, 'r') as f:
        config = yaml.safe_load(f)
        
    print("Loading XGBoost Models...")
    models, features_dict = load_models_and_features()
    
    print("Initializing ZMQ Publisher...")
    publisher = ZmqPublisher(config)
    
    timeframe_mapping = config['data'].get('timeframe_mapping', {})
    default_timeframe = config['data'].get('timeframe', '1h')
    
    exchange = ccxt.binanceusdm({'enableRateLimit': True})
    registry_path = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'data', 'symbol_registry.json')
    
    regime_detector = RegimeDetector()
    
    def regime_updater():
        while True:
            try:
                regime_detector.update(exchange)
            except Exception as e:
                print(f"[REGIME] 后台更新线程异常: {e}")
            time.sleep(60)
            
    threading.Thread(target=regime_updater, daemon=True).start()
    
    now_bj = datetime.now(BEIJING_TZ).strftime("%Y-%m-%d %H:%M:%S")
    print(f"[{now_bj}] Inference Daemon started. Monitoring real-time market data...")
    
    while True:
        try:
            with open(registry_path, 'r') as f:
                registry = json.load(f)
        except Exception as e:
            print(f"Failed to load registry: {e}")
            registry = {"symbols": {}}
            
        current_regime = regime_detector.get_regime()
            
        symbols_info = registry.get("symbols", {})
        
        for symbol, info in symbols_info.items():
            if info.get("status") == "DELISTED":
                continue
                
            tier = info.get("tier", "layer3")
            symbol_timeframe = timeframe_mapping.get(tier, default_timeframe)
            
            # 1. Fetch recent data to build sliding window for features
            df = fetch_recent_klines(exchange, symbol, symbol_timeframe, limit=200)
            if df is None or len(df) < 50:
                continue
                
            # 2. Get features for the most recent candle
            latest_features = get_latest_features(df, symbol_timeframe)
            
            if tier not in models:
                # Fallback to layer3 or any available model
                tier = list(models.keys())[0] if models else None
                if not tier: continue
            
            target_models = models[tier]
            feature_list = features_dict[tier]
            
            # Create DMatrix for inference (extract only the needed features in correct order)
            try:
                # Use feature_list from PKL as base
                base_infer = pd.DataFrame([latest_features[feature_list]])
                base_infer.replace([np.inf, -np.inf], np.nan, inplace=True)
                
                # Align for long model
                feats_long = target_models['long'].feature_names
                X_long = base_infer.copy()
                if feats_long:
                    for col in feats_long:
                        if col not in X_long.columns:
                            X_long[col] = latest_features.get(col, 0.0)
                    X_long = X_long[feats_long]
                dmatrix_long = xgb.DMatrix(X_long)
                
                # Align for short model
                feats_short = target_models['short'].feature_names
                X_short = base_infer.copy()
                if feats_short:
                    for col in feats_short:
                        if col not in X_short.columns:
                            X_short[col] = latest_features.get(col, 0.0)
                    X_short = X_short[feats_short]
                dmatrix_short = xgb.DMatrix(X_short)
                    
            except KeyError as e:
                print(f"Missing feature for {symbol}: {e}")
                continue
                
            # Perform Inference
            prob_long = float(target_models['long'].predict(dmatrix_long)[0])
            prob_short = float(target_models['short'].predict(dmatrix_short)[0])
            
            if abs(prob_long - prob_short) < 0.05:
                continue
                
            if prob_long > prob_short:
                is_long = True
                prob = prob_long
            else:
                is_long = False
                prob = prob_short
                
            current_price = latest_features['close']
            
            # Fetch ATR for HyperQ signal sizing
            atr = float(latest_features.get('ATRr_14', current_price * 0.01))
            
            # 4. Publish (ZmqPublisher handles rejection and cooling dynamically)
            publisher.publish(symbol, prob, current_price, atr, is_long, current_regime, registry_info=info)
            
            # 5. Log prediction to SQLite for drift monitoring
            try:
                conn = sqlite3.connect(DB_PATH)
                conn.execute('''
                    INSERT INTO predictions (timestamp, symbol, prob, tier, mark_price, is_long)
                    VALUES (datetime('now'), ?, ?, ?, ?, ?)
                ''', (symbol, prob, tier, current_price, is_long))
                conn.commit()
                conn.close()
            except Exception as e:
                print(f"Error logging prediction for {symbol}: {e}")
            
            # Throttle to prevent API bans across 500+ symbols
            time.sleep(0.1)
            
        # Run inference loop every 60 seconds
        time.sleep(60)

if __name__ == "__main__":
    main()
