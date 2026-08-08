import re

with open('/home/vth/Vth/ml_engine/monitoring/drift_monitor.py', 'r') as f:
    content = f.read()

# We want to replace everything from `def monitor_auc_drift` to the end of the file.
new_code = """
def send_telegram_alert(config, msg):
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
    
    import requests
    try:
        r = requests.post(url, json=payload, timeout=10)
        r.raise_for_status()
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
        f.write("#!/bin/bash\\n")
        f.write(f"cd {base_dir}\\n")
        f.write("source venv/bin/activate\\n")
        f.write(f"python -m training.train_xgb --full --output models/model_{timestamp}.json\\n")
        f.write("echo 'Training complete. Please validate the new model before deploying.'\\n")
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
    print("\\n--- AUC Drift Analysis ---")
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
    is_calm_period = db_age < timedelta(days=7)
    
    if is_calm_period:
        print(f"Calm Period Active: DB age is {db_age.days} days (requires 7 days before alerting).")
    
    end_eval_time = datetime.utcnow() - timedelta(hours=horizon)
    start_eval_time = end_eval_time - timedelta(hours=24)
    
    query = \"\"\"
    SELECT timestamp, symbol, prob, tier, mark_price, is_long 
    FROM predictions
    WHERE timestamp >= ? AND timestamp <= ?
    \"\"\"
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
            print(f"\\nReal-time 24h AUC ({tier}): {auc:.4f} (Samples: {len(tier_df)})")
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
            print(f"\\n🚨 [CRITICAL] {trigger_layer} has been < {auc_threshold} for {consecutive_days_alert} consecutive days!")
            
            # Generate package
            start_date_str = recent_days[0]['date']
            pkg_path = generate_retrain_package(trigger_layer, start_date_str, today_str)
            
            # Formulate message
            l1 = today_auc_results.get('auc_layer1', 'N/A')
            l2 = today_auc_results.get('auc_layer2', 'N/A')
            l3 = today_auc_results.get('auc_layer3', 'N/A')
            
            msg = f\"\"\"🚨 *模型重训审批请求*

*触发原因：* {trigger_layer.capitalize()} AUC 已连续 {consecutive_days_alert} 天低于 {auc_threshold}
*当前 AUC：* L1={l1}, L2={l2}, L3={l3}（下跌趋势）
*数据窗口：* {start_date_str} 至 {today_str}
*候选包路径：* `{pkg_path}`

*操作建议：*
1. 审查候选包内容
2. 若批准，执行：`cd {base_dir} && bash {pkg_path}/run_train.sh`

*如需取消本次告警*，请回复 "IGNORE" (手动编辑配置)。\"\"\"
            
            send_telegram_alert(config, msg)
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
"""

# Replace in content
split_str = "def monitor_auc_drift(conn, config):"
parts = content.split(split_str)
if len(parts) == 2:
    new_content = parts[0] + new_code
    with open('/home/vth/Vth/ml_engine/monitoring/drift_monitor.py', 'w') as f:
        f.write(new_content)
    print("Patch applied successfully.")
else:
    print("Could not find the split point.")
