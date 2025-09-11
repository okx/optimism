#!/bin/bash

set -x
set -e

source .env

sed_inplace() {
  if [[ "$OSTYPE" == "darwin"* ]]; then
    sed -i '' "$@"
  else
    sed -i "$@"
  fi
}
PWD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

RAMFS_DIR=$PWD_DIR/ramfs
echo "RAMFS_DIR is $RAMFS_DIR"

mkdir -p $RAMFS_DIR || echo "$RAMFS_DIR already exists"
#if not macos then
if [[ "$OSTYPE" != "darwin"* ]]; then
  mount -t ramfs ramfs $RAMFS_DIR
  echo "Mounted ramfs at $RAMFS_DIR"
fi
cp "$MDBX_FILE" $RAMFS_DIR/mdbx.dat

FORK_BLOCK_HEX=$(printf "0x%x" "$FORK_BLOCK")
sed_inplace 's/"number": "0x0"/"number": "'"$FORK_BLOCK_HEX"'"/' ./config-op/genesis.json
sed_inplace 's/"number": 0/"number": '"$FORK_BLOCK"'/' ./config-op/rollup.json

# Extract contract addresses from state.json and update .env file
echo "🔧 Extracting contract addresses from state.json..."

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
            echo "✅ Found DisputeGameFactoryProxy address: $DISPUTE_GAME_FACTORY_ADDRESS"
            sed_inplace "s/DISPUTE_GAME_FACTORY_ADDRESS=.*/DISPUTE_GAME_FACTORY_ADDRESS=$DISPUTE_GAME_FACTORY_ADDRESS/" .env
        else
            echo "⚠️  DisputeGameFactoryProxy address not found in opChainDeployments"
        fi

        if [ -n "$L2OO_ADDRESS" ]; then
            echo "✅ Found L2OutputOracleProxy address: $L2OO_ADDRESS"
            sed_inplace "s/L2OO_ADDRESS=.*/L2OO_ADDRESS=$L2OO_ADDRESS/" .env
        else
            echo "⚠️  L2OutputOracleProxy address not found in opChainDeployments"
        fi

        if [ -n "$OPCM_IMPL_ADDRESS" ]; then
            echo "✅ Found opcmAddress address: $OPCM_IMPL_ADDRESS"
            sed_inplace "s/OPCM_IMPL_ADDRESS=.*/OPCM_IMPL_ADDRESS=$OPCM_IMPL_ADDRESS/" .env
        else
            echo "⚠️  opcmAddress address not found in opChainDeployments"
        fi

        if [ -n "$SYSTEM_CONFIG_PROXY_ADDRESS" ]; then
            echo "✅ Found SystemConfigProxy address: $SYSTEM_CONFIG_PROXY_ADDRESS"
            sed_inplace "s/SYSTEM_CONFIG_PROXY_ADDRESS=.*/SYSTEM_CONFIG_PROXY_ADDRESS=$SYSTEM_CONFIG_PROXY_ADDRESS/" .env
        else
            echo "⚠️  SystemConfigProxy address not found in opChainDeployments"
        fi

        if [ -n "$PROXY_ADMIN" ]; then
            echo "✅ Found ProxyAdmin address: $PROXY_ADMIN"
            sed_inplace "s/PROXY_ADMIN=.*/PROXY_ADMIN=$PROXY_ADMIN/" .env
        else
            echo "⚠️  ProxyAdmin address not found in opChainDeployments"
        fi

        # Show summary
        echo "📄 Contract addresses updated in .env:"
        echo "   DISPUTE_GAME_FACTORY_ADDRESS=$DISPUTE_GAME_FACTORY_ADDRESS"
        echo "   L2OO_ADDRESS=$L2OO_ADDRESS"
        echo "   OPCM_IMPL_ADDRESS=$OPCM_IMPL_ADDRESS"
        echo "   SYSTEM_CONFIG_PROXY_ADDRESS=$SYSTEM_CONFIG_PROXY_ADDRESS"
        echo "   PROXY_ADMIN=$PROXY_ADMIN"
    else
        echo "❌ $STATE_JSON is not a valid JSON object"
    fi
else
    echo "❌ state.json not found at $STATE_JSON"
fi

# init op-geth-seq and op-geth-rpc
OP_GETH_DATADIR="$(pwd)/data/op-geth-seq"
rm -rf "$OP_GETH_DATADIR"
mkdir -p "$OP_GETH_DATADIR"
docker compose run --no-deps \
  -v "$(pwd)/$CONFIG_DIR/genesis.json:/genesis.json" \
  op-geth-seq \
  --datadir "/datadir" \
  --gcmode=archive \
  --db.engine=$DB_ENGINE \
  --log.format json \
  migrate \
  --state.scheme=hash \
  --smt-db-path=$RAMFS_DIR/mdbx.dat \
  --ignore-addresses=0x000000000000000000000000000000005ca1ab1e \
  /genesis.json 2>&1 | tee init.log

# update genesis block hash in rollup.json
NEW_BLOCK_HASH=$(grep "Successfully wrote genesis state" init.log | jq -r .hash)
ROLLUP_CONTENT=$(jq ".genesis.l2.hash = \"$NEW_BLOCK_HASH\"" config-op/rollup.json)
echo $ROLLUP_CONTENT | jq > config-op/rollup.json
rm -f init.log

OP_GETH_DATADIR="$(pwd)/data/op-geth-rpc"
rm -rf "$OP_GETH_DATADIR"
mkdir -p "$OP_GETH_DATADIR"
docker compose run --no-deps \
  -v "$(pwd)/$CONFIG_DIR/genesis.json:/genesis.json" \
  op-geth-rpc \
  --datadir "/datadir" \
  --gcmode=archive \
  --db.engine=$DB_ENGINE \
  init \
  --state.scheme=hash \
  --smt-db-path=$RAMFS_DIR/mdbx.dat \
  --ignore-addresses=0x000000000000000000000000000000005ca1ab1e \
  /genesis.json

echo "finished init op-geth-seq and op-geth-rpc"

# genesis.json is too large to embed in go, so we compress it now and decompress it in go code
gzip -c config-op/genesis.json > config-op/genesis.json.gz

# Ensure prestate files exist and devnetL1.json is consistent before deploying contracts
EXPORT_DIR="$PWD_DIR/data/cannon-data"
rm -rf $EXPORT_DIR
mkdir -p $EXPORT_DIR
docker run --rm \
    -v /var/run/docker.sock:/var/run/docker.sock \
    -v "$(pwd)/config-op/rollup.json:/app/op-program/chainconfig/configs/195-rollup.json" \
    -v "$(pwd)/config-op/genesis.json.gz:/app/op-program/chainconfig/configs/195-genesis-l2.json" \
    -v "$EXPORT_DIR:/app/op-program/bin" \
    -w /app \
    --network "${DOCKER_NETWORK}" \
    -e DOCKER_HOST=unix:///var/run/docker.sock \
    "${OP_STACK_IMAGE_TAG}" \
    bash -c "
        echo '📊 Verifying Docker connection:'
        apt-get update
        apt-get install docker.io -y
        docker --version
        docker ps --format 'table {{.Names}}\t{{.Status}}' | head -3

        echo '🚀 Running make reproducible-prestate...'
        make reproducible-prestate

        echo '📁 Checking contents of op-program/bin:'
        ls -la /app/op-program/bin/ || echo 'Directory is empty or does not exist'
    "

if [[ "$OSTYPE" != "darwin"* ]]; then
  umount  $RAMFS_DIR
  echo "un mounted ramfs at $RAMFS_DIR"
fi
