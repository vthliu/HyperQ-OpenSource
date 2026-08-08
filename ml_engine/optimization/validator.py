import json
import os
import toml
import numpy as np
from datetime import datetime
from .auto_optimizer import fetch_trades, apply_params, calculate_metrics

CANDIDATES_PATH = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'data', 'optimization', 'optimization_candidates.json')
OUTPUT_PATH = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'data', 'optimization', 'deployment_candidate.json')
CONFIG_PATH = '/home/vth/Vth/hyperq-rs/config.toml'

def get_current_params():
    config = toml.load(CONFIG_PATH)
    return {
        'rwa_risk_multiplier': config.get('position', {}).get('rwa_risk_multiplier', 0.3),
        'max_holding_hours': config.get('time_stop', {}).get('max_holding_hours', 2.0),
        'time_stop_profit_threshold': config.get('time_stop', {}).get('profit_threshold', 3.0) / 100.0,
        'hard_stop_roe': config.get('sentinel', {}).get('hard_stop_roe', -0.20),
        'prob_threshold_layer1': config.get('prob_threshold', {}).get('layer1', 0.60),
        'prob_threshold_layer2': config.get('prob_threshold', {}).get('layer2', 0.60),
        'prob_threshold_layer3': config.get('prob_threshold', {}).get('layer3', 0.70)
    }

def split_into_rolling_windows(df, num_windows=3):
    if len(df) == 0:
        return []
    
    # Sort by time
    df_sorted = df.sort_values(by='exit_time')
    
    # Simple array split using iloc to keep it as DataFrame
    chunk_size = max(1, len(df_sorted) // num_windows)
    windows = []
    for i in range(num_windows):
        start_idx = i * chunk_size
        end_idx = (i + 1) * chunk_size if i < num_windows - 1 else len(df_sorted)
        windows.append(df_sorted.iloc[start_idx:end_idx])
    return windows

def calculate_score(params, trades_df):
    simulated_roes = trades_df.apply(lambda row: apply_params(row, params), axis=1).values
    sharpe, calmar, win_rate, pf, mdd, mean_roe = calculate_metrics(simulated_roes)
    return sharpe, calmar

def main():
    print("[VALIDATOR] Starting candidate validation...")
    
    if not os.path.exists(CANDIDATES_PATH):
        print("[VALIDATOR] No candidates file found. Exiting.")
        return
        
    with open(CANDIDATES_PATH, 'r') as f:
        data = json.load(f)
        
    candidates = data.get('candidates', [])
    if not candidates:
        print("[VALIDATOR] No candidates to validate. Exiting.")
        return
        
    trades_df = fetch_trades()
    current_params = get_current_params()
    
    curr_sharpe, curr_calmar = calculate_score(current_params, trades_df)
    print(f"[VALIDATOR] Current Params Baseline -> Sharpe: {curr_sharpe:.4f}, Calmar: {curr_calmar:.4f}")
    
    valid_candidates = []
    
    for cand in candidates:
        params = cand['params']
        
        # 1. Backtest
        cand_sharpe, cand_calmar = calculate_score(params, trades_df)
        
        # 2. Threshold Check
        if cand_sharpe < curr_sharpe * 0.95:
            print(f"Candidate {cand['rank']} rejected: Sharpe {cand_sharpe:.4f} < {curr_sharpe * 0.95:.4f} (-5% drop)")
            continue
            
        if cand_calmar < curr_calmar * 0.90:
            print(f"Candidate {cand['rank']} rejected: Calmar {cand_calmar:.4f} < {curr_calmar * 0.90:.4f} (-10% drop)")
            continue
            
        # 3. Robustness check: Rolling windows
        windows = split_into_rolling_windows(trades_df, 3)
        window_sharpes = []
        for w in windows:
            w_sharpe, _ = calculate_score(params, w)
            window_sharpes.append(w_sharpe)
            
        if not window_sharpes:
            continue
            
        max_diff = max(window_sharpes) - min(window_sharpes)
        if max_diff > 0.3:
            print(f"Candidate {cand['rank']} rejected: Unstable across windows (max diff {max_diff:.4f} > 0.3). Window sharpes: {window_sharpes}")
            continue
            
        cand['validation_metrics'] = {
            "sharpe_old": curr_sharpe,
            "sharpe_new": cand_sharpe,
            "calmar_old": curr_calmar,
            "calmar_new": cand_calmar,
            "rolling_window_sharpes": window_sharpes,
            "validation_passed": True
        }
        
        valid_candidates.append(cand)
        
    if not valid_candidates:
        print("[VALIDATOR] No candidates passed validation. Keeping current config.")
        return
        
    # Pick the best one among the valid ones based on score
    best_candidate = max(valid_candidates, key=lambda x: x['score'])
    
    output = {
        "deployment_id": datetime.now().strftime("%Y%m%d_%H%M%S"),
        "candidate_rank": best_candidate['rank'],
        "params": best_candidate['params'],
        "validation_results": best_candidate['validation_metrics'],
        "deployment_timestamp": None,
        "rollback_triggered": False
    }
    
    with open(OUTPUT_PATH, 'w') as f:
        json.dump(output, f, indent=4)
        
    print(f"[VALIDATOR] Successfully validated Candidate {best_candidate['rank']}. Output saved to {OUTPUT_PATH}")

if __name__ == "__main__":
    main()
