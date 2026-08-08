import pandas as pd
import os
import sys

# Append parent dir to path
sys.path.append(os.path.dirname(os.path.dirname(__file__)))
from features.processor import transform

def get_latest_features(df: pd.DataFrame, timeframe: str = "1h") -> pd.Series:
    """
    Takes a dataframe of the last N candles (e.g., 200).
    Applies the exact same transform() used in training.
    Returns the features for the latest (most recent) candle.
    """
    # 1. Apply shared feature engineering
    df_features = transform(df, timeframe)
    
    # 2. Get the last row (the most recent candle)
    latest_row = df_features.iloc[-1]
    
    return latest_row
