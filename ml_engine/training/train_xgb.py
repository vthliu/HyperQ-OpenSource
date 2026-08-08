import os
import yaml
import pandas as pd
import xgboost as xgb
from sklearn.metrics import roc_auc_score, precision_score
import pickle

import sys
import json
import argparse
sys.path.append(os.path.dirname(os.path.dirname(__file__)))

from data.storage import load_raw_klines
from features.processor import transform
from features.labels import triple_barrier_labels

def prepare_data(config, tier=None, symbols=None):
    if symbols:
        active_symbols = symbols
    else:
        active_symbols = config['data']['symbols']
        
    timeframe = config['data'].get('timeframe_mapping', {}).get(tier, config['data'].get('timeframe', '1h'))
    raw_dir = os.path.join(os.path.dirname(os.path.dirname(__file__)), config['data']['raw_dir'])
    
    all_dfs = []
    
    for symbol in active_symbols:
        df = load_raw_klines(symbol, timeframe, raw_dir)
        if df is None:
            continue
            
        print(f"Processing {symbol} (Rows: {len(df)})")
        
        # 1. Feature Engineering (Shared with Inference)
        df = transform(df)
        
        # 2. Labeling
        df = triple_barrier_labels(
            df, 
            horizon=config['features']['horizon_periods'],
            tp_atr_multiplier=config['features']['tp_atr_multiplier'],
            sl_atr_multiplier=config['features']['sl_atr_multiplier']
        )
        
        df['symbol'] = symbol
        df = df.dropna()
        
        # Memory optimization
        exclude_early = ['open', 'high', 'low', 'close', 'volume', 'quote_asset_volume', 'number_of_trades', 'taker_buy_quote_asset_volume']
        keep_cols = [c for c in df.columns if c not in exclude_early]
        df = df[keep_cols]
        f64_cols = df.select_dtypes(include=['float64']).columns
        df[f64_cols] = df[f64_cols].astype('float32')
        
        all_dfs.append(df)
        
    full_df = pd.concat(all_dfs, ignore_index=True)
    import gc
    del all_dfs
    gc.collect()
    full_df['timestamp'] = pd.to_datetime(full_df['timestamp'], unit='ms')
    full_df = full_df.sort_values('timestamp').reset_index(drop=True)
    
    return full_df

def train_for_tier(tier, symbols, config):
    print(f"Preparing dataset for {tier} with timeframe mapped features and Triple-Barrier labels...")
    df = prepare_data(config, tier, symbols)
    
    if len(df) < 1000:
        print(f"Skipping {tier}: Not enough data ({len(df)} rows).")
        return
    
    exclude_cols = [
        'timestamp', 'symbol', 'label_long', 'label_short', 'open', 'high', 'low', 'close', 'volume',
        'quote_asset_volume', 'number_of_trades', 'taker_buy_quote_asset_volume'
    ]
    feature_cols = [c for c in df.columns if c not in exclude_cols]
    
    # Time-series rolling split
    train_start = pd.to_datetime(config['training']['train_start'], utc=True)
    train_end = pd.to_datetime(config['training']['train_end'], utc=True)
    val_start = pd.to_datetime(config['training']['val_start'], utc=True)
    val_end = pd.to_datetime(config['training']['val_end'], utc=True)
    test_start = pd.to_datetime(config['training']['test_start'], utc=True)
    test_end = pd.to_datetime(config['training']['test_end'], utc=True)
    
    if df['timestamp'].dt.tz is None:
        train_start = train_start.tz_localize(None)
        train_end = train_end.tz_localize(None)
        val_start = val_start.tz_localize(None)
        val_end = val_end.tz_localize(None)
        test_start = test_start.tz_localize(None)
        test_end = test_end.tz_localize(None)
    
    train_df = df[(df['timestamp'] >= train_start) & (df['timestamp'] <= train_end)]
    val_df = df[(df['timestamp'] >= val_start) & (df['timestamp'] <= val_end)]
    test_df = df[(df['timestamp'] >= test_start) & (df['timestamp'] <= test_end)]
    
    import gc
    del df
    gc.collect()
    
    if tier == "layer3":
        print("Downsampling layer3 train data to 30% to prevent OOM...")
        train_df = train_df.sample(frac=0.3, random_state=42)
        
    print(f"Data Splitting:")
    print(f"Train: {len(train_df)} rows")
    print(f"Validation: {len(val_df)} rows")
    print(f"Test: {len(test_df)} rows")
    
    X_train = train_df[feature_cols]
    X_val = val_df[feature_cols]
    X_test = test_df[feature_cols]
    
    train_direction(tier, 'long', X_train, train_df['label_long'], X_val, val_df['label_long'], X_test, test_df['label_long'], config)
    train_direction(tier, 'short', X_train, train_df['label_short'], X_val, val_df['label_short'], X_test, test_df['label_short'], config)
    
    # Save feature list
    feature_path = f"ml_engine/training/models/feature_list_{tier}.pkl"
    import joblib
    joblib.dump(feature_cols, feature_path)
    print(f"Features saved to {feature_path}\n")

def train_direction(tier, direction, X_train, y_train, X_val, y_val, X_test, y_test, config):
    print(f"\n--- Training {direction.upper()} Model for {tier} ---")
    xgb_params = {
        'objective': 'binary:logistic',
        'eval_metric': 'auc',
        'tree_method': 'hist',
        'max_depth': config['training']['max_depth'],
        'eta': config['training']['eta'],
        'subsample': config['training']['subsample'],
        'colsample_bytree': config['training']['colsample_bytree'],
        'min_child_weight': config['training']['min_child_weight'],
        'alpha': config['training']['reg_alpha'],
        'lambda': config['training']['reg_lambda'],
    }
    
    if tier == "layer3":
        xgb_params['max_depth'] = 3
        xgb_params['eta'] = 0.02
        xgb_params['subsample'] = 0.6
        xgb_params['alpha'] = 1.0
        xgb_params['lambda'] = 3.0
    
    # Handle Class Imbalance (scale_pos_weight)
    num_neg = (y_train == 0).sum()
    num_pos = (y_train == 1).sum()
    if num_pos > 0:
        if tier == "layer3":
            xgb_params['scale_pos_weight'] = 3.0
        else:
            xgb_params['scale_pos_weight'] = float(num_neg / num_pos)
        print(f"Class Imbalance: {num_neg} negative vs {num_pos} positive. Set scale_pos_weight = {xgb_params['scale_pos_weight']:.2f}")
    
    dtrain = xgb.DMatrix(X_train, label=y_train)
    dval = xgb.DMatrix(X_val, label=y_val)
    dtest = xgb.DMatrix(X_test, label=y_test)
    
    evals = [(dtrain, 'train'), (dval, 'val')]
    model = xgb.train(
        xgb_params,
        dtrain,
        num_boost_round=config['training']['n_estimators'],
        evals=evals,
        early_stopping_rounds=config['training']['early_stopping_rounds'],
        verbose_eval=100
    )
    
    print(f"\nEvaluating on pure OUT-OF-SAMPLE Test Set ({direction.upper()})...")
    preds = model.predict(dtest)
    auc = roc_auc_score(y_test, preds)
    threshold_10_percent = pd.Series(preds).quantile(0.9)
    binary_preds_top_10 = (preds >= threshold_10_percent).astype(int)
    prec = precision_score(y_test, binary_preds_top_10, zero_division=0)
    base_rate = y_test.mean()
    
    print("=====================================")
    print(f"TEST SET AUC: {auc:.4f}")
    print(f"TEST SET Precision @ Top 10%: {prec:.4f}")
    print(f"Base rate (Positive labels in test): {base_rate:.4f}")
    print("=====================================")
    
    import gc
    del dtrain, dval, dtest
    gc.collect()
    
    import os
    import json
    import tempfile
    import shutil
    os.makedirs('ml_engine/training/models', exist_ok=True)
    model_path = f"ml_engine/training/models/model_{tier}_{direction}.json"
    model.save_model(model_path)
    print(f"Model saved to {model_path}")
    
    # Save native threshold dynamically
    thresholds_path = "ml_engine/data/optimization/native_thresholds.json"
    os.makedirs(os.path.dirname(thresholds_path), exist_ok=True)
    
    native_thresholds = {}
    if os.path.exists(thresholds_path):
        try:
            with open(thresholds_path, 'r') as f:
                native_thresholds = json.load(f)
        except Exception:
            pass
            
    if tier not in native_thresholds:
        native_thresholds[tier] = {}
        
    native_thresholds[tier][direction] = float(threshold_10_percent)
    
    long_th = native_thresholds[tier].get('long', float(threshold_10_percent))
    short_th = native_thresholds[tier].get('short', float(threshold_10_percent))
    native_thresholds[tier]['baseline'] = min(long_th, short_th)
    
    temp_fd, temp_path = tempfile.mkstemp(dir=os.path.dirname(thresholds_path))
    with os.fdopen(temp_fd, 'w') as f:
        json.dump(native_thresholds, f, indent=4)
        f.flush()
        os.fsync(f.fileno())
        
    shutil.move(temp_path, thresholds_path)
    print(f"Native threshold for {tier} {direction} saved as {threshold_10_percent:.4f}\n")

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--full", action="store_true", help="Train on all mature symbols in registry")
    args = parser.parse_args()

    config_path = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'config.yaml')
    with open(config_path, 'r') as f:
        config = yaml.safe_load(f)
        
    symbols_by_tier = {"global": config['data'].get('symbols', [])}
    if args.full:
        registry_path = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'data', 'symbol_registry.json')
        if os.path.exists(registry_path):
            with open(registry_path, 'r') as f:
                registry = json.load(f)
            
            symbols_by_tier = {"layer1": [], "layer2": [], "layer3": []}
            for s, info in registry.get("symbols", {}).items():
                if info.get("status") == "MATURE" and not info.get("data_insufficient", False):
                    tier = info.get("tier", "layer3")
                    if tier in symbols_by_tier:
                        symbols_by_tier[tier].append(s)
            
            print(f"Loaded symbols by tier: { {k: len(v) for k,v in symbols_by_tier.items()} }")
            
    for tier, symbols in symbols_by_tier.items():
        if not symbols:
            continue
        print(f"=== Training {tier} Model ({len(symbols)} symbols) ===")
        train_for_tier(tier, symbols, config)

if __name__ == "__main__":
    main()
