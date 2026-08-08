import json
import yaml

with open('/home/vth/Vth/ml_engine/config.yaml', 'r') as f:
    config = yaml.safe_load(f)
layer3_assets = config.get('inference', {}).get('layer3_assets', [])

l12_trades = []
l3_trades = []

with open('/var/log/hyperq/trades.jsonl', 'r') as f:
    for line in f:
        data = json.loads(line)
        if data.get('type') == 'MARKET_CLOSE':
            sym = data['symbol']
            roe = data.get('unrealized_roe')
            if roe is not None:
                if sym in layer3_assets:
                    l3_trades.append(roe)
                else:
                    l12_trades.append(roe)

def print_metrics(name, trades):
    # take last 20
    trades = trades[-20:]
    if not trades:
        print(f"{name}: No closed trades found.")
        return
        
    wins = [r for r in trades if r > 0]
    losses = [r for r in trades if r <= 0]
    
    win_rate = len(wins) / len(trades)
    avg_roe = sum(trades) / len(trades)
    avg_win = sum(wins) / len(wins) if wins else 0
    avg_loss = sum(losses) / len(losses) if losses else 0
    
    print(f"{name} (last {len(trades)} trades):")
    print(f"  Win Rate: {win_rate*100:.1f}%")
    print(f"  Avg ROE:  {avg_roe:.2f}%")
    print(f"  Avg Win:  {avg_win:.2f}%")
    print(f"  Avg Loss: {avg_loss:.2f}%")

print_metrics("Layer 1/2", l12_trades)
print_metrics("Layer 3", l3_trades)

