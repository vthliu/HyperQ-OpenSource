import json

registry_path = "data/symbol_registry.json"
with open(registry_path, 'r') as f:
    registry = json.load(f)

for symbol, info in registry["symbols"].items():
    if info.get("manual_override") == True:
        info["data_insufficient"] = False

with open(registry_path, 'w') as f:
    json.dump(registry, f, indent=2)

print("Updated TradFi symbols to data_insufficient=False")
