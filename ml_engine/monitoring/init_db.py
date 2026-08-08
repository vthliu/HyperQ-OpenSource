import sqlite3
import os

def init_db():
    db_path = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'data', 'tracking.db')
    
    conn = sqlite3.connect(db_path)
    cursor = conn.cursor()
    
    # 1. Predictions table
    cursor.execute('''
    CREATE TABLE IF NOT EXISTS predictions (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        timestamp DATETIME NOT NULL,
        symbol TEXT NOT NULL,
        prob REAL NOT NULL,
        tier TEXT NOT NULL,
        mark_price REAL NOT NULL
    )
    ''')
    
    # 2. Trades table
    cursor.execute('''
    CREATE TABLE IF NOT EXISTS trades (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        timestamp DATETIME NOT NULL,
        symbol TEXT NOT NULL,
        side TEXT NOT NULL,
        type TEXT NOT NULL,
        qty REAL NOT NULL,
        expected_price REAL NOT NULL,
        fill_price REAL,
        slippage_pct REAL,
        tier TEXT
    )
    ''')
    
    # 3. PnL Snapshots table
    cursor.execute('''
    CREATE TABLE IF NOT EXISTS pnl_snapshots (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        timestamp DATETIME NOT NULL,
        symbol TEXT NOT NULL,
        unrealized_roe REAL NOT NULL,
        entry_price REAL NOT NULL,
        mark_price REAL NOT NULL
    )
    ''')
    
    # Create indexes for faster queries
    cursor.execute('CREATE INDEX IF NOT EXISTS idx_pred_ts ON predictions(timestamp)')
    cursor.execute('CREATE INDEX IF NOT EXISTS idx_trades_ts ON trades(timestamp)')
    cursor.execute('CREATE INDEX IF NOT EXISTS idx_pnl_ts ON pnl_snapshots(timestamp)')
    
    conn.commit()
    conn.close()
    
    print(f"Tracking database initialized successfully at {db_path}")

if __name__ == "__main__":
    init_db()
