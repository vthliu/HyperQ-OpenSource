import time
import requests
import json
import os
import subprocess
from datetime import datetime, timezone, timedelta

# Beijing Time (UTC+8)
BEIJING_TZ = timezone(timedelta(hours=8))

# Paths
REGISTRY_PATH = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'data', 'symbol_registry.json')
FETCHER_SCRIPT = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'data', 'fetcher.py')

def load_registry():
    if not os.path.exists(REGISTRY_PATH):
        return {"symbols": {}}
    with open(REGISTRY_PATH, 'r') as f:
        return json.load(f)

def save_registry(registry):
    with open(REGISTRY_PATH, 'w') as f:
        json.dump(registry, f, indent=2)

def fetch_exchange_info():
    url = "https://fapi.binance.com/fapi/v1/exchangeInfo"
    try:
        response = requests.get(url, timeout=10)
        if response.status_code != 200:
            print(f"API Error fetching exchangeInfo: {response.status_code} - {response.text}")
            return None
        data = response.json()
        return [s['symbol'] for s in data.get('symbols', []) if s['contractType'] == 'PERPETUAL' and s['quoteAsset'] == 'USDT' and s.get('status') == 'TRADING']
    except Exception as e:
        print(f"Error fetching exchangeInfo: {e}")
        return None

def main():
    print("Starting Discovery Daemon...")
    
    while True:
        registry = load_registry()
        known_symbols = set(registry.get("symbols", {}).keys())
        
        current_symbols = fetch_exchange_info()
        if current_symbols is None:
            time.sleep(60)
            continue
        
        new_symbols = [s for s in current_symbols if s not in known_symbols]
        
        
        active_registered = [sym for sym, info in registry.get("symbols", {}).items() if info.get("status") != "DELISTED"]
        delisted_symbols = [s for s in active_registered if s not in current_symbols]
        
        updated = False
        
        if new_symbols:
            now_bj = datetime.now(BEIJING_TZ).strftime("%Y-%m-%d %H:%M:%S")
            print(f"[{now_bj}] Discovered new symbols: {new_symbols}")
            updated = True
            
            for symbol in new_symbols:
                # Add to registry (Layer 3 by default)
                registry["symbols"][symbol] = {
                    "symbol": symbol,
                    "status": "PRE_LISTING",
                    "discovered_at": now_bj,
                    "launch_at": now_bj,
                    "data_count": 0,
                    "tier": "layer3",
                    "cooldown_hours": 2,
                    "prob_threshold": 0.70,
                    "first_signal_at": None,
                    "first_trade_at": None,
                    "manual_override": False
                }
                
                print(f"Added {symbol} to registry. Triggering data fetch...")
                
                # Auto-fetch data
                try:
                    subprocess.run(["python3", FETCHER_SCRIPT], env=os.environ)
                except Exception as e:
                    print(f"Failed to fetch data for {symbol}: {e}")
                    
        if delisted_symbols:
            now_bj = datetime.now(BEIJING_TZ).strftime("%Y-%m-%d %H:%M:%S")
            print(f"[{now_bj}] [DELISTED] Found {len(delisted_symbols)} delisted symbols: {delisted_symbols}")
            for symbol in delisted_symbols:
                if symbol in registry["symbols"]:
                    registry["symbols"][symbol]["status"] = "DELISTED"
                    registry["symbols"][symbol]["delisted_at"] = now_bj
                
                # Cleanup disk space (Delete historical Parquet data)
                raw_dir = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'data', 'raw')
                for ext in ['.parquet', '.done']:
                    file_path = os.path.join(raw_dir, f"{symbol}_1h{ext}")
                    if os.path.exists(file_path):
                        try:
                            os.remove(file_path)
                            print(f"🧹 Cleaned up disk space: Deleted {file_path}")
                        except Exception as e:
                            print(f"Failed to delete {file_path}: {e}")
                            
            updated = True
            
        if updated:
            # Save updated registry
            save_registry(registry)
            
        time.sleep(60) # Poll every 1 minute

if __name__ == "__main__":
    main()
