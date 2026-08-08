import sqlite3
import pandas as pd
import json
import os

DB_PATH = '/home/vth/Vth/ml_engine/data/tracking.db'
AUC_PATH = '/home/vth/Vth/ml_engine/data/monitoring/auc_history.json'

print("=== 1. Checking AUC History ===")
if os.path.exists(AUC_PATH):
    with open(AUC_PATH, 'r') as f:
        data = json.load(f)
        print(json.dumps(data, indent=2))
else:
    print("No AUC history found.")

print("\n=== 2. Checking Trades Database ===")
if not os.path.exists(DB_PATH):
    print("No tracking.db found.")
else:
    conn = sqlite3.connect(DB_PATH)
    
    # Check tables
    cursor = conn.cursor()
    cursor.execute("SELECT name FROM sqlite_master WHERE type='table';")
    tables = cursor.fetchall()
    print("Tables in DB:", [t[0] for t in tables])
    
    if 'trades' in [t[0] for t in tables]:
        # get schema
        df_schema = pd.read_sql_query("PRAGMA table_info(trades)", conn)
        print("\nTrades Table Columns:", df_schema['name'].tolist())
        
        # Let's just fetch recent trades
        try:
            # We don't know exactly what columns exist, let's select all and inspect
            df = pd.read_sql_query("SELECT * FROM trades ORDER BY timestamp DESC LIMIT 100", conn)
            
            # Since 'tier' might not be in trades, we might need to get it from predictions if they share a msg_id,
            # or maybe the symbol can be used to infer layer3
            # Layer3 assets from config:
            import yaml
            with open('/home/vth/Vth/ml_engine/config.yaml', 'r') as f:
                config = yaml.safe_load(f)
            layer3_assets = config.get('inference', {}).get('layer3_assets', [])
            
            if 'tier' in df.columns:
                l3_df = df[df['tier'] == 'layer3'].head(20)
                other_df = df[df['tier'] != 'layer3'].head(20)
            else:
                # Infer by symbol
                l3_df = df[df['symbol'].isin(layer3_assets)].head(20)
                other_df = df[~df['symbol'].isin(layer3_assets)].head(20)
                
            print(f"\nFound {len(l3_df)} recent Layer 3 trades in the last 100.")
            print(f"Found {len(other_df)} recent Layer 1/2 trades in the last 100.")
            
            def calc_metrics(subset):
                if len(subset) == 0:
                    return None
                
                # Assume columns: unrealized_roe or we can compute from fill_price and entry_price
                # From earlier, executor logs: "unrealized_roe", "entry_price", "fill_price", "qty", "side"
                if 'unrealized_roe' in subset.columns:
                    roes = subset['unrealized_roe']
                else:
                    # try to calculate
                    print(subset.head(1))
                    return None
                    
                wins = roes[roes > 0]
                losses = roes[roes <= 0]
                
                win_rate = len(wins) / len(subset)
                avg_win = wins.mean() if len(wins) > 0 else 0
                avg_loss = losses.mean() if len(losses) > 0 else 0
                
                return {
                    "count": len(subset),
                    "win_rate": win_rate,
                    "avg_win": avg_win,
                    "avg_loss": avg_loss,
                    "avg_roe": roes.mean()
                }
                
            l3_metrics = calc_metrics(l3_df)
            other_metrics = calc_metrics(other_df)
            
            print("\nLayer 3 Metrics (last 20):", l3_metrics)
            print("Layer 1/2 Metrics (last 20):", other_metrics)
            
        except Exception as e:
            print("Error parsing trades:", e)
    else:
        print("No 'trades' table.")
    conn.close()
