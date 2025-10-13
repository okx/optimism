#!/bin/bash

set -e

source .env

sed_inplace() {
  if [[ "$OSTYPE" == "darwin"* ]]; then
    sed -i '' "$@"
  else
    sed -i "$@"
  fi
}

# Check if FORK_BLOCK is set
if [ -z "$FORK_BLOCK" ]; then
    echo " ❌ FORK_BLOCK environment variable is not set"
    echo "Please set FORK_BLOCK in your .env file"
    exit 1
fi

FORK_BLOCK_HEX=$(printf "0x%x" "$FORK_BLOCK")
sed_inplace '/"config": {/,/}/ s/"optimism": {/"legacyXLayerBlock": '"$((FORK_BLOCK + 1))"',\n    "optimism": {/' ./config-op/genesis.json
sed_inplace 's/"parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000"/"parentHash": "'"$PARENT_HASH"'"/' ./config-op/genesis.json
sed_inplace '/"70997970c51812dc3a010c7d01b50e0d17dc79c8": {/,/}/ s/"balance": "[^"]*"/"balance": "0x446c3b15f9926687d2c40534fdb564000000000000"/' config-op/genesis.json
sed_inplace 's/"number": 0/"number": '"$((FORK_BLOCK + 1))"'/' ./config-op/rollup.json

# Extract contract addresses from state.json and update .env file
echo "🔧 Extracting contract addresses from state.json..."
PWD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STATE_JSON="$PWD_DIR/config-op/state.json"

if [ -f "$STATE_JSON" ]; then
    # Extract contract addresses from state.json
    DEPLOYMENTS_TYPE=$(jq -r 'type' "$STATE_JSON")
    if [ "$DEPLOYMENTS_TYPE" = "object" ]; then
        OPCD_TYPE=$(jq -r '.opChainDeployments | type' "$STATE_JSON" 2>/dev/null)
        if [ "$OPCD_TYPE" = "object" ]; then
            DISPUTE_GAME_FACTORY_ADDRESS=$(jq -r '.opChainDeployments.DisputeGameFactoryProxy // empty' "$STATE_JSON")
            L2OO_ADDRESS=$(jq -r '.opChainDeployments.L2OutputOracleProxy // empty' "$STATE_JSON")
            OPCM_IMPL_ADDRESS=$(jq -r '.appliedIntent.opcmAddress // empty' "$STATE_JSON")
            SYSTEM_CONFIG_PROXY_ADDRESS=$(jq -r '.opChainDeployments.SystemConfigProxy // empty' "$STATE_JSON")
            PROXY_ADMIN=$(jq -r '.superchainContracts.SuperchainProxyAdminImpl // empty' "$STATE_JSON")
        elif [ "$OPCD_TYPE" = "array" ]; then
            DISPUTE_GAME_FACTORY_ADDRESS=$(jq -r '.opChainDeployments[0].DisputeGameFactoryProxy // empty' "$STATE_JSON")
            L2OO_ADDRESS=$(jq -r '.opChainDeployments[0].L2OutputOracleProxy // empty' "$STATE_JSON")
            OPCM_IMPL_ADDRESS=$(jq -r '.appliedIntent.opcmAddress // empty' "$STATE_JSON")
            SYSTEM_CONFIG_PROXY_ADDRESS=$(jq -r '.opChainDeployments[0].SystemConfigProxy // empty' "$STATE_JSON")
            PROXY_ADMIN=$(jq -r '.superchainContracts.SuperchainProxyAdminImpl // empty' "$STATE_JSON")
        else
            DISPUTE_GAME_FACTORY_ADDRESS=""
            L2OO_ADDRESS=""
            OPCM_IMPL_ADDRESS=""
            SYSTEM_CONFIG_PROXY_ADDRESS=""
            PROXY_ADMIN=""
        fi

        # Update .env if found
        if [ -n "$DISPUTE_GAME_FACTORY_ADDRESS" ]; then
            echo " ✅ Found DisputeGameFactoryProxy address: $DISPUTE_GAME_FACTORY_ADDRESS"
            sed_inplace "s/DISPUTE_GAME_FACTORY_ADDRESS=.*/DISPUTE_GAME_FACTORY_ADDRESS=$DISPUTE_GAME_FACTORY_ADDRESS/" .env
        else
            echo " ⚠️ DisputeGameFactoryProxy address not found in opChainDeployments"
        fi

        if [ -n "$L2OO_ADDRESS" ]; then
            echo " ✅ Found L2OutputOracleProxy address: $L2OO_ADDRESS"
            sed_inplace "s/L2OO_ADDRESS=.*/L2OO_ADDRESS=$L2OO_ADDRESS/" .env
        else
            echo " ⚠️ L2OutputOracleProxy address not found in opChainDeployments"
        fi

        if [ -n "$OPCM_IMPL_ADDRESS" ]; then
            echo " ✅ Found opcmAddress address: $OPCM_IMPL_ADDRESS"
            sed_inplace "s/OPCM_IMPL_ADDRESS=.*/OPCM_IMPL_ADDRESS=$OPCM_IMPL_ADDRESS/" .env
        else
            echo " ⚠️ opcmAddress address not found in opChainDeployments"
        fi

        if [ -n "$SYSTEM_CONFIG_PROXY_ADDRESS" ]; then
            echo " ✅ Found SystemConfigProxy address: $SYSTEM_CONFIG_PROXY_ADDRESS"
            sed_inplace "s/SYSTEM_CONFIG_PROXY_ADDRESS=.*/SYSTEM_CONFIG_PROXY_ADDRESS=$SYSTEM_CONFIG_PROXY_ADDRESS/" .env
        else
            echo " ⚠️ SystemConfigProxy address not found in opChainDeployments"
        fi

        if [ -n "$PROXY_ADMIN" ]; then
            echo " ✅ Found ProxyAdmin address: $PROXY_ADMIN"
            sed_inplace "s/PROXY_ADMIN=.*/PROXY_ADMIN=$PROXY_ADMIN/" .env
        else
            echo " ⚠️ ProxyAdmin address not found in opChainDeployments"
        fi

        # Show summary
        echo " 📄 Contract addresses updated in .env:"
        echo "   DISPUTE_GAME_FACTORY_ADDRESS=$DISPUTE_GAME_FACTORY_ADDRESS"
        echo "   L2OO_ADDRESS=$L2OO_ADDRESS"
        echo "   OPCM_IMPL_ADDRESS=$OPCM_IMPL_ADDRESS"
        echo "   SYSTEM_CONFIG_PROXY_ADDRESS=$SYSTEM_CONFIG_PROXY_ADDRESS"
        echo "   PROXY_ADMIN=$PROXY_ADMIN"
    else
        echo " ❌ $STATE_JSON is not a valid JSON object"
    fi
else
    echo " ❌ state.json not found at $STATE_JSON"
fi

# init op-geth-seq and op-geth-rpc
OP_GETH_DATADIR="$(pwd)/data/op-geth-seq"
rm -rf "$OP_GETH_DATADIR"
mkdir -p "$OP_GETH_DATADIR"
docker compose run --no-deps --rm \
  -v "$(pwd)/$CONFIG_DIR/genesis.json:/genesis.json" \
  op-geth-seq \
  --datadir "/datadir" \
  --gcmode=archive \
  --db.engine=$DB_ENGINE \
  --log.format json \
  init \
  --state.scheme=hash \
  /genesis.json 2>&1 | tee init.log

# Start op-geth-seq to get the block hash at FORK_BLOCK+1
echo "🚀 Starting op-geth-seq to get block hash at FORK_BLOCK+1..."
docker compose up -d op-geth-seq

# Wait for op-geth-seq to be ready
echo "⏳ Waiting for op-geth-seq to be ready..."
sleep 20
set +x
# Get the block hash at FORK_BLOCK+1
TARGET_BLOCK=$((FORK_BLOCK + 1))
echo "🔍 Getting block hash at block number: $TARGET_BLOCK"
NEW_BLOCK_HASH=$(curl -s -X POST -H "Content-Type: application/json" --data "{\"jsonrpc\":\"2.0\",\"method\":\"eth_getBlockByNumber\",\"params\":[\"0x$(printf "%x" $TARGET_BLOCK)\",false],\"id\":1}" http://localhost:8123 | jq -r '.result.hash' 2>/dev/null)
echo "New block hash: $NEW_BLOCK_HASH"
if [ -z "$NEW_BLOCK_HASH" ] || [ "$NEW_BLOCK_HASH" = "null" ] || [ "$NEW_BLOCK_HASH" = "undefined" ]; then
    echo " ❌ Failed to get block hash at block $TARGET_BLOCK"
    echo "Please check if op-geth-seq is running and has produced enough blocks"
    docker compose logs op-geth-seq --tail=20
    exit 1
fi
set -x

echo " ✅ Got block hash at block $TARGET_BLOCK: $NEW_BLOCK_HASH"

# Stop op-geth-seq after getting the hash
docker compose stop op-geth-seq

# update genesis block hash in rollup.json
jq ".genesis.l2.hash = \"$NEW_BLOCK_HASH\"" config-op/rollup.json > config-op/rollup.json.tmp
mv config-op/rollup.json.tmp config-op/rollup.json

# Copy initialized database from op-geth-seq to other nodes
OP_GETH_RPC_DATADIR="$(pwd)/data/op-geth-rpc"

echo " 🔄 Copying database from op-geth-seq to op-geth-rpc..."
rm -rf "$OP_GETH_RPC_DATADIR"
cp -r "$OP_GETH_DATADIR" "$OP_GETH_RPC_DATADIR"

OP_GETH_RPC_2_DATADIR="$(pwd)/data/op-geth-rpc-2"

echo " 🔄 Copying database from op-geth-seq to op-geth-rpc-2..."
rm -rf "$OP_GETH_RPC_2_DATADIR"
cp -r "$OP_GETH_DATADIR" "$OP_GETH_RPC_2_DATADIR"

# Remove nodekey to ensure op-geth-rpc and op-geth-rpc-2 generates a unique node ID
echo " 🔑 Removing nodekey to generate unique node ID for op-geth-rpc..."
rm -f "$OP_GETH_RPC_DATADIR/geth/nodekey"


if [ "$CONDUCTOR_ENABLED" = "true" ]; then
    OP_GETH_DATADIR="$(pwd)/data/op-geth-seq2"
    rm -rf "$OP_GETH_DATADIR"
    mkdir -p "$OP_GETH_DATADIR"
    docker compose run --no-deps \
      -v "$(pwd)/$CONFIG_DIR/genesis.json:/genesis.json" \
      op-geth-seq2 \
      --datadir "/datadir" \
      --gcmode=archive \
      --db.engine=$DB_ENGINE \
      init \
      --state.scheme=hash \
      /genesis.json


    OP_GETH_DATADIR="$(pwd)/data/op-geth-seq3"
    rm -rf "$OP_GETH_DATADIR"
    mkdir -p "$OP_GETH_DATADIR"
    docker compose run --no-deps \
      -v "$(pwd)/$CONFIG_DIR/genesis.json:/genesis.json" \
      op-geth-seq3 \
      --datadir "/datadir" \
      --gcmode=archive \
      --db.engine=$DB_ENGINE \
      init \
      --state.scheme=hash \
      /genesis.json
fi

echo "finished init op-geth-seq and op-geth-rpc"

# genesis.json is too large to embed in go, so we compress it now and decompress it in go code
gzip -c config-op/genesis.json > config-op/genesis.json.gz

# Ensure prestate files exist and devnetL1.json is consistent before deploying contracts
EXPORT_DIR="$PWD_DIR/data/cannon-data"
rm -rf $EXPORT_DIR
mkdir -p $EXPORT_DIR

echo "🔨 Building op-program prestate files..."

# Determine if we are using rootless Docker and set the appropriate Docker command
ROOTLESS_DOCKER=$(docker info -f "{{println .SecurityOptions}}" | grep rootless || true)
if ! [ -z "$ROOTLESS_DOCKER" ]; then
echo "Using rootless Docker!"
DOCKER_CMD="docker run --rm --privileged "
DOCKER_TYPE="rootless"
else
DOCKER_CMD="docker run --rm -v /var/run/docker.sock:/var/run/docker.sock "
DOCKER_TYPE="default"
fi

# Run the reproducible-prestate command
$DOCKER_CMD \
    -v "$(pwd)/scripts:/scripts" \
    -v "$(pwd)/config-op/rollup.json:/app/op-program/chainconfig/configs/${CHAIN_ID}-rollup.json" \
    -v "$(pwd)/config-op/genesis.json.gz:/app/op-program/chainconfig/configs/${CHAIN_ID}-genesis-l2.json" \
    -v "$(pwd)/l1-geth/execution/genesis.json:/app/op-program/chainconfig/configs/1337-genesis-l1.json" \
    -v "$EXPORT_DIR:/app/op-program/bin" \
    "${OP_STACK_IMAGE_TAG}" \
    bash -c " \
      /scripts/docker-install-start.sh $DOCKER_TYPE
      make -C op-program reproducible-prestate
    "
