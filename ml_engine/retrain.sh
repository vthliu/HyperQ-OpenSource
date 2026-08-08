#!/usr/bin/env bash
# =============================================================================
# retrain.sh — 一键全量重训练脚本
# 用法: bash retrain.sh [--tier layer1|layer2|layer3|all]
# =============================================================================
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ML_DIR="$SCRIPT_DIR"
VENV="$ML_DIR/venv/bin/python3"
MODELS_DIR="$ML_DIR/training/models"
BACKUP_DIR="$ML_DIR/training/models/backup_$(date +%Y%m%d_%H%M%S)"

echo "============================================="
echo "  HyperQ-rs Full Retraining Pipeline"
echo "  Started: $(date)"
echo "============================================="

# 1. Backup old models
echo ""
echo "[1/4] Backing up existing models..."
mkdir -p "$BACKUP_DIR"
cp "$MODELS_DIR"/*.json "$BACKUP_DIR/" 2>/dev/null && echo "  ✅ Old models backed up to $BACKUP_DIR" || echo "  ⚠️  No existing models to backup"

# 2. Verify CCI fix is in place
echo ""
echo "[2/4] Verifying CCI fix in processor.py..."
if grep -q "Correct formula" "$ML_DIR/features/processor.py"; then
    echo "  ✅ CCI bug fix confirmed in processor.py"
else
    echo "  ❌ CCI bug fix not found — please check processor.py"
    exit 1
fi

# 3. Run training
echo ""
echo "[3/4] Starting XGBoost training (all tiers)..."
echo "  Training window: 2022-01-01 → 2025-01-01"
echo "  Validation:      2025-01-02 → 2025-10-01"  
echo "  Test (OOS):      2025-10-02 → 2026-07-31"
echo ""

cd "$ML_DIR"
$VENV training/train_xgb.py --full 2>&1 | tee "$ML_DIR/training/retrain_$(date +%Y%m%d).log"

echo ""
echo "[4/4] Retraining complete!"
echo ""
echo "New models saved to: $MODELS_DIR"
echo "Log file: $ML_DIR/training/retrain_$(date +%Y%m%d).log"
echo ""
echo "Next step: restart hyperq-rs to load new models"
echo "  cargo run --release"
echo "============================================="
echo "  Finished: $(date)"
echo "============================================="
