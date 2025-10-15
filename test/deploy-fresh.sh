#!/bin/bash
# =============================================================================
# Fresh OP Stack Testnet Deployment
# Function: Deploy a new OP Stack testnet from scratch (no Erigon migration)
# Corresponds to: scripts 1-4 from pre-release branch
# =============================================================================
set -e

source .env

echo "=========================================="
echo "OP Stack Fresh Deployment"
echo "=========================================="
echo ""

# Set deployment mode
export DEPLOYMENT_MODE="fresh"

# Step list
STEPS=(
    "01-setup-l1"              # Start L1 node
    "02-fund-accounts"         # Fund accounts
    "10-deploy-op-contracts"   # Deploy OP contracts (complete flow)
    "14-deploy-cgt"            # Deploy Custom Gas Token (required for migration)
    "30-init-opgeth"           # Initialize op-geth (using genesis.json)
    "40-start-op-services"     # Start op-geth/op-node/batcher
    "41-setup-p2p"             # Setup P2P network
    "31-build-prestate"        # Build op-program prestate (AFTER services start)
    "50-setup-fraud-proof"     # Complete fraud proof flow
)

echo "Deployment steps:"
for i in "${!STEPS[@]}"; do
    echo "   $((i+1)). ${STEPS[$i]}"
done
echo ""

# Execute each step
FAILED_STEP=""
for step in "${STEPS[@]}"; do
    # Skip commented steps
    if [[ "$step" =~ ^#.* ]]; then
        continue
    fi

    STEP_SCRIPT="./steps/${step}.sh"

    if [ ! -f "$STEP_SCRIPT" ]; then
        echo "WARNING: Step script not found: $STEP_SCRIPT (skipping)"
        continue
    fi

    echo ""
    echo "Executing step: $step"
    echo "----------------------------------------"

    if bash "$STEP_SCRIPT"; then
        echo "Step completed: $step"
    else
        echo "ERROR: Step failed: $step"
        FAILED_STEP="$step"
        break
    fi
done

echo ""
echo "=========================================="

if [ -n "$FAILED_STEP" ]; then
    echo "ERROR: Deployment failed at step: $FAILED_STEP"
    echo ""
    echo "Troubleshooting suggestions:"
    echo "   1. Check $FAILED_STEP.sh log output"
    echo "   2. Verify configuration in .env file"
    echo "   3. Check Docker service status: docker ps"
    echo "   4. Check L1 connection: cast block-number --rpc-url \$L1_RPC_URL"
    exit 1
else
    echo "SUCCESS: Fresh OP Stack deployment completed!"
    echo ""
    echo "Deployment info:"
    echo "   L1 RPC: $L1_RPC_URL"
    echo "   L2 RPC: $L2_RPC_URL"
    echo "   Chain ID: $CHAIN_ID"
    echo ""
    echo "Important files:"
    echo "   - config-op/genesis.json       (L2 genesis config)"
    echo "   - config-op/rollup.json        (Rollup config)"
    echo "   - config-op/state.json         (Contract deployment state)"
    echo "   - data/cannon-data/            (Prestate files)"
    echo ""
    echo "Next steps:"
    echo "   1. Check service status: docker ps"
    echo "   2. View logs: docker logs op-geth-seq"
    echo "   3. Test L2: cast block-number --rpc-url \$L2_RPC_URL"
fi

