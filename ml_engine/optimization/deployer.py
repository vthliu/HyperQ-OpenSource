import json
import os
import shutil
import toml
import subprocess
from datetime import datetime

CANDIDATE_PATH = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'data', 'optimization', 'deployment_candidate.json')
DEPLOYMENT_RECORD_PATH = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'data', 'optimization', 'deployment_history.jsonl')
CONFIG_PATH = '/home/vth/Vth/hyperq-rs/config.toml'
CONFIG_DIR = os.path.dirname(CONFIG_PATH)

def update_config_toml(params):
    config = toml.load(CONFIG_PATH)
    
    if 'position' not in config: config['position'] = {}
    config['position']['rwa_risk_multiplier'] = params['rwa_risk_multiplier']
    
    if 'time_stop' not in config: config['time_stop'] = {}
    config['time_stop']['max_holding_hours'] = params['max_holding_hours']
    # Remember to multiply profit_threshold by 100 since we represent it as % in rust
    config['time_stop']['profit_threshold'] = params['time_stop_profit_threshold'] * 100.0
    
    if 'sentinel' not in config: config['sentinel'] = {}
    config['sentinel']['hard_stop_roe'] = params['hard_stop_roe']
    
    if 'prob_threshold' not in config: config['prob_threshold'] = {}
    config['prob_threshold']['layer1'] = params['prob_threshold_layer1']
    config['prob_threshold']['layer2'] = params['prob_threshold_layer2']
    config['prob_threshold']['layer3'] = params['prob_threshold_layer3']
    
    with open(CONFIG_PATH, 'w') as f:
        toml.dump(config, f)

def main():
    print("[DEPLOYER] Starting safe deployment...")
    
    if not os.path.exists(CANDIDATE_PATH):
        print("[DEPLOYER] No deployment candidate found. Exiting.")
        return
        
    with open(CANDIDATE_PATH, 'r') as f:
        candidate = json.load(f)
        
    if candidate.get('deployment_timestamp'):
        print("[DEPLOYER] Candidate has already been deployed. Exiting.")
        return
        
    deployment_id = candidate.get('deployment_id')
    params = candidate['params']
    
    # 1. Backup current config
    backup_path = os.path.join(CONFIG_DIR, f"config.toml.backup_{deployment_id}")
    shutil.copy2(CONFIG_PATH, backup_path)
    print(f"[DEPLOYER] ✅ config.toml backed up to {backup_path}")
    
    # 2. Update config.toml
    update_config_toml(params)
    print("[DEPLOYER] ✅ config.toml updated with new optimized parameters")
    
    # 3. Save deployment record
    candidate['deployment_timestamp'] = datetime.now().isoformat()
    candidate['rollback_triggered'] = False
    
    with open(CANDIDATE_PATH, 'w') as f:
        json.dump(candidate, f, indent=4)
        
    with open(DEPLOYMENT_RECORD_PATH, 'a') as f:
        f.write(json.dumps(candidate) + '\n')
        
    # 4. Restart hyperq.service safely
    # Because hyperq-rs has startup healing, restarting it is 100% safe.
    print("[DEPLOYER] Restarting hyperq.service...")
    result = subprocess.run(["sudo", "systemctl", "restart", "hyperq.service"], capture_output=True, text=True)
    if result.returncode != 0:
        print(f"[DEPLOYER] ❌ Failed to restart hyperq.service: {result.stderr}")
        # Could rollback here, but let rollback_daemon handle it if it crashes
    else:
        print("[DEPLOYER] ✅ hyperq.service restarted successfully.")
        
    # 5. Start rollback daemon window via systemd if possible, but it's triggered by systemd sequential execution
    # For now we'll just print that it's finished. The systemd unit will call rollback_daemon next.
    print(f"[DEPLOYER] Deployment {deployment_id} complete. Handing over to Rollback Daemon.")

if __name__ == "__main__":
    main()
