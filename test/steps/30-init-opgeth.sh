#!/bin/bash
# =============================================================================
# Step30: Initialize OP Geth
# Function: Initialize op-geth data directory using genesis.json
# Parameters: --genesis-file <path> (optional, defaults to config-op/genesis.json)
# =============================================================================
set -e
set -x

# Change to test directory (parent of steps/)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$(dirname "$SCRIPT_DIR")"

source .env
source tools.sh
source utils.sh

echo "=========================================="
echo "Step30: Initialize OP Geth"
echo "=========================================="

PWD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PWD_DIR=$(dirname "$PWD_DIR")
cd $PWD_DIR

# Parse arguments
GENESIS_FILE="./config-op/genesis.json"
while [[ $# -gt 0 ]]; do
    case $1 in
        --genesis-file)
            GENESIS_FILE="$2"
            shift 2
            ;;
        *)
            echo "ERROR: Unknown parameter: $1"
            echo "Usage: $0 [--genesis-file <path>]"
            exit 1
            ;;
    esac
done

# Validate genesis file exists
if [ ! -f "$GENESIS_FILE" ]; then
    echo "ERROR: Genesis file not found: $GENESIS_FILE"
    exit 1
fi

echo "Using Genesis file: $GENESIS_FILE"

# Extract contract addresses from state.json and update .env
echo "Extracting contract addresses from state.json..."
STATE_JSON="$PWD_DIR/config-op/state.json"

if [ -f "$STATE_JSON" ]; then
    # Determine structure type (object vs array)
    OPCD_TYPE=$(jq -r '.opChainDeployments | type' "$STATE_JSON" 2>/dev/null)

    if [ "$OPCD_TYPE" = "object" ]; then
        DISPUTE_GAME_FACTORY_ADDRESS=$(jq -r '.opChainDeployments.DisputeGameFactoryProxy // empty' "$STATE_JSON")
        L2OO_ADDRESS=$(jq -r '.opChainDeployments.L2OutputOracleProxy // empty' "$STATE_JSON")
        OPCM_IMPL_ADDRESS=$(jq -r '.appliedIntent.opcmAddress // empty' "$STATE_JSON")
        SYSTEM_CONFIG_PROXY_ADDRESS=$(jq -r '.opChainDeployments.SystemConfigProxy // empty' "$STATE_JSON")
        OPTIMISM_PORTAL_PROXY_ADDRESS=$(jq -r '.opChainDeployments.OptimismPortalProxy // empty' "$STATE_JSON")
        PROXY_ADMIN=$(jq -r '.superchainContracts.SuperchainProxyAdminImpl // empty' "$STATE_JSON")
    elif [ "$OPCD_TYPE" = "array" ]; then
        DISPUTE_GAME_FACTORY_ADDRESS=$(jq -r '.opChainDeployments[0].DisputeGameFactoryProxy // empty' "$STATE_JSON")
        L2OO_ADDRESS=$(jq -r '.opChainDeployments[0].L2OutputOracleProxy // empty' "$STATE_JSON")
        OPCM_IMPL_ADDRESS=$(jq -r '.appliedIntent.opcmAddress // empty' "$STATE_JSON")
        SYSTEM_CONFIG_PROXY_ADDRESS=$(jq -r '.opChainDeployments[0].SystemConfigProxy // empty' "$STATE_JSON")
        OPTIMISM_PORTAL_PROXY_ADDRESS=$(jq -r '.opChainDeployments[0].OptimismPortalProxy // empty' "$STATE_JSON")
        PROXY_ADMIN=$(jq -r '.superchainContracts.SuperchainProxyAdminImpl // empty' "$STATE_JSON")
    else
        DISPUTE_GAME_FACTORY_ADDRESS=""
        L2OO_ADDRESS=""
        OPCM_IMPL_ADDRESS=""
        SYSTEM_CONFIG_PROXY_ADDRESS=""
        OPTIMISM_PORTAL_PROXY_ADDRESS=""
        PROXY_ADMIN=""
    fi

    # Update .env with extracted addresses
    if [ -n "$DISPUTE_GAME_FACTORY_ADDRESS" ]; then
        echo "Found DisputeGameFactoryProxy: $DISPUTE_GAME_FACTORY_ADDRESS"
        sed_inplace "s/DISPUTE_GAME_FACTORY_ADDRESS=.*/DISPUTE_GAME_FACTORY_ADDRESS=$DISPUTE_GAME_FACTORY_ADDRESS/" .env
    fi

    if [ -n "$L2OO_ADDRESS" ]; then
        echo "Found L2OutputOracleProxy: $L2OO_ADDRESS"
        sed_inplace "s/L2OO_ADDRESS=.*/L2OO_ADDRESS=$L2OO_ADDRESS/" .env
    fi

    if [ -n "$OPCM_IMPL_ADDRESS" ]; then
        echo "Found opcmAddress: $OPCM_IMPL_ADDRESS"
        sed_inplace "s/OPCM_IMPL_ADDRESS=.*/OPCM_IMPL_ADDRESS=$OPCM_IMPL_ADDRESS/" .env
    fi

    if [ -n "$SYSTEM_CONFIG_PROXY_ADDRESS" ]; then
        echo "Found SystemConfigProxy: $SYSTEM_CONFIG_PROXY_ADDRESS"
        sed_inplace "s/SYSTEM_CONFIG_PROXY_ADDRESS=.*/SYSTEM_CONFIG_PROXY_ADDRESS=$SYSTEM_CONFIG_PROXY_ADDRESS/" .env
    fi

    if [ -n "$OPTIMISM_PORTAL_PROXY_ADDRESS" ]; then
        echo "Found OptimismPortalProxy: $OPTIMISM_PORTAL_PROXY_ADDRESS"
        sed_inplace "s/OPTIMISM_PORTAL_PROXY_ADDRESS=.*/OPTIMISM_PORTAL_PROXY_ADDRESS=$OPTIMISM_PORTAL_PROXY_ADDRESS/" .env
    fi

    if [ -n "$PROXY_ADMIN" ]; then
        echo "Found ProxyAdmin: $PROXY_ADMIN"
        sed_inplace "s/PROXY_ADMIN=.*/PROXY_ADMIN=$PROXY_ADMIN/" .env
    fi

    echo "Contract addresses updated in .env"
else
    echo "WARNING: state.json not found at $STATE_JSON"
fi

# Initialize op-geth-seq
OP_GETH_DATADIR="$(pwd)/data/op-geth-seq"
rm -rf "$OP_GETH_DATADIR"
mkdir -p "$OP_GETH_DATADIR"

echo "Initializing op-geth-seq..."
docker compose run --no-deps --rm \
  -v "$(pwd)/$GENESIS_FILE:/genesis.json" \
  op-geth-seq \
  --datadir "/datadir" \
  --gcmode=archive \
  --db.engine=${DB_ENGINE:-pebble} \
  --log.format json \
  init \
  --state.scheme=hash \
  /genesis.json 2>&1 | tee init.log

echo "SUCCESS: op-geth-seq initialization completed"

# Copy data to RPC node
OP_GETH_RPC_DATADIR="$(pwd)/data/op-geth-rpc"
echo "Copying data to op-geth-rpc..."
rm -rf "$OP_GETH_RPC_DATADIR"
cp -r "$OP_GETH_DATADIR" "$OP_GETH_RPC_DATADIR"
rm -f "$OP_GETH_RPC_DATADIR/geth/nodekey"  # Generate unique node ID

echo "SUCCESS: op-geth-rpc data prepared"

# If conductor is enabled, initialize other sequencer nodes
if [ "$CONDUCTOR_ENABLED" = "true" ]; then
    echo "Initializing additional sequencer nodes (conductor mode)..."

    OP_GETH_DATADIR2="$(pwd)/data/op-geth-seq2"
    rm -rf "$OP_GETH_DATADIR2"
    cp -r "$OP_GETH_DATADIR" "$OP_GETH_DATADIR2"

    OP_GETH_DATADIR3="$(pwd)/data/op-geth-seq3"
    rm -rf "$OP_GETH_DATADIR3"
    cp -r "$OP_GETH_DATADIR" "$OP_GETH_DATADIR3"

    echo "SUCCESS: All nodes initialized in conductor mode"
fi

echo ""
echo "SUCCESS: Step 30 completed: OP Geth initialized"
echo "   Sequencer Data: $OP_GETH_DATADIR"
echo "   RPC Data: $OP_GETH_RPC_DATADIR"

