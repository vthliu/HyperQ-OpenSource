import json

path = '/home/vth/Vth/ml_engine/data/symbol_registry.json'
with open(path, 'r') as f:
    data = json.load(f)

for symbol, info in data.items():
    if 'cooldown_hours' in info:
        info['cooldown_hours'] = 2

with open(path, 'w') as f:
    json.dump(data, f, indent=2)

print("Updated symbol_registry.json")
