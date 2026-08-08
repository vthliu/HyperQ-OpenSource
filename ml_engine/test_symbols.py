import ccxt
import pandas as pd
from datetime import datetime
pd.set_option('display.max_columns', None)
exchange = ccxt.binanceusdm({'enableRateLimit': True})
for symbol in ['BANKUSDT', 'AKEUSDT']:
    try:
        ohlcv = exchange.fapiPublicGetKlines({'symbol': symbol, 'interval': '1h', 'limit': 2})
        df = pd.DataFrame(ohlcv)
        print(f"--- {symbol} ---")
        print(df)
    except Exception as e:
        print(f"Error for {symbol}: {e}")
