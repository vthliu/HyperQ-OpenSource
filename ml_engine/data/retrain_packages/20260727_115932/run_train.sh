#!/bin/bash
cd /home/vth/Vth/ml_engine
source venv/bin/activate
python -m training.train_xgb --full
echo 'Training complete. Models are saved in ml_engine/training/models/'
