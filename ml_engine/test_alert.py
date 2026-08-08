import json
import os

history_path = '/home/vth/Vth/ml_engine/data/monitoring/auc_history.json'
os.makedirs(os.path.dirname(history_path), exist_ok=True)

# Generate mock data for the past 2 days with AUC < 0.52
data = {
    "history": [
        {"date": "2026-07-24", "auc_layer1": 0.51, "auc_layer2": 0.53, "auc_layer3": 0.49},
        {"date": "2026-07-25", "auc_layer1": 0.50, "auc_layer2": 0.52, "auc_layer3": 0.48}
    ],
    "alert_triggered": False,
    "last_alert_date": None
}

with open(history_path, 'w') as f:
    json.dump(data, f, indent=2)

print("Mock history injected.")
