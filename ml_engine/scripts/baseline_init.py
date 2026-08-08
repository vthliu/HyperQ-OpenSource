import os
import json
import requests
from datetime import datetime, timezone, timedelta

# Beijing Time
BEIJING_TZ = timezone(timedelta(hours=8))
REGISTRY_PATH = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'data', 'symbol_registry.json')
EXCHANGE_INFO_URL = "https://fapi.binance.com/fapi/v1/exchangeInfo"
TICKER_24H_URL = "https://fapi.binance.com/fapi/v1/ticker/24hr"

def load_registry():
    if os.path.exists(REGISTRY_PATH):
        with open(REGISTRY_PATH, 'r') as f:
            return json.load(f)
    return {"symbols": {}}

def save_registry(registry):
    os.makedirs(os.path.dirname(REGISTRY_PATH), exist_ok=True)
    with open(REGISTRY_PATH, 'w') as f:
        json.dump(registry, f, indent=2)

def fetch_exchange_info():
    response = requests.get(EXCHANGE_INFO_URL, timeout=10)
    response.raise_for_status()
    data = response.json()
    symbols = []
    for s in data.get('symbols', []):
        if s.get('contractType') == 'PERPETUAL' and s.get('quoteAsset') == 'USDT' and s.get('status') == 'TRADING':
            symbols.append(s['symbol'])
    return symbols

def fetch_24h_tickers():
    response = requests.get(TICKER_24H_URL, timeout=10)
    response.raise_for_status()
    data = response.json()
    volume_map = {}
    for item in data:
        symbol = item['symbol']
        # quoteVolume is the USDT volume
        try:
            quote_volume = float(item['quoteVolume'])
        except (ValueError, TypeError):
            quote_volume = 0.0
        volume_map[symbol] = quote_volume
    return volume_map

def run_baseline_init():
    print("Starting Baseline Initialization...")
    now_bj = datetime.now(BEIJING_TZ).strftime("%Y-%m-%d %H:%M:%S")
    
    registry = load_registry()
    existing_symbols = set(registry["symbols"].keys())
    
    print("Fetching exchange info...")
    active_symbols = fetch_exchange_info()
    
    print("Fetching 24h ticker data...")
    volume_map = fetch_24h_tickers()
    
    new_count = 0
    updated_count = 0
    
    for symbol in active_symbols:
        quote_volume = volume_map.get(symbol, 0.0)
        
        # Classification
        if quote_volume == 0.0:
            tier = "layer3"
            data_insufficient = True
        elif quote_volume > 1_000_000_000:
            tier = "layer1"
            data_insufficient = False
        elif quote_volume > 100_000_000:
            tier = "layer2"
            data_insufficient = False
        else:
            tier = "layer3"
            data_insufficient = False
            
        # Threshold logic
        if tier == "layer3":
            threshold = 0.70
            cooldown = 0.33
        elif tier == "layer2":
            threshold = 0.60
            cooldown = 1.0
        else:
            threshold = 0.60
            cooldown = 1.0
            
        # Determine if we should preserve existing
        if symbol in existing_symbols:
            # Preserve tier if it was manually set to something else? 
            # Requirements say: "自定义 Layer 1 标的不满足流动性门槛，人工覆盖优先"
            # We will just preserve the entire existing record to be safe, except updating status to MATURE
            record = registry["symbols"][symbol]
            record["status"] = "MATURE"
            if "data_insufficient" not in record:
                record["data_insufficient"] = False
            updated_count += 1
        else:
            # Completely new symbol to the registry
            registry["symbols"][symbol] = {
                "symbol": symbol,
                "status": "MATURE",
                "discovered_at": now_bj,
                "launch_at": now_bj,
                "data_count": 0,
                "tier": tier,
                "cooldown_hours": cooldown,
                "prob_threshold": threshold,
                "first_signal_at": None,
                "first_trade_at": None,
                "manual_override": False,
                "data_insufficient": data_insufficient
            }
            new_count += 1
            
    save_registry(registry)
    print(f"[{now_bj}] Baseline Init Complete. Added {new_count} new symbols. Updated {updated_count} existing symbols.")
    print(f"Total symbols in registry: {len(registry['symbols'])}")

if __name__ == "__main__":
    run_baseline_init()
