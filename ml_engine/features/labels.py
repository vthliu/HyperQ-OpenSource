import pandas as pd
import numpy as np

def triple_barrier_labels(
    df: pd.DataFrame, 
    horizon: int = 8, 
    tp_atr_multiplier: float = 1.5, 
    sl_atr_multiplier: float = 1.0
) -> pd.DataFrame:
    """
    Triple-Barrier Method for labeling financial time series using ATR.
    
    Upper Barrier: Take profit (ATR * tp_atr_multiplier)
    Lower Barrier: Stop loss (ATR * sl_atr_multiplier)
    Time Barrier: Maximum holding periods (horizon)
    
    Returns:
    Dataframe with 'label_long' and 'label_short' columns:
    1 = Hit Take Profit before Stop Loss
    0 = Hit Stop Loss first, or hit Time Barrier
    """
    df = df.copy()
    labels_long = np.zeros(len(df))
    labels_short = np.zeros(len(df))
    
    closes = df['close'].values
    highs = df['high'].values
    lows = df['low'].values
    
    # Check if ATR exists, if not use a fallback 2% estimation
    atrs = df['ATRr_14'].values if 'ATRr_14' in df.columns else closes * 0.02
    
    for i in range(len(df)):
        if i + horizon >= len(df):
            # Not enough data for the horizon, leave as NaN or drop later
            labels_long[i] = np.nan
            labels_short[i] = np.nan
            continue
            
        entry_price = closes[i]
        current_atr = atrs[i]
        
        # If ATR is nan, fallback to 2%
        if np.isnan(current_atr):
            current_atr = entry_price * 0.02
        
        # Long Barriers
        long_tp_price = entry_price + (current_atr * tp_atr_multiplier)
        long_sl_price = entry_price - (current_atr * sl_atr_multiplier)
        
        # Short Barriers (Symmetric)
        short_tp_price = entry_price - (current_atr * tp_atr_multiplier)
        short_sl_price = entry_price + (current_atr * sl_atr_multiplier)
        
        hit_long_tp = False
        hit_long_sl = False
        
        hit_short_tp = False
        hit_short_sl = False
        
        # Look ahead for Long
        for j in range(1, horizon + 1):
            idx = i + j
            if idx >= len(df):
                break
                
            future_high = highs[idx]
            future_low = lows[idx]
            
            if future_low <= long_sl_price:
                hit_long_sl = True
                break
                
            if future_high >= long_tp_price:
                hit_long_tp = True
                break
                
        # Look ahead for Short
        for j in range(1, horizon + 1):
            idx = i + j
            if idx >= len(df):
                break
                
            future_high = highs[idx]
            future_low = lows[idx]
            
            if future_high >= short_sl_price:
                hit_short_sl = True
                break
                
            if future_low <= short_tp_price:
                hit_short_tp = True
                break
                
        if hit_long_tp and not hit_long_sl:
            labels_long[i] = 1
            
        if hit_short_tp and not hit_short_sl:
            labels_short[i] = 1
            
    df['label_long'] = labels_long
    df['label_short'] = labels_short
    return df
