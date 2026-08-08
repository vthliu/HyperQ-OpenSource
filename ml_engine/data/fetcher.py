import ccxt
import pandas as pd
import time
import argparse
import yaml
import os
import sys
sys.path.append(os.path.dirname(__file__))
import json
from concurrent.futures import ThreadPoolExecutor, as_completed
from storage import save_raw_klines, load_raw_klines
from datetime import datetime, timezone

def fetch_historical_klines(symbol: str, timeframe: str, since_ms: int, limit: int = 1000) -> pd.DataFrame:
    """Fetch klines from Binance Futures using ccxt with pagination."""
    exchange = ccxt.binanceusdm({
        'enableRateLimit': True,
    })
    
    all_ohlcv = []
    current_since = since_ms
    
    print(f"Fetching {symbol} {timeframe} starting from {datetime.fromtimestamp(since_ms/1000, tz=timezone.utc)}")
    
    while True:
        try:
            # Use raw fapi endpoint to get extended fields
            params = {'symbol': symbol, 'interval': exchange.timeframes[timeframe], 'startTime': current_since, 'limit': limit}
            ohlcv = exchange.fapiPublicGetKlines(params)
            if not ohlcv:
                break
            
            # Format is: [Open time, Open, High, Low, Close, Volume, Close time, Quote asset volume, Number of trades, Taker buy base asset volume, Taker buy quote asset volume, Ignore]
            parsed_ohlcv = [
                [
                    int(c[0]),          # timestamp
                    float(c[1]),        # open
                    float(c[2]),        # high
                    float(c[3]),        # low
                    float(c[4]),        # close
                    float(c[5]),        # volume
                    float(c[7]),        # quote_asset_volume
                    int(c[8]),          # number_of_trades
                    float(c[10])        # taker_buy_quote_asset_volume
                ] for c in ohlcv
            ]
            all_ohlcv.extend(parsed_ohlcv)
            
            # The last candle's timestamp + 1 to fetch the next batch
            last_ts = parsed_ohlcv[-1][0]
            if last_ts <= current_since:
                current_since = last_ts + 1
            else:
                current_since = last_ts + 1
                
            print(f"Fetched {len(parsed_ohlcv)} candles. Last timestamp: {datetime.fromtimestamp(last_ts/1000, tz=timezone.utc)}")
            
            if len(parsed_ohlcv) < limit:
                break
                
            time.sleep(exchange.rateLimit / 1000)
            
        except Exception as e:
            print(f"Error fetching data for {symbol}: {e}")
            time.sleep(5)
            
    df = pd.DataFrame(all_ohlcv, columns=[
        'timestamp', 'open', 'high', 'low', 'close', 'volume',
        'quote_asset_volume', 'number_of_trades', 'taker_buy_quote_asset_volume'
    ])
    
    return df

def fetch_for_symbol(symbol: str, timeframe: str, start_ms: int, raw_dir: str, update_mode: bool):
    done_file = os.path.join(raw_dir, f"{symbol}_{timeframe}.done")
    if not update_mode and os.path.exists(done_file):
        print(f"[{symbol}] Already fetched completely. Skipping (found .done file).")
        return
        
    since_ms = start_ms
    if update_mode:
        existing_df = load_raw_klines(symbol, timeframe, raw_dir)
        if existing_df is not None and not existing_df.empty:
            since_ms = int(existing_df['timestamp'].max())
            print(f"[{symbol}] Updating from last timestamp {datetime.fromtimestamp(since_ms/1000, tz=timezone.utc)}")
            
    df = fetch_historical_klines(symbol, timeframe, since_ms)
    if not df.empty:
        save_raw_klines(df, symbol, timeframe, raw_dir)
        # Mark as done
        with open(done_file, 'w') as f:
            f.write("DONE")
    else:
        print(f"[{symbol}] No new data fetched.")

def main():
    parser = argparse.ArgumentParser(description="Fetch Binance Futures Klines")
    parser.add_argument("--update", action="store_true", help="Update existing data to current time")
    parser.add_argument("--all", action="store_true", help="Fetch all MATURE symbols from registry")
    parser.add_argument("--concurrency", type=int, default=10, help="Number of concurrent workers")
    parser.add_argument("--slow", action="store_true", help="Run strictly sequentially with a 10s delay between symbols (to prevent API bans)")
    args = parser.parse_args()
    
    config_path = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'config.yaml')
    with open(config_path, 'r') as f:
        config = yaml.safe_load(f)
        
    timeframe_mapping = config['data'].get('timeframe_mapping', {})
    default_timeframe = config['data'].get('timeframe', '1h')
    start_date_str = config['data']['start_date']
    raw_dir = os.path.join(os.path.dirname(os.path.dirname(__file__)), config['data']['raw_dir'])
    os.makedirs(raw_dir, exist_ok=True)
    
    registry_path = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'data', 'symbol_registry.json')
    with open(registry_path, 'r') as f:
        registry = json.load(f)
    
    default_start_ms = int(datetime.strptime(start_date_str, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc).timestamp() * 1000)
    
    if args.all:
        symbols = [s for s, info in registry.get("symbols", {}).items() if info.get("status") == "MATURE" and not info.get("data_insufficient", False)]
    else:
        symbols = config['data']['symbols']
        
    print(f"Starting fetch for {len(symbols)} symbols...")
    
    if args.slow:
        print("SLOW MODE ENABLED: Running sequentially with 10s delay.")
        for symbol in symbols:
            try:
                tier = registry.get("symbols", {}).get(symbol, {}).get("tier", "layer3")
                sym_timeframe = timeframe_mapping.get(tier, default_timeframe)
                fetch_for_symbol(symbol, sym_timeframe, default_start_ms, raw_dir, args.update)
                print(f"[{symbol}] Completed. Sleeping for 10 seconds...")
                time.sleep(10)
            except Exception as e:
                print(f"[{symbol}] Fetch failed with exception: {e}")
                time.sleep(10)
    else:
        print(f"Concurrency {args.concurrency}...")
        with ThreadPoolExecutor(max_workers=args.concurrency) as executor:
            futures = {}
            for s in symbols:
                tier = registry.get("symbols", {}).get(s, {}).get("tier", "layer3")
                sym_timeframe = timeframe_mapping.get(tier, default_timeframe)
                future = executor.submit(fetch_for_symbol, s, sym_timeframe, default_start_ms, raw_dir, args.update)
                futures[future] = s
            
            for future in as_completed(futures):
                symbol = futures[future]
                try:
                    future.result()
                except Exception as e:
                    print(f"[{symbol}] Fetch failed with exception: {e}")

if __name__ == "__main__":
    main()
