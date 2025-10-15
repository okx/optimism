#!/bin/bash
# =============================================================================
# Migration from Erigon to OP Stack (Migration Deployment)
# Function: Migrate existing Erigon network state to OP Stack
# Corresponds to: scripts 1-8 from testnet_migration branch
# =============================================================================
set -e

source .env

echo "=========================================="
echo "OP Stack Migration from Erigon"
echo "=========================================="
echo ""

# Set deployment mode
export DEPLOYMENT_MODE="erigon"

# Step list
STEPS=(
    "01-setup-l1"              # Start L1 node (reusable)
    "00-start-erigon"          # Start Erigon services (Erigon-specific)
    "02-fund-accounts"         # Fund OP Stack role accounts (reusable)
    "10-deploy-op-contracts"   # Deploy OP contracts (complete flow)
    "14-deploy-cgt"            # Deploy Custom Gas Token (required for migration)
    "20-stop-erigon"           # Stop Erigon and extract fork info
    "21-migrate-state"         # Migrate Erigon state to merged.genesis.json
    "30-init-opgeth"           # Initialize op-geth (using merged.genesis.json)
    "40-start-op-services"     # Start op-geth/op-node/batcher
    "41-setup-p2p"             # Setup P2P network
    "31-build-prestate"        # Build op-program prestate (AFTER services start)
    "50-setup-fraud-proof"     # Complete fraud proof flow
)

echo "Migration steps:"
for i in "${!STEPS[@]}"; do
    echo "   $((i+1)). ${STEPS[$i]}"
done
echo ""
echo "IMPORTANT NOTES:"
echo "   - DEPLOYMENT_MODE=erigon: 01-setup-l1 will start L1+Erigon via 'make run_erigon'"
echo "   - Migration process will stop Erigon services after data extraction"
echo "   - Recommended to backup Erigon data before migration"
echo "   - Ensure Docker is running with sufficient resources"
echo ""

# Confirm to continue
if [ "${SKIP_CONFIRMATION:-false}" != "true" ]; then
    read -p "Continue with migration? (yes/no): " CONFIRM
    if [ "$CONFIRM" != "yes" ]; then
        echo "ERROR: User cancelled migration"
        exit 0
    fi
fi

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

    # For 30-init-opgeth, pass merged.genesis.json parameter
    if [ "$step" = "30-init-opgeth" ]; then
        if [ -f "./merged.genesis.json" ]; then
            if bash "$STEP_SCRIPT" --genesis-file ./merged.genesis.json; then
                echo "Step completed: $step (using merged.genesis.json)"
            else
                echo "ERROR: Step failed: $step"
                FAILED_STEP="$step"
                break
            fi
        else
            echo "ERROR: merged.genesis.json not found, please complete state migration first"
            echo "   Hint: Step 21 (migrate-state) should generate this file"
            FAILED_STEP="$step"
            break
        fi
    else
        if bash "$STEP_SCRIPT"; then
            echo "Step completed: $step"
        else
            echo "ERROR: Step failed: $step"
            FAILED_STEP="$step"
            break
        fi
    fi
done

echo ""
echo "=========================================="

if [ -n "$FAILED_STEP" ]; then
    echo "ERROR: Migration failed at step: $FAILED_STEP"
    echo ""
    echo "Troubleshooting suggestions:"
    echo "   1. Check $FAILED_STEP.sh log output"
    echo "   2. Verify configuration in .env file"
    echo "   3. Check if Erigon data directory exists"
    echo "   4. Check Docker service status: docker ps"
    echo "   5. View migration log: cat migrate.log"
    exit 1
else
    echo "SUCCESS: Erigon to OP Stack migration completed!"
    echo ""
    echo "Migration info:"
    echo "   L1 RPC: $L1_RPC_URL"
    echo "   L2 RPC: $L2_RPC_URL"
    echo "   Chain ID: $CHAIN_ID"
    if [ -n "$FORK_BLOCK" ]; then
        echo "   Fork Block: $FORK_BLOCK"
    fi
    echo ""
    echo "Important files:"
    echo "   - merged.genesis.json          (Migrated genesis)"
    echo "   - config-op/rollup.json        (Rollup config)"
    echo "   - config-op/state.json         (Contract deployment state)"
    echo "   - migrate.log                  (Migration log)"
    echo "   - data/cannon-data/            (Prestate files)"
    echo ""
    echo "Next steps:"
    echo "   1. Check service status: docker ps"
    echo "   2. View logs: docker logs op-geth-seq"
    echo "   3. Verify migration: cast block \$FORK_BLOCK --rpc-url \$L2_RPC_URL"
    echo "   4. Test transaction: cast send <address> --value 0.1ether --rpc-url \$L2_RPC_URL"
fi

