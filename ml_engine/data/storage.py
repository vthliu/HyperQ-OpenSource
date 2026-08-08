import os
import pandas as pd
import pyarrow as pa
import pyarrow.parquet as pq
from typing import Optional

def save_raw_klines(df: pd.DataFrame, symbol: str, timeframe: str, raw_dir: str):
    """
    Save klines to parquet file. Appends if file exists and deduplicates.
    """
    os.makedirs(raw_dir, exist_ok=True)
    file_path = os.path.join(raw_dir, f"{symbol}_{timeframe}.parquet")
    
    if os.path.exists(file_path):
        existing_df = pd.read_parquet(file_path)
        combined = pd.concat([existing_df, df])
        # Drop duplicates based on timestamp (keep last)
        combined = combined.drop_duplicates(subset=['timestamp'], keep='last')
        combined = combined.sort_values('timestamp')
        combined.to_parquet(file_path, index=False)
        print(f"Updated {file_path}. Total rows: {len(combined)}")
    else:
        df = df.sort_values('timestamp')
        df.to_parquet(file_path, index=False)
        print(f"Created {file_path}. Total rows: {len(df)}")

def load_raw_klines(symbol: str, timeframe: str, raw_dir: str) -> Optional[pd.DataFrame]:
    """
    Load klines from parquet file.
    """
    file_path = os.path.join(raw_dir, f"{symbol}_{timeframe}.parquet")
    if os.path.exists(file_path):
        return pd.read_parquet(file_path)
    return None
