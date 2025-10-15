#!/bin/bash
# =============================================================================
# Step01: Start L1 node
# Function: StartingL1ValidatingnodeErigonnetworkL1
# =============================================================================
set -e
set -x

# Change to test directory (parent of steps/)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$(dirname "$SCRIPT_DIR")"

source .env
source tools.sh

echo "=========================================="
echo "Step01: Start L1 node"
echo "=========================================="

if [ "$ENV" = "local" ]; then
    echo "Starting local L1 node..."
    echo "   Starting L1 validator via docker compose..."
    ${DOCKER_COMPOSE_CMD} up -d l1-validator

    sleep 30

    # Wait for L1 node to finish syncing
    echo "Waiting for L1 node to sync..."
    while [[ "$(cast rpc eth_syncing --rpc-url $L1_RPC_URL)" != "false" ]]; do
        echo "   L1 node still syncing..."
        sleep 5
    done

    echo "SUCCESS: L1 node started and synced"

    # Validate L1 connection
    L1_BLOCK=$(cast block-number --rpc-url $L1_RPC_URL)
    L1_CHAIN_ID=$(cast chain-id --rpc-url $L1_RPC_URL)
    echo "   L1 Chain ID: $L1_CHAIN_ID"
    echo "   L1 Latest Block: $L1_BLOCK"

elif [ "$ENV" = "testnet" ]; then
    echo "  TestenvironmentL1Running"
    echo "   L1 RPC URL: $L1_RPC_URL"

    # ValidatingL1
    if ! cast block-number --rpc-url $L1_RPC_URL >/dev/null 2>&1; then
        echo " ERROR: ConnectingL1 RPC: $L1_RPC_URL"
        exit 1
    fi

    echo "SUCCESS: L1node"
else
    echo " ERROR: unknownenvironment ENV=$ENV"
    exit 1
fi

echo ""
echo "SUCCESS: Step01completed: L1node"

