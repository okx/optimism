#!/bin/bash
# =============================================================================
# Step40: Start OP services
# Function: Start op-geth (sequencer + rpc), op-node, op-batcher, op-conductor
# =============================================================================
set -e
set -x

# Change to test directory (parent of steps/)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$(dirname "$SCRIPT_DIR")"

source .env
source utils.sh
source tools.sh

echo "=========================================="
echo "Step40: Start OP services"
echo "=========================================="

PWD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PWD_DIR=$(dirname "$PWD_DIR")
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STEPS_DIR="$SCRIPT_DIR"

cd $PWD_DIR

# Preparingdatadirectory
echo "Preparing data directories..."

OP_GETH_DATADIR="$(pwd)/data/op-geth-seq"
OP_GETH_DATADIR2="$(pwd)/data/op-geth-seq2"
OP_GETH_DATADIR3="$(pwd)/data/op-geth-seq3"
OP_GETH_RPC_DATADIR="$(pwd)/data/op-geth-rpc"

# Copyingsequencerdatanode
if [ "$CONDUCTOR_ENABLED" = "true" ]; then
    echo " Copyingdataconductornode..."
    rm -rf "$OP_GETH_DATADIR2"
    cp -r "$OP_GETH_DATADIR" "$OP_GETH_DATADIR2"

    rm -rf "$OP_GETH_DATADIR3"
    cp -r "$OP_GETH_DATADIR" "$OP_GETH_DATADIR3"
fi

# CopyingdataRPCnodeCopying
if [ ! -d "$OP_GETH_RPC_DATADIR" ]; then
    echo " CopyingdataRPCnode..."
    rm -rf "$OP_GETH_RPC_DATADIR"
    cp -r "$OP_GETH_DATADIR" "$OP_GETH_RPC_DATADIR"
    rm -f "$OP_GETH_RPC_DATADIR/geth/nodekey"
fi

# testnetenvironmentUpdatingL1 RPC URLs
if [ "$ENV" = "testnet" ]; then
    echo " Configtestnetenvironment..."
    L1_RPC_URL="https://fullnode-inner.okg.com/sepolia/fork/okbc/rpc"
    L1_BEACON_URL_IN_DOCKER="https://fullnode-inner.okg.com/ethsepoliabeacon/native/layer1/rpc"
    sed_inplace "s|L1_RPC_URL=.*|L1_RPC_URL=$L1_RPC_URL|" .env
    sed_inplace "s|L1_RPC_URL_IN_DOCKER=.*|L1_RPC_URL_IN_DOCKER=$L1_RPC_URL|" .env
    sed_inplace "s|L1_BEACON_URL_IN_DOCKER=.*|L1_BEACON_URL_IN_DOCKER=$L1_BEACON_URL_IN_DOCKER|" .env
fi

echo ""
echo " Start OP services..."
echo ""

# Startingbatcher
echo "  Startingop-batcher..."
${DOCKER_COMPOSE_CMD} up -d op-batcher
sleep 3

# conductorStartingsequencer
if [ "$CONDUCTOR_ENABLED" = "true" ]; then
    echo "  Startingconductormodesequencer..."
    ${DOCKER_COMPOSE_CMD} up -d op-conductor
    ${DOCKER_COMPOSE_CMD} up -d op-conductor2
    ${DOCKER_COMPOSE_CMD} up -d op-conductor3
    sleep 3

    # sequencer
    if [ -f "$STEPS_DIR/active-sequencer.sh" ]; then
        echo " sequencer..."
        $STEPS_DIR/active-sequencer.sh
    fi
fi

# Waiting forserviceStarting
sleep 10

# CheckingL2 genesis hash
echo " Checkinggenesis hash..."
LOG_OUTPUT=$(${DOCKER_COMPOSE_CMD} logs op-seq 2>&1 | tail -20)

if echo "$LOG_OUTPUT" | grep -q "expected L2 genesis hash to match L2 block at genesis block number"; then
    echo " L2 genesis hash!"
    echo "ERROR:"
    echo "$LOG_OUTPUT" | grep "expected L2 genesis hash to match L2 block at genesis block number"
    exit 1
fi

echo "SUCCESS: Genesis hashValidating"
echo ""

# StartingRPCnode
echo "  Startingop-rpcnode..."
${DOCKER_COMPOSE_CMD} up -d op-rpc
sleep 3

echo ""
echo "SUCCESS: Step40completed: OPserviceStarting"
echo ""
echo " Runningservice:"
docker ps --filter "name=op-" --format "table {{.Names}}\t{{.Status}}" | head -10

