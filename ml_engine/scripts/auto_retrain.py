import os
import subprocess
import shutil
import time

def auto_retrain_if_needed():
    # In a fully fleshed out system, this would check symbol data counts or schedule.
    print("Initiating automated retraining cycle...")
    
    # 1. Stop Inference Daemon
    print("Stopping hyperq-ml.service...")
    subprocess.run(["sudo", "systemctl", "stop", "hyperq-ml.service"])
    
    # Backup old model
    models_dir = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'training', 'models')
    model_path = os.path.join(models_dir, 'model_v1.json')
    backup_path = os.path.join(models_dir, 'model_v1_backup.json')
    
    if os.path.exists(model_path):
        shutil.copy(model_path, backup_path)
        
    train_script = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'training', 'train_xgb.py')
    
    # 2. Run Training
    print("Running training...")
    try:
        # Run training which outputs to model_v1.json
        result = subprocess.run(["python3", train_script], check=True, capture_output=True, text=True)
        print("Training completed successfully.")
        # TODO: Parse AUC from result.stdout and compare with old AUC for graceful rollback.
        # For now, if it succeeds without throwing, we consider it deployed.
    except subprocess.CalledProcessError as e:
        print(f"Training failed! Rolling back...\n{e.stderr}")
        if os.path.exists(backup_path):
            shutil.copy(backup_path, model_path)
            
    # 3. Start Inference Daemon
    print("Restarting hyperq-ml.service...")
    subprocess.run(["sudo", "systemctl", "start", "hyperq-ml.service"])
    print("Automated retraining cycle completed.")

if __name__ == "__main__":
    auto_retrain_if_needed()
