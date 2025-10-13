#!/bin/bash
set -e

sed_inplace() {
  if [[ "$OSTYPE" == "darwin"* ]]; then
    sed -i '' "$@"
  else
    sed -i "$@"
  fi
}

get_config_file() {
    if [ "$REALTIME_ENABLED" = "true" ]; then
        echo "config.rt.toml"
    else
        echo "config.toml"
    fi
}

# Load environment variables early
source .env

PWD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$PWD_DIR")"
SCRIPTS_DIR=$ROOT_DIR/test/scripts

if [ "$REALTIME_ENABLED" = "true" ]; then
    docker compose up -d xlayer-kafka
    sleep 20
fi
docker compose up -d op-batcher

if [ "$CONDUCTOR_ENABLED" = "true" ]; then
    CONFIG_FILE=$(get_config_file) docker compose up -d op-seq
    CONFIG_FILE=$(get_config_file) docker compose up -d op-seq2
    CONFIG_FILE=$(get_config_file) docker compose up -d op-seq3
    sleep 5
    CONFIG_FILE=$(get_config_file) docker compose up -d op-conductor
    CONFIG_FILE=$(get_config_file) docker compose up -d op-conductor2
    CONFIG_FILE=$(get_config_file) docker compose up -d op-conductor3
    sleep 5
    $SCRIPTS_DIR/active-sequencer.sh
fi

if [ "$LAUNCH_RPC_NODE" = "true" ]; then
    CONFIG_FILE=$(get_config_file) docker compose up -d op-rpc
    docker compose up -d op-rpc-2
fi

sleep 10

# Setup P2P static connections between op-geth nodes
echo "🔗 Setting up P2P static connections between op-geth nodes..."

# Function to get enode from a geth container
get_enode() {
    local container_name=$1
    local rpc_port=$2
    local enode=$(docker exec $container_name geth attach --exec "admin.nodeInfo.enode" --datadir /datadir 2>/dev/null | tr -d '"')
    echo "$enode"
}

# Function to replace 127.0.0.1 with container name in enode
replace_enode_ip() {
    local enode=$1
    local container_name=$2
    echo "$enode" | sed "s/@127.0.0.1:/@$container_name:/"
}

# Get enodes for all op-geth containers
echo "📡 Getting enode addresses..."

# Get enodes
OP_GETH_SEQ_ENODE=$(get_enode "op-geth-seq" "8545")
OP_GETH_RPC_ENODE=$(get_enode "op-geth-rpc" "8545")
OP_GETH_RPC_2_ENODE=$(get_enode "op-geth-rpc-2" "8545")

if [ "$CONDUCTOR_ENABLED" = "true" ]; then
    OP_GETH_SEQ2_ENODE=$(get_enode "op-geth-seq2" "8545")
    OP_GETH_SEQ3_ENODE=$(get_enode "op-geth-seq3" "8545")
fi

# Replace 127.0.0.1 with container names
OP_GETH_SEQ_ENODE=$(replace_enode_ip "$OP_GETH_SEQ_ENODE" "op-geth-seq")
OP_GETH_RPC_ENODE=$(replace_enode_ip "$OP_GETH_RPC_ENODE" "op-geth-rpc")
OP_GETH_RPC_2_ENODE=$(replace_enode_ip "$OP_GETH_RPC_2_ENODE" "op-geth-rpc-2")

if [ "$CONDUCTOR_ENABLED" = "true" ]; then
    OP_GETH_SEQ2_ENODE=$(replace_enode_ip "$OP_GETH_SEQ2_ENODE" "op-geth-seq2")
    OP_GETH_SEQ3_ENODE=$(replace_enode_ip "$OP_GETH_SEQ3_ENODE" "op-geth-seq3")
fi

echo "✅ Enode addresses:"
echo "  op-geth-seq: $OP_GETH_SEQ_ENODE"
echo "  op-geth-rpc: $OP_GETH_RPC_ENODE"
echo "  op-geth-rpc-2: $OP_GETH_RPC_2_ENODE"
if [ "$CONDUCTOR_ENABLED" = "true" ]; then
    echo "  op-geth-seq2: $OP_GETH_SEQ2_ENODE"
    echo "  op-geth-seq3: $OP_GETH_SEQ3_ENODE"
fi

# Function to add peer to a geth container
add_peer() {
    local container_name=$1
    local peer_enode=$2
    echo "🔗 Adding peer to $container_name: $peer_enode"
    docker exec $container_name geth attach --exec "admin.addPeer('$peer_enode')" --datadir /datadir 2>/dev/null
}

# Setup static connections between sequencer nodes
echo "🔗 Setting up static connections between sequencer nodes..."

# Add peers to op-geth-seq (connect to other sequencers)
echo "🔗 Setting up peers for op-geth-seq..."
if [ "$CONDUCTOR_ENABLED" = "true" ]; then
    add_peer "op-geth-seq" "$OP_GETH_SEQ2_ENODE"
    add_peer "op-geth-seq" "$OP_GETH_SEQ3_ENODE"
fi

if [ "$CONDUCTOR_ENABLED" = "true" ]; then
    # Add peers to op-geth-seq2 (connect to other sequencers)
    echo "🔗 Setting up peers for op-geth-seq2..."
    add_peer "op-geth-seq2" "$OP_GETH_SEQ_ENODE"
    add_peer "op-geth-seq2" "$OP_GETH_SEQ3_ENODE"

    # Add peers to op-geth-seq3 (connect to other sequencers)
    echo "🔗 Setting up peers for op-geth-seq3..."
    add_peer "op-geth-seq3" "$OP_GETH_SEQ_ENODE"
    add_peer "op-geth-seq3" "$OP_GETH_SEQ2_ENODE"
fi

# Setup RPC node to connect to all sequencer nodes
echo "🔗 Setting up RPC node to connect to all sequencer nodes..."
add_peer "op-geth-rpc" "$OP_GETH_SEQ_ENODE"
add_peer "op-geth-rpc-2" "$OP_GETH_SEQ_ENODE"
if [ "$CONDUCTOR_ENABLED" = "true" ]; then
    add_peer "op-geth-rpc" "$OP_GETH_SEQ2_ENODE"
    add_peer "op-geth-rpc" "$OP_GETH_SEQ3_ENODE"
    add_peer "op-geth-rpc-2" "$OP_GETH_SEQ2_ENODE"
    add_peer "op-geth-rpc-2" "$OP_GETH_SEQ3_ENODE"
fi

echo "✅ P2P static connections established:"
echo "  - Sequencer nodes (op-geth-seq, op-geth-seq2, op-geth-seq3) are connected to each other"
echo "  - RPC node (op-geth-rpc) is connected to all sequencer nodes"
echo "  - RPC node (op-geth-rpc-2) is connected to all sequencer nodes"



PWD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd $PWD_DIR
EXPORT_DIR="$PWD_DIR/data/cannon-data"
mkdir -p $EXPORT_DIR

echo "Adding game type to DisputeGameFactory via op-deployer..."

# Retrieve existing values from chain for reference
# Get permissioned game implementation
PERMISSIONED_GAME=$(cast call --rpc-url $L1_RPC_URL $DISPUTE_GAME_FACTORY_ADDRESS "gameImpls(uint32)(address)" 1)

# Get prestate value from prestate-proof-mt64.json
ABSOLUTE_PRESTATE=$(jq -r '.pre' "$EXPORT_DIR/prestate-proof-mt64.json")
MAX_GAME_DEPTH=$(cast call --rpc-url $L1_RPC_URL $PERMISSIONED_GAME "maxGameDepth()")
SPLIT_DEPTH=$(cast call --rpc-url $L1_RPC_URL $PERMISSIONED_GAME "splitDepth()")
VM_RAW=$(cast call --rpc-url $L1_RPC_URL $PERMISSIONED_GAME "vm()")
VM="0x${VM_RAW: -40}"
ANCHOR_STATE_REGISTRY=$(cast call --rpc-url $L1_RPC_URL $PERMISSIONED_GAME "anchorStateRegistry()")
L2_CHAIN_ID=$(cast call --rpc-url $L1_RPC_URL $PERMISSIONED_GAME "l2ChainId()")

# Call the function to add game type 1 (permissioned) via Transactor
"$SCRIPTS_DIR/add-game-type.sh" 1 true $TEMP_CLOCK_EXTENSION $TEMP_MAX_CLOCK_DURATION $ABSOLUTE_PRESTATE

export GAME_TYPE=1
CONFIG_FILE=$(get_config_file) docker compose up -d op-proposer

echo "Waiting for op-proposer to create a game..."
GAME_CREATED=false
MAX_WAIT_TIME=600  # 10 minutes timeout
WAIT_COUNT=0

while [ "$GAME_CREATED" = false ] && [ $WAIT_COUNT -lt $MAX_WAIT_TIME ]; do
    # Check if a game was created by op-proposer
    GAME_COUNT=$(cast call --rpc-url $L1_RPC_URL $DISPUTE_GAME_FACTORY_ADDRESS "gameCount()(uint256)")
    if [ "$GAME_COUNT" -gt 0 ]; then
        echo " ✅ Game created! Game count: $GAME_COUNT"
        GAME_CREATED=true
    else
        echo " ⏳ Waiting for game creation... ($WAIT_COUNT/$MAX_WAIT_TIME seconds)"
        sleep 1
        WAIT_COUNT=$((WAIT_COUNT + 1))
    fi
done

if [ "$GAME_CREATED" = false ]; then
    echo " ❌ Timeout waiting for game creation"
    exit 1
fi

echo "🛑 Stopping op-proposer..."
docker compose stop op-proposer

echo "⏰ Sleeping for ($TEMP_MAX_CLOCK_DURATION seconds)..."
sleep $TEMP_MAX_CLOCK_DURATION

echo "🔧 Executing dispute resolution sequence using op-challenger..."

# Get the latest game address
LATEST_GAME_INDEX=$((GAME_COUNT - 1))
GAME_ADDRESS=$(cast call --json --rpc-url $L1_RPC_URL $DISPUTE_GAME_FACTORY_ADDRESS "gameAtIndex(uint256)(uint256,uint256,address)" $LATEST_GAME_INDEX | jq -r '.[-1]')
echo "Latest game address: $GAME_ADDRESS"

# Execute the dispute resolution sequence using op-challenger commands
echo "1. Resolving claim (0,0) using op-challenger..."
docker run --rm \
  --network "$DOCKER_NETWORK" \
  -v "$(pwd)/data/cannon-data:/data" \
  -v "$(pwd)/config-op/rollup.json:/rollup.json" \
  -v "$(pwd)/config-op/genesis.json:/l2-genesis.json" \
  "${OP_STACK_IMAGE_TAG}" \
  /app/op-challenger/bin/op-challenger resolve-claim \
    --l1-eth-rpc=${L1_RPC_URL_IN_DOCKER} \
    --private-key=${OP_CHALLENGER_PRIVATE_KEY} \
    --game-address=$GAME_ADDRESS \
    --claim=0

echo "2. Resolving game using op-challenger..."
docker run --rm \
  --network "$DOCKER_NETWORK" \
  -v "$(pwd)/data/cannon-data:/data" \
  -v "$(pwd)/config-op/rollup.json:/rollup.json" \
  -v "$(pwd)/config-op/genesis.json:/l2-genesis.json" \
  "${OP_STACK_IMAGE_TAG}" \
  /app/op-challenger/bin/op-challenger resolve \
    --l1-eth-rpc=${L1_RPC_URL_IN_DOCKER} \
    --private-key=${OP_CHALLENGER_PRIVATE_KEY} \
    --game-address=$GAME_ADDRESS

sleep $DISPUTE_GAME_FINALITY_DELAY_SECONDS

echo "3. Claiming credit for proposer using cast command..."
TX_OUTPUT=$(cast send --json \
    --legacy \
    --rpc-url $L1_RPC_URL \
    --private-key $OP_CHALLENGER_PRIVATE_KEY \
    $GAME_ADDRESS \
    "claimCredit(address)" \
    $PROPOSER_ADDRESS)

TX_HASH=$(echo "$TX_OUTPUT" | jq -r '.transactionHash // empty')
TX_STATUS=$(echo "$TX_OUTPUT" | jq -r '.status // empty')
if [ "$TX_STATUS" = "0x1" ] || [ "$TX_STATUS" = "1" ]; then
    echo " ✅ Credit claimed successfully"
else
    echo " ❌ Transaction failed with status: $TX_STATUS"
    echo "Full output: $TX_OUTPUT"
    exit 1
fi

echo " ✅ Dispute resolution sequence completed using op-challenger commands!"

# Retrieve existing values from chain for reference
# Get permissioned game implementation
PERMISSIONED_GAME=$(cast call --rpc-url $L1_RPC_URL $DISPUTE_GAME_FACTORY_ADDRESS "gameImpls(uint32)(address)" 1)
ABSOLUTE_PRESTATE=$(cast call --rpc-url $L1_RPC_URL $PERMISSIONED_GAME "absolutePrestate()")
ANCHOR_STATE_REGISTRY=$(cast call --rpc-url $L1_RPC_URL $PERMISSIONED_GAME "anchorStateRegistry()")

# Call the function to add game type 0 (permissionless) via Transactor
"$SCRIPTS_DIR/add-game-type.sh" 0 false $CLOCK_EXTENSION $MAX_CLOCK_DURATION $ABSOLUTE_PRESTATE

export GAME_TYPE=0

sleep $TEMP_GAME_WINDOW
CONFIG_FILE=$(get_config_file) docker compose up -d --remove-orphans op-proposer op-challenger op-dispute-mon
