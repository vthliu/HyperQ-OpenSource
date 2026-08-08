import os
import json
import sqlite3
import time
from datetime import datetime

DB_PATH = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'data', 'tracking.db')
TRADES_LOG = '/var/log/hyperq/trades.jsonl'
PNL_LOG = '/var/log/hyperq/pnl.jsonl'
CURSOR_FILE = os.path.join(os.path.dirname(__file__), '.importer_cursors.json')

def load_cursors():
    if os.path.exists(CURSOR_FILE):
        with open(CURSOR_FILE, 'r') as f:
            return json.load(f)
    return {"trades_pos": 0, "pnl_pos": 0}

def save_cursors(cursors):
    with open(CURSOR_FILE, 'w') as f:
        json.dump(cursors, f)

def import_trades(conn, cursors):
    if not os.path.exists(TRADES_LOG):
        return
        
    pos = cursors["trades_pos"]
    file_size = os.path.getsize(TRADES_LOG)
    
    if pos > file_size: # Log rotated or truncated
        pos = 0
        
    if pos == file_size:
        return
        
    cursor = conn.cursor()
    new_records = 0
    
    with open(TRADES_LOG, 'r') as f:
        f.seek(pos)
        for line in f:
            if not line.strip(): continue
            try:
                data = json.loads(line)
                cursor.execute('''
                    INSERT INTO trades (
                        timestamp, symbol, side, type, qty, expected_price, fill_price, slippage_pct,
                        unrealized_roe, max_favorable_excursion, max_adverse_excursion,
                        entry_time, entry_price, exit_reason
                    )
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                ''', (
                    data.get("timestamp"),
                    data.get("symbol"),
                    data.get("side"),
                    data.get("type"),
                    data.get("qty"),
                    data.get("expected_price"),
                    data.get("fill_price"),
                    data.get("slippage_pct"),
                    data.get("unrealized_roe"),
                    data.get("max_favorable_excursion"),
                    data.get("max_adverse_excursion"),
                    data.get("entry_time"),
                    data.get("entry_price"),
                    data.get("exit_reason")
                ))
                new_records += 1
            except Exception as e:
                print(f"Error parsing trades log: {e}")
                
        cursors["trades_pos"] = f.tell()
        
    conn.commit()
    if new_records > 0:
        print(f"Imported {new_records} new trades")

def import_pnl(conn, cursors):
    if not os.path.exists(PNL_LOG):
        return
        
    pos = cursors["pnl_pos"]
    file_size = os.path.getsize(PNL_LOG)
    
    if pos > file_size: # Log rotated or truncated
        pos = 0
        
    if pos == file_size:
        return
        
    cursor = conn.cursor()
    new_records = 0
    
    with open(PNL_LOG, 'r') as f:
        f.seek(pos)
        for line in f:
            if not line.strip(): continue
            try:
                data = json.loads(line)
                cursor.execute('''
                    INSERT INTO pnl_snapshots (timestamp, symbol, unrealized_roe, entry_price, mark_price)
                    VALUES (?, ?, ?, ?, ?)
                ''', (
                    data.get("timestamp"),
                    data.get("symbol"),
                    data.get("unrealized_roe"),
                    data.get("entry_price"),
                    data.get("mark_price")
                ))
                new_records += 1
            except Exception as e:
                print(f"Error parsing pnl log: {e}")
                
        cursors["pnl_pos"] = f.tell()
        
    conn.commit()
    if new_records > 0:
        print(f"Imported {new_records} new pnl snapshots")

def main():
    print(f"[{datetime.now().isoformat()}] Starting log importer...")
    conn = sqlite3.connect(DB_PATH)
    
    try:
        while True:
            cursors = load_cursors()
            import_trades(conn, cursors)
            import_pnl(conn, cursors)
            save_cursors(cursors)
            
            time.sleep(10) # Run every 10 seconds
    except KeyboardInterrupt:
        print("Stopping log importer...")
    finally:
        conn.close()

if __name__ == "__main__":
    main()
