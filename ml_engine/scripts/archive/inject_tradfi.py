import json, os
from datetime import datetime, timezone, timedelta

BEIJING_TZ = timezone(timedelta(hours=8))
now_bj = datetime.now(BEIJING_TZ).strftime("%Y-%m-%d %H:%M:%S")

registry_path = "data/symbol_registry.json"
with open(registry_path, 'r') as f:
    registry = json.load(f)

tradfi = {
    "layer1": ["TSLAUSDT", "AMZNUSDT", "MSTRUSDT", "AMDUSDT", "INTCUSDT"],
    "layer2": ["XAGUSDT", "BXUSDT", "HPEUSDT"],
    "layer3": ["AMATUSDT", "TQQQUSDT"]
}

for tier, symbols in tradfi.items():
    threshold = 0.70 if tier == "layer3" else 0.60
    for symbol in symbols:
        if symbol not in registry["symbols"]:
            registry["symbols"][symbol] = {
                "symbol": symbol,
                "status": "MATURE",
                "discovered_at": now_bj,
                "launch_at": now_bj,
                "data_count": 0,
                "tier": tier,
                "cooldown_hours": 2,
                "prob_threshold": threshold,
                "first_signal_at": None,
                "first_trade_at": None,
                "manual_override": True,
                "data_insufficient": True # Cannot fetch data from Binance for these yet
            }

with open(registry_path, 'w') as f:
    json.dump(registry, f, indent=2)

print(f"Total symbols now: {len(registry['symbols'])}")
