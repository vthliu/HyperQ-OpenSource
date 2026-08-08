import json
import os
import time
import shutil
import sqlite3
import subprocess
import pandas as pd
from datetime import datetime, timedelta

DB_PATH = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'data', 'tracking.db')
CANDIDATE_PATH = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'data', 'optimization', 'deployment_candidate.json')
CONFIG_PATH = '/home/vth/Vth/hyperq-rs/config.toml'
CONFIG_DIR = os.path.dirname(CONFIG_PATH)

def fetch_historical_baseline():
    conn = sqlite3.connect(DB_PATH)
    # Calculate hard stops over the last 7 days
    query = """
    SELECT count(*) as count 
    FROM trades 
    WHERE type = 'MARKET_CLOSE' 
      AND exit_reason LIKE '%Hard Stop%'
      AND timestamp > datetime('now', '-7 days')
    """
    df = pd.read_sql_query(query, conn)
    conn.close()
    
    # 7 days baseline hard stops
    hs_7d = df.iloc[0]['count']
    # Calculate per 6-hour baseline
    # 7 days = 28 * 6 hours
    hs_6h_avg = hs_7d / 28.0 if hs_7d > 0 else 0.5 # Min baseline of 0.5 to prevent dividing by zero or being too strict
    return hs_6h_avg

def check_conditions_since(start_time, baseline_hs):
    conn = sqlite3.connect(DB_PATH)
    query = f"""
    SELECT 
        unrealized_roe, exit_reason, slippage_pct
    FROM trades 
    WHERE type = 'MARKET_CLOSE' 
      AND timestamp > '{start_time.isoformat()}'
    ORDER BY timestamp ASC
    """
    df = pd.read_sql_query(query, conn)
    conn.close()
    
    if len(df) == 0:
        return None
        
    # 1. Check Hard Stop Frequency
    hs_count = len(df[df['exit_reason'].str.contains('Hard Stop', na=False)])
    if hs_count > max(baseline_hs * 2, 2): # allow at least 2 before triggering
        return f"sentinel_hard_stop_spike (count={hs_count}, baseline={baseline_hs:.2f})"
        
    # 2. Check Continuous Losses
    consecutive_losses = 0
    for roe in df['unrealized_roe']:
        if pd.notnull(roe) and roe < 0:
            consecutive_losses += 1
            if consecutive_losses >= 3:
                return "consecutive_losses (3 in a row)"
        else:
            consecutive_losses = 0
            
    # 3. Check Large Loss
    for roe in df['unrealized_roe']:
        if pd.notnull(roe) and roe < -0.15:
            return f"large_loss (roe={roe*100:.2f}%)"
            
    # 4. Check Slippage
    if df['slippage_pct'].mean() > 0.005:
        return f"high_slippage (avg={df['slippage_pct'].mean()*100:.2f}%)"
        
    return None

def rollback(reason, candidate):
    print(f"⚠️ [ROLLBACK] 触发回滚！原因: {reason}")
    deployment_id = candidate.get('deployment_id')
    
    backup_path = os.path.join(CONFIG_DIR, f"config.toml.backup_{deployment_id}")
    if os.path.exists(backup_path):
        shutil.copy2(backup_path, CONFIG_PATH)
        print("[ROLLBACK] ✅ config.toml 已回滚")
    else:
        print(f"[ROLLBACK] ❌ 找不到备份文件 {backup_path}！无法回滚！")
        return
        
    print("[ROLLBACK] 重启 hyperq.service...")
    subprocess.run(["sudo", "systemctl", "restart", "hyperq.service"])
    print("[ROLLBACK] ✅ hyperq.service 已重启")
    
    candidate['rollback_triggered'] = True
    candidate['rollback_reason'] = reason
    candidate['rollback_timestamp'] = datetime.now().isoformat()
    
    with open(CANDIDATE_PATH, 'w') as f:
        json.dump(candidate, f, indent=4)
        
    print(f"🔴 [ALERT] 参数回滚已触发！原因: {reason}")

def main():
    if not os.path.exists(CANDIDATE_PATH):
        print("[MONITOR] No deployment candidate found. Exiting.")
        return
        
    with open(CANDIDATE_PATH, 'r') as f:
        candidate = json.load(f)
        
    if candidate.get('rollback_triggered'):
        print("[MONITOR] Rollback already triggered. Exiting.")
        return
        
    deployment_timestamp_str = candidate.get('deployment_timestamp')
    if not deployment_timestamp_str:
        print("[MONITOR] Not deployed yet. Exiting.")
        return
        
    start_time = datetime.fromisoformat(deployment_timestamp_str)
    end_time = start_time + timedelta(hours=6)
    
    if datetime.now() > end_time:
        print("[MONITOR] 6-hour monitoring window has already expired cleanly. Exiting.")
        return
        
    baseline_hs = fetch_historical_baseline()
    print(f"[MONITOR] Starting 6-hour watch. Baseline Hard Stops per 6H: {baseline_hs:.2f}")
    
    while datetime.now() < end_time:
        reason = check_conditions_since(start_time, baseline_hs)
        if reason:
            rollback(reason, candidate)
            return
            
        # sleep 5 minutes
        time.sleep(300)
        
    print("[MONITOR] ✅ 6-hour monitoring completed safely. Deployment is stable.")

if __name__ == "__main__":
    main()
