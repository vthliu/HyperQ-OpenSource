import yaml
import pandas as pd
from training.train_xgb import prepare_data

with open('config.yaml', 'r') as f:
    config = yaml.safe_load(f)

df = prepare_data(config, symbols=['BTCUSDT'])
print("After prepare_data:", len(df), df['timestamp'].min(), df['timestamp'].max())
