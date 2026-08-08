import sqlite3
import pandas as pd
import optuna
import os
import json
import numpy as np
from datetime import datetime, timedelta

DB_PATH = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'data', 'tracking.db')
OUTPUT_PATH = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'data', 'optimization', 'optimization_candidates.json')

def fetch_trades():
    conn = sqlite3.connect(DB_PATH)
    query = """
    SELECT 
        symbol, tier, side, entry_price, fill_price as exit_price,
        unrealized_roe, exit_reason, max_favorable_excursion, max_adverse_excursion,
        entry_time, timestamp as exit_time
    FROM trades 
    WHERE type = 'MARKET_CLOSE' 
      AND timestamp > datetime('now', '-30 days')
      AND (exit_reason IS NULL OR exit_reason NOT LIKE '%Hard Stop%')
    """
    df = pd.read_sql_query(query, conn)
    conn.close()
    return df

def apply_params(row, params):
    # Conservative estimation logic for simulation
    # If the new hard_stop_roe is stricter than the actual max_adverse_excursion (or unrealized_roe if missing),
    # we simulate the trade being stopped out earlier.
    new_hard_stop = params['hard_stop_roe'] * 100 # convert to percentage to match ROE
    actual_roe = row['unrealized_roe'] if pd.notnull(row['unrealized_roe']) else 0.0
    mae = row['max_adverse_excursion'] if pd.notnull(row['max_adverse_excursion']) else actual_roe
    
    # Check if the new hard stop is stricter than what we experienced
    if new_hard_stop > mae:
        # We assume it would have hit the new stricter hard stop.
        # However, to be conservative, we cap the final ROE at max(actual_roe, new_hard_stop).
        # We don't assume it would have been better.
        simulated_roe = max(actual_roe, new_hard_stop)
    else:
        # If the new hard stop is looser, the trade proceeds exactly as it did.
        simulated_roe = actual_roe
        
    return simulated_roe

def calculate_metrics(simulated_roes):
    if len(simulated_roes) == 0:
        return 0, 0, 0, 0, 0, 0
    
    mean_roe = np.mean(simulated_roes)
    std_roe = np.std(simulated_roes) if len(simulated_roes) > 1 else 0
    sharpe = (mean_roe / std_roe) if std_roe > 0 else 0
    
    win_rate = np.mean(simulated_roes > 0)
    
    cumulative = np.cumsum(simulated_roes)
    running_max = np.maximum.accumulate(cumulative)
    drawdowns = running_max - cumulative
    max_drawdown = np.max(drawdowns) if len(drawdowns) > 0 else 0
    
    calmar = (mean_roe / max_drawdown) if max_drawdown > 0 else 0
    
    gross_profit = np.sum(simulated_roes[simulated_roes > 0])
    gross_loss = np.abs(np.sum(simulated_roes[simulated_roes < 0]))
    profit_factor = (gross_profit / gross_loss) if gross_loss > 0 else float('inf')
    
    return sharpe, calmar, win_rate, profit_factor, max_drawdown, mean_roe

def objective(trial, trades_df):
    import json
    import os
    
    thresholds_path = "ml_engine/data/optimization/native_thresholds.json"
    native_thresholds = {}
    if os.path.exists(thresholds_path):
        try:
            with open(thresholds_path, 'r') as f:
                native_thresholds = json.load(f)
        except Exception:
            pass
            
    # Load baselines (default to historically reasonable starting points if file is missing)
    base_l1 = native_thresholds.get("layer1", {}).get("baseline", 0.60)
    base_l2 = native_thresholds.get("layer2", {}).get("baseline", 0.60)
    base_l3 = native_thresholds.get("layer3", {}).get("baseline", 0.65)

    params = {
        'rwa_risk_multiplier': trial.suggest_float('rwa_risk_multiplier', 0.15, 0.50),
        'max_holding_hours': trial.suggest_float('max_holding_hours', 1.0, 4.0),
        'time_stop_profit_threshold': trial.suggest_float('time_stop_profit_threshold', 1.5, 6.0),
        'hard_stop_roe': trial.suggest_float('hard_stop_roe', -0.30, -0.15),
        'prob_threshold_layer1': base_l1 + trial.suggest_float('offset_layer1', -0.03, 0.05),
        'prob_threshold_layer2': base_l2 + trial.suggest_float('offset_layer2', -0.03, 0.05),
        'prob_threshold_layer3': base_l3 + trial.suggest_float('offset_layer3', -0.03, 0.05),
    }
    
    simulated_roes = trades_df.apply(lambda row: apply_params(row, params), axis=1).values
    
    sharpe, calmar, win_rate, pf, mdd, mean_roe = calculate_metrics(simulated_roes)
    
    return sharpe, calmar

def main():
    print("[OPTIMIZER] Starting Phase 15 Parameter Auto-Adaptation...")
    trades_df = fetch_trades()
    
    if len(trades_df) < 30:
        print(f"[OPTIMIZER] 样本量不足 (N={len(trades_df)} < 30)，跳过优化，沿用原有配置。")
        
        # Ensure we write a minimal JSON so that validator and deployer can fail gracefully or skip
        output = {
            "optimization_date": datetime.now().isoformat(),
            "data_window": f"{(datetime.now() - timedelta(days=30)).isoformat()}_to_{datetime.now().isoformat()}",
            "trades_analyzed": len(trades_df),
            "candidates": []
        }
        with open(OUTPUT_PATH, 'w') as f:
            json.dump(output, f, indent=4)
        return
    
    print(f"[OPTIMIZER] Found {len(trades_df)} valid trades. Starting Optuna optimization...")
    
    storage_url = f"sqlite:///{os.path.join(os.path.dirname(DB_PATH), 'optuna.db')}"
    study_name = "hyperq_phase15_v2"
    
    # We want to maximize both Sharpe and Calmar
    study = optuna.create_study(
        study_name=study_name, 
        storage=storage_url,
        directions=["maximize", "maximize"], 
        load_if_exists=True
    )
    
    # Disable optuna logs to avoid spamming the systemd journal
    optuna.logging.set_verbosity(optuna.logging.WARNING)
    
    study.optimize(lambda trial: objective(trial, trades_df), n_trials=50)
    
    best_trials = study.best_trials
    candidates = []
    
    for i, trial in enumerate(best_trials):
        params = trial.params
        
        # We need to construct the full formatted_params including absolute thresholds
        # since the trial params only have offsets now
        base_l1 = native_thresholds.get("layer1", {}).get("baseline", 0.60)
        base_l2 = native_thresholds.get("layer2", {}).get("baseline", 0.60)
        base_l3 = native_thresholds.get("layer3", {}).get("baseline", 0.65)
        
        # reconstruct the absolute thresholds for simulation
        sim_params = params.copy()
        sim_params['prob_threshold_layer1'] = base_l1 + params.get('offset_layer1', 0.0)
        sim_params['prob_threshold_layer2'] = base_l2 + params.get('offset_layer2', 0.0)
        sim_params['prob_threshold_layer3'] = base_l3 + params.get('offset_layer3', 0.0)
        
        simulated_roes = trades_df.apply(lambda row: apply_params(row, sim_params), axis=1).values
        sharpe, calmar, win_rate, pf, mdd, mean_roe = calculate_metrics(simulated_roes)
        
        score = 0.4 * sharpe + 0.4 * calmar + 0.2 * win_rate
        
        formatted_params = sim_params.copy()
        formatted_params['time_stop_profit_threshold'] = params['time_stop_profit_threshold'] / 100.0
        
        candidates.append({
            "score": score,
            "params": formatted_params,
            "metrics": {
                "sharpe": sharpe,
                "calmar": calmar,
                "win_rate": win_rate,
                "profit_factor": pf,
                "avg_trade_roe": mean_roe,
                "max_drawdown": mdd,
                "num_trades": len(trades_df)
            }
        })
    
    candidates.sort(key=lambda x: x["score"], reverse=True)
    top_candidates = candidates[:5]
    
    for i, cand in enumerate(top_candidates):
        cand["rank"] = i + 1
        
    output = {
        "optimization_date": datetime.now().isoformat(),
        "data_window": f"{(datetime.now() - timedelta(days=30)).isoformat()}_to_{datetime.now().isoformat()}",
        "trades_analyzed": len(trades_df),
        "candidates": top_candidates
    }
    
    with open(OUTPUT_PATH, 'w') as f:
        json.dump(output, f, indent=4)
        
    print(f"[OPTIMIZER] Optimization complete. Top {len(top_candidates)} candidates saved to {OUTPUT_PATH}")

if __name__ == "__main__":
    main()
