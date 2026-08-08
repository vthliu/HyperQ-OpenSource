import os
import sys
import time
import yaml
import sqlite3
import pandas as pd
import numpy as np
from datetime import datetime, timedelta
import ccxt
from sklearn.metrics import roc_auc_score

DB_PATH = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'data', 'tracking.db')
CONFIG_PATH = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'config.yaml')

def load_config():
    with open(CONFIG_PATH, 'r') as f:
        return yaml.safe_load(f)

def get_actual_labels(predictions_df, horizon, tp_pct, sl_pct):
    exchange = ccxt.binanceusdm({'enableRateLimit': True})
    
    # We need to fetch future prices for each symbol
    labels = []
    
    unique_symbols = predictions_df['symbol'].unique()
    for symbol in unique_symbols:
        symbol_preds = predictions_df[predictions_df['symbol'] == symbol]
        min_ts = symbol_preds['timestamp'].min()
        max_ts = symbol_preds['timestamp'].max()
        
        # Fetch klines from min_ts to max_ts + horizon
        since = int(min_ts.timestamp() * 1000)
        
        try:
            # We fetch 1h klines
            klines = exchange.fetch_ohlcv(symbol, '1h', since=since, limit=200)
            if not klines:
                for _ in range(len(symbol_preds)):
                    labels.append(np.nan)
                continue
                
            df_klines = pd.DataFrame(klines, columns=['ts', 'open', 'high', 'low', 'close', 'volume'])
            df_klines['ts'] = pd.to_datetime(df_klines['ts'], unit='ms')
            
            # Very slow mapping (just for drift monitor, it's fine)
            for _, row in symbol_preds.iterrows():
                pred_ts = row['timestamp']
                
                # Filter future klines within horizon
                future_klines = df_klines[(df_klines['ts'] > pred_ts) & 
                                          (df_klines['ts'] <= pred_ts + timedelta(hours=horizon))]
                
                if len(future_klines) == 0:
                    labels.append(np.nan)
                    continue
                
                entry_price = row['mark_price']
                max_high = future_klines['high'].max()
                min_low = future_klines['low'].min()
                
                # Triple barrier logic
                is_long = row.get('is_long', 1)
                
                long_tp_price = entry_price * (1 + tp_pct)
                long_sl_price = entry_price * (1 + sl_pct)
                
                short_tp_price = entry_price * (1 - tp_pct)
                short_sl_price = entry_price * (1 - sl_pct)
                
                label = 0
                for _, k_row in future_klines.iterrows():
                    if is_long:
                        if k_row['low'] <= long_sl_price:
                            break
                        if k_row['high'] >= long_tp_price:
                            label = 1
                            break
                    else:
                        if k_row['high'] >= short_sl_price:
                            break
                        if k_row['low'] <= short_tp_price:
                            label = 1
                            break
                            
                labels.append(label)
                
        except Exception as e:
            print(f"Error fetching klines for {symbol}: {e}")
            for _ in range(len(symbol_preds)):
                labels.append(np.nan)
                
        time.sleep(0.1) # rate limit
        
    return labels

def monitor_slippage(conn):
    print("\n--- Slippage Analysis ---")
    query = """
    SELECT 
        tier,
        AVG(slippage_pct) as avg_slippage,
        COUNT(*) as trade_count
    FROM trades
    GROUP BY tier;
    """
    df = pd.read_sql_query(query, conn)
    print(df)
    
    for _, row in df.iterrows():
        if row['tier'] == 'layer3' and row['avg_slippage'] > 0.005 and row['trade_count'] > 10:
            print(f"⚠️ [WARNING] Layer 3 Slippage is too high: {row['avg_slippage']*100:.2f}% (Trades: {row['trade_count']})")
            print("Recommendation: Adjust risk multiplier or raise probability threshold for Layer 3.")


def send_telegram_alert(config, msg, job_id=None):
    telegram_cfg = config.get('telegram', {})
    bot_token = telegram_cfg.get('bot_token', '')
    chat_id = telegram_cfg.get('chat_id', '')
    
    if not bot_token or not chat_id:
        print(">>> TELEGRAM ALERT GENERATED (But token is empty, logging only) <<<")
        print(msg)
        return
        
    url = f"https://api.telegram.org/bot{bot_token}/sendMessage"
    payload = {
        "chat_id": chat_id,
        "text": msg,
        "parse_mode": "Markdown"
    }
    
    if job_id:
        payload["reply_markup"] = {
            "inline_keyboard": [
                [
                    {"text": "✅ 批准重训", "callback_data": f"approve_{job_id}"},
                    {"text": "❌ 拒绝", "callback_data": "reject"}
                ]
            ]
        }
        
    import requests
    try:
        response = requests.post(url, json=payload, timeout=10)
        response.raise_for_status()
        print("Telegram alert sent successfully.")
    except Exception as e:
        print(f"Failed to send Telegram alert: {e}")

def generate_retrain_package(layer, start_date, end_date):
    base_dir = os.path.dirname(os.path.dirname(__file__))
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    package_dir = os.path.join(base_dir, 'data', 'retrain_packages', timestamp)
    os.makedirs(package_dir, exist_ok=True)
    
    run_train_sh = os.path.join(package_dir, 'run_train.sh')
    with open(run_train_sh, 'w') as f:
        f.write("#!/bin/bash\n")
        f.write(f"cd {base_dir}\n")
        f.write("source venv/bin/activate\n")
        f.write(f"python -m training.train_xgb --full\n")
        f.write("echo 'Training complete. Models are saved in ml_engine/training/models/'\n")
    os.chmod(run_train_sh, 0o755)
    
    # Just a mock snapshot for now
    snapshot = {
        "generated_at": timestamp,
        "trigger_layer": layer,
        "data_window": f"{start_date} to {end_date}"
    }
    import json
    with open(os.path.join(package_dir, 'data_snapshot.json'), 'w') as f:
        json.dump(snapshot, f, indent=2)
        
    # Copy config
    config_path = os.path.join(base_dir, 'config.yaml')
    if os.path.exists(config_path):
        import shutil
        shutil.copy(config_path, os.path.join(package_dir, 'config_snapshot.yaml'))
        
    return package_dir

def monitor_auc_drift(conn, config):
    print("\n--- AUC Drift Analysis ---")
    horizon = config['features']['horizon_periods']
    tp_atr_multiplier = config['features']['tp_atr_multiplier']
    sl_atr_multiplier = config['features']['sl_atr_multiplier']
    
    auc_threshold = config.get('monitoring', {}).get('auc_threshold', 0.52)
    consecutive_days_alert = config.get('monitoring', {}).get('consecutive_days_alert', 3)
    
    first_pred = pd.read_sql_query("SELECT MIN(timestamp) as min_ts FROM predictions", conn)
    if first_pred.empty or pd.isna(first_pred.iloc[0]['min_ts']):
        print("No predictions found in DB.")
        return
        
    db_start_time = pd.to_datetime(first_pred.iloc[0]['min_ts'])
    db_age = datetime.utcnow() - db_start_time
    
    calm_days = config.get('monitoring', {}).get('calm_period_days', 7)
    min_trades = config.get('monitoring', {}).get('min_trades_for_calm_exit', 30)
    
    trade_count = 0
    try:
        t_df = pd.read_sql_query("SELECT COUNT(*) as c FROM trades WHERE type='MARKET_CLOSE'", conn)
        if not t_df.empty:
            trade_count = t_df.iloc[0]['c']
    except Exception as e:
        print(f"Failed to query trades count: {e}")
        
    is_calm_period = (db_age < timedelta(days=calm_days)) and (trade_count < min_trades)
    
    if is_calm_period:
        print(f"Calm Period Active: DB age is {db_age.days} days, Trades: {trade_count} (Requires {calm_days} days OR {min_trades} trades to exit).")
    else:
        print(f"Calm Period INACTIVE: Trades: {trade_count} >= {min_trades} OR Age: {db_age.days} >= {calm_days}.")
    
    end_eval_time = datetime.utcnow() - timedelta(hours=horizon)
    start_eval_time = end_eval_time - timedelta(hours=24)
    
    query = """
    SELECT timestamp, symbol, prob, tier, mark_price, is_long 
    FROM predictions
    WHERE timestamp >= ? AND timestamp <= ?
    """
    preds_df = pd.read_sql_query(query, conn, params=(start_eval_time, end_eval_time))
    
    if preds_df.empty:
        print(f"No matured predictions found between {start_eval_time} and {end_eval_time}.")
        return
        
    preds_df['timestamp'] = pd.to_datetime(preds_df['timestamp'])
    
    import time
    preds_df['actual_label'] = get_actual_labels(preds_df, horizon, tp_pct, sl_pct)
    valid_df = preds_df.dropna(subset=['actual_label'])
    
    today_str = datetime.now().strftime("%Y-%m-%d")
    today_auc_results = {"date": today_str}
    
    from sklearn.metrics import roc_auc_score, precision_score
    for tier in ['layer1', 'layer2', 'layer3']:
        tier_df = valid_df[valid_df['tier'] == tier]
        if len(tier_df) < 20:
            print(f"Not enough {tier} valid labels to compute robust AUC (Found: {len(tier_df)}).")
            continue
            
        try:
            auc = roc_auc_score(tier_df['actual_label'], tier_df['prob'])
            print(f"\nReal-time 24h AUC ({tier}): {auc:.4f} (Samples: {len(tier_df)})")
            today_auc_results[f"auc_{tier}"] = float(auc)
        except ValueError as e:
            print(f"Could not calculate AUC for {tier}: {e}")
            
    # State tracking
    base_dir = os.path.dirname(os.path.dirname(__file__))
    history_path = os.path.join(base_dir, 'data', 'monitoring', 'auc_history.json')
    os.makedirs(os.path.dirname(history_path), exist_ok=True)
    
    import json
    history_data = {"history": [], "alert_triggered": False, "last_alert_date": None}
    if os.path.exists(history_path):
        try:
            with open(history_path, 'r') as f:
                history_data = json.load(f)
        except Exception:
            pass
            
    # Update history
    history = history_data["history"]
    # Remove existing entry for today if running multiple times
    history = [h for h in history if h['date'] != today_str]
    history.append(today_auc_results)
    
    # Keep last 14 days to prevent file bloating
    history = history[-14:]
    history_data["history"] = history
    
    # Evaluate triggers
    if history_data["alert_triggered"]:
        print("Alert already triggered. Awaiting manual reset ('IGNORE' or new deployment).")
    elif not is_calm_period and len(history) >= consecutive_days_alert:
        recent_days = history[-consecutive_days_alert:]
        # Check if any layer is < threshold for all consecutive days
        trigger_layer = None
        for layer in ['layer1', 'layer2', 'layer3']:
            key = f"auc_{layer}"
            # Ensure the layer has data for all days
            if all(key in day and day[key] < auc_threshold for day in recent_days):
                trigger_layer = layer
                break
                
        if trigger_layer:
            print(f"\n🚨 [CRITICAL] {trigger_layer} has been < {auc_threshold} for {consecutive_days_alert} consecutive days!")
            
            # Generate package
            start_date_str = recent_days[0]['date']
            pkg_path = generate_retrain_package(trigger_layer, start_date_str, today_str)
            
            # Formulate message
            l1 = today_auc_results.get('auc_layer1', 'N/A')
            l2 = today_auc_results.get('auc_layer2', 'N/A')
            l3 = today_auc_results.get('auc_layer3', 'N/A')
            
            job_id = os.path.basename(pkg_path)
            
            msg = f"""🚨 *模型重训审批请求*

*触发原因：* {trigger_layer.capitalize()} AUC 已连续 {consecutive_days_alert} 天低于 {auc_threshold}
*当前 AUC：* L1={l1}, L2={l2}, L3={l3}（下跌趋势）
*数据窗口：* {start_date_str} 至 {today_str}
*任务编号：* `{job_id}`

*请在下方直接点击按钮进行一键审批。*"""
            
            send_telegram_alert(config, msg, job_id=job_id)
            history_data["alert_triggered"] = True
            history_data["last_alert_date"] = today_str

    with open(history_path, 'w') as f:
        json.dump(history_data, f, indent=2)

def main():
    print(f"[{datetime.now().isoformat()}] Running Drift Monitor...")
    config = load_config()
    conn = sqlite3.connect(DB_PATH)
    
    try:
        monitor_slippage(conn)
        monitor_auc_drift(conn, config)
    finally:
        conn.close()

if __name__ == "__main__":
    main()
