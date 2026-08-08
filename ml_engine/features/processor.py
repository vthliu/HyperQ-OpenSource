import pandas as pd
import pandas_ta as ta
import numpy as np

def add_lag_features(df: pd.DataFrame) -> pd.DataFrame:
    """Add time-series lag features to capture momentum and autocorrelation."""
    # 1. Returns lag features (past 1, 2, 3, 5 periods)
    df['returns'] = df['close'].pct_change()
    for lag in [1, 2, 3, 5]:
        df[f'returns_lag_{lag}'] = df['returns'].shift(lag)
        
    # 2. High-Low ratio (volatility/amplitude) lag
    df['high_low_ratio'] = df['high'] / df['low']
    df['high_low_ratio_lag_1'] = df['high_low_ratio'].shift(1)
    
    return df

def transform(df: pd.DataFrame, timeframe: str = "1h") -> pd.DataFrame:
    """
    Main feature engineering pipeline.
    This function is SHARED perfectly between Training and Inference.
    """
    # Ensure correct types
    df = df.copy()
    
    if 'timestamp' in df.columns and not isinstance(df.index, pd.DatetimeIndex):
        df.index = pd.to_datetime(df['timestamp'], unit='ms')
    elif 'ts' in df.columns and not isinstance(df.index, pd.DatetimeIndex):
        df.index = pd.to_datetime(df['ts'], unit='ms')
    
    # Generate technical indicators using pandas-ta extension methods directly
    df.ta.sma(length=5, append=True)
    df.ta.sma(length=20, append=True)
    df.ta.sma(length=50, append=True)
    df.ta.ema(length=12, append=True)
    df.ta.ema(length=26, append=True)
    df.ta.macd(fast=12, slow=26, signal=9, append=True)
    df.ta.rsi(length=14, append=True)
    df.ta.stochrsi(length=14, append=True)
    df.ta.bbands(length=20, std=2, append=True)
    df.ta.atr(length=14, append=True)
    df.ta.adx(length=14, append=True)
    # Fix pandas_ta CCI operator-precedence bug:
    # pandas_ta computes: cci = tp - sma_tp/(c*mad)  (missing parentheses)
    # Correct formula:    cci = (tp - sma_tp) / (c * mad)
    n_cci = 20
    c_cci = 0.015
    tp = (df['high'] + df['low'] + df['close']) / 3
    sma_tp = tp.rolling(n_cci).mean()
    mad_tp = tp.rolling(n_cci).apply(lambda x: np.fabs(x - x.mean()).mean(), raw=True)
    df['CCI_20_0.015'] = (tp - sma_tp) / (c_cci * mad_tp)
    df.ta.obv(append=True)
    df.ta.mfi(length=14, append=True)
    df.ta.roc(length=10, append=True)
    df.ta.willr(length=14, append=True)
    df.ta.vwap(append=True)
    df.ta.cmf(length=20, append=True)
    
    # Add simple distance from MAs
    if 'SMA_20' in df.columns:
        df['dist_sma_20'] = (df['close'] - df['SMA_20']) / df['SMA_20']
    if 'EMA_26' in df.columns:
        df['dist_ema_26'] = (df['close'] - df['EMA_26']) / df['EMA_26']
        
    # ==========================================
    # Layer 3 / Low-Liquidity Proxy Features
    # ==========================================
    # 1. Volume Surge Ratio
    if 'volume' in df.columns:
        vol_sma_20 = df['volume'].rolling(20).mean()
        df['volume_surge_ratio'] = df['volume'] / (vol_sma_20 + 1e-9)
        
    # Mean-Reversion & Exhaustion Features (Anti-FOMO)
    # Pump Exhaustion Index: Current price vs the 12-period lowest low
    df['pump_exhaustion_index'] = (df['close'] - df['low'].rolling(12).min()) / (df['low'].rolling(12).min() + 1e-9)
    
    # BBand Overextension: How far above the upper bollinger band
    if 'BBU_20_2.0_2.0' in df.columns:
        df['bband_overextension'] = (df['close'] - df['BBU_20_2.0_2.0']) / (df['BBU_20_2.0_2.0'] + 1e-9)
    else:
        df['bband_overextension'] = 0.0
        
    # Wick Ratios: Detect rejection at local tops/bottoms
    candle_range = df['high'] - df['low'] + 1e-9
    candle_body_top = df[['open', 'close']].max(axis=1)
    candle_body_bottom = df[['open', 'close']].min(axis=1)
    df['upper_wick_ratio'] = (df['high'] - candle_body_top) / candle_range
    df['lower_wick_ratio'] = (candle_body_bottom - df['low']) / candle_range
        
    # 2. Price Tick Velocity (Acceleration over 5 periods)
    df['price_tick_velocity'] = (df['close'] - df['close'].shift(5)) / (df['close'].shift(5) + 1e-9) / 5.0
    
    # Check if extended fields exist (for older datasets they might not)
    if 'quote_asset_volume' in df.columns and 'number_of_trades' in df.columns and 'taker_buy_quote_asset_volume' in df.columns:
        # 3. Trade Imbalance: (Taker Buy - Taker Sell) / Total Vol
        # taker_sell = total - taker_buy
        taker_sell = df['quote_asset_volume'] - df['taker_buy_quote_asset_volume']
        df['trade_imbalance'] = (df['taker_buy_quote_asset_volume'] - taker_sell) / (df['quote_asset_volume'] + 1e-9)
        
        # 4. Avg Trade Size & Whale Alert Score
        df['avg_trade_size'] = df['quote_asset_volume'] / (df['number_of_trades'] + 1e-9)
        avg_ts_ema_20 = df['avg_trade_size'].ewm(span=20, adjust=False).mean()
        df['whale_alert_score'] = df['avg_trade_size'] / (avg_ts_ema_20 + 1e-9)
        
        # 5. Liquidity Score
        df['liquidity_score'] = np.log1p(df['quote_asset_volume'])
    else:
        # Fallback for old data without extended fields
        df['trade_imbalance'] = 0.0
        df['avg_trade_size'] = 0.0
        df['whale_alert_score'] = 1.0
        df['liquidity_score'] = 0.0
        
    # Add lag features (Crucial!)
    df = add_lag_features(df)
        
    # Drop rows with NaN values created by window functions (e.g. SMA_50 requires 50 rows)
    # Only drop na if we are not doing a real-time single-row inference (Inference Daemon handles this by passing a window)
    return df
