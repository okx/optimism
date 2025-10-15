#!/bin/bash
# =============================================================================
# Step 00: Start Erigon services (Erigon-specific)
# Function: Start XLayer Erigon sequencer, RPC, bridge, etc.
# Note: Assumes L1 is already running (via 01-setup-l1.sh)
# =============================================================================
set -e
set -x

# Change to test directory (parent of steps/)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$(dirname "$SCRIPT_DIR")"

source .env
source tools.sh

echo "=========================================="
echo "Step 00: Start Erigon services"
echo "=========================================="

if [ "$ENV" = "local" ]; then
    echo "Starting Erigon services..."

    # Deploy ERC20 on L1 (required by Erigon)
    echo "Deploying ERC20 token on L1..."
    cd contracts
    forge create OKBToken.sol:StandardERC20 \
        --private-key $SEQUENCER_PRIVATE_KEY \
        --rpc-url $L1_RPC_URL \
        --legacy \
        --broadcast \
        --constructor-args "OKBToken" "OKB" 1000000000
    cd ..

    # Fund accounts required by Erigon (from SEQUENCER)
    echo "Funding Erigon accounts..."
    for addr in $DEPLOYER_ADDRESS $AGGREGATOR_ADDRESS $RICH_ADDRESS $CHALLENGER_ADDRESS; do
        echo "   Funding 1000 ETH to $addr..."
        cast send -f $SEQUENCER_ADDRESS \
            --private-key $SEQUENCER_PRIVATE_KEY \
            --value 1000ether \
            $addr \
            --legacy \
            --rpc-url $L1_RPC_URL
    done

    sleep 5

    # Initialize Erigon
    echo "Initializing Erigon..."
    ./steps/erigon-init-helper.sh

    sleep 5

    # Start Erigon services
    echo "Starting Erigon database services..."
    ${DOCKER_COMPOSE_CMD} up -d xlayer-bridge-db
    ${DOCKER_COMPOSE_CMD} up -d xlayer-pool-db

    sleep 5

    echo "Starting Erigon core services..."
    ${DOCKER_COMPOSE_CMD} up -d xlayer-agglayer-prover
    ${DOCKER_COMPOSE_CMD} up -d xlayer-agglayer

    sleep 5

    echo "Starting Erigon sequencer and pool manager..."
    ${DOCKER_COMPOSE_CMD} up -d xlayer-approve
    ${DOCKER_COMPOSE_CMD} up -d xlayer-seq
    ${DOCKER_COMPOSE_CMD} up -d xlayer-pool-manager

    sleep 5

    echo "Starting Erigon RPC..."
    ${DOCKER_COMPOSE_CMD} up -d xlayer-rpc

    sleep 10

    echo "Starting Erigon bridge services..."
    ${DOCKER_COMPOSE_CMD} up -d xlayer-bridge-service
    ${DOCKER_COMPOSE_CMD} up -d xlayer-bridge-ui

    # Fund bridge account
    cast send -f $DEPLOYER_ADDRESS \
        --private-key $DEPLOYER_PRIVATE_KEY \
        --value 0.01ether \
        $SEQUENCER_ADDRESS \
        --legacy \
        --rpc-url $L2_RPC_URL || true

    ${DOCKER_COMPOSE_CMD} up -d xlayer-agg-sender

    sleep 3

    echo ""
    echo "SUCCESS: Step 00 completed: Erigon services started"
    echo "   Erigon L2 RPC: $L2_RPC_URL (check docker ps for xlayer-rpc)"

elif [ "$ENV" = "testnet" ]; then
    echo "ERROR: Testnet environment not implemented for Erigon"
    exit 1
else
    echo "ERROR: Unknown environment: $ENV"
    exit 1
fi

