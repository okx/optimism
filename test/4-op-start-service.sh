#!/bin/bash
set -e

sed_inplace() {
  if [[ "$OSTYPE" == "darwin"* ]]; then
    sed -i '' "$@"
  else
    sed -i "$@"
  fi
}

# Load environment variables early
source .env

PWD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$PWD_DIR")"
SCRIPTS_DIR=$ROOT_DIR/test/scripts

if [ "$REALTIME_ENABLED" = "true" ]; then
    docker compose up -d xlayer-kafka
    sleep 10
fi
docker compose up -d op-batcher

if [ "$CONDUCTOR_ENABLED" = "true" ]; then
    docker compose up -d op-conductor
    docker compose up -d op-conductor2
    docker compose up -d op-conductor3
    sleep 3
    $SCRIPTS_DIR/active-sequencer.sh
fi

if [ "$LAUNCH_RPC_NODE" = "true" ]; then
    docker compose up -d op-rpc
    if [ "$REALTIME_ENABLED" = "true" ]; then
        docker compose up -d op-rpc-rt
    fi
fi

sleep 10

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
docker compose up -d op-proposer

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
docker compose up -d --remove-orphans op-proposer op-challenger op-dispute-mon
