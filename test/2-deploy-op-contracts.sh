#!/bin/bash

set -e

sed_inplace() {
  if [[ "$OSTYPE" == "darwin"* ]]; then
    sed -i '' "$@"
  else
    sed -i "$@"
  fi
}

ROOT_DIR=$(git rev-parse --show-toplevel)
PWD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

cd $PWD_DIR

source .env

# Derive CHALLENGER address from OP_CHALLENGER_PRIVATE_KEY if not set
if [ -z "$CHALLENGER" ]; then
    CHALLENGER=$(cast wallet address $OP_CHALLENGER_PRIVATE_KEY)
    echo " ✅ Derived CHALLENGER address from private key: $CHALLENGER"
fi

# Deploy Gnosis Safe for l1ProxyAdminOwner
echo "🔧 Deploying Gnosis Safe for l1ProxyAdminOwner..."

# Deploy Safe using the DeploySimpleSafe script
SAFE_DEPLOY_OUTPUT=$(docker run --rm \
  --network "$DOCKER_NETWORK" \
  -v "$(pwd)/$CONFIG_DIR:/deployments" \
  -e DEPLOYER_PRIVATE_KEY="$DEPLOYER_PRIVATE_KEY" \
  -w /app/packages/contracts-bedrock \
  "${OP_CONTRACTS_IMAGE_TAG}" \
  forge script --json --broadcast \
    --rpc-url $L1_RPC_URL_IN_DOCKER \
    --private-key $DEPLOYER_PRIVATE_KEY \
    scripts/deploy/DeploySimpleSafe.s.sol:DeploySimpleSafe)

# Extract Safe address from deployment output
echo "🔍 Parsing deployment output..."

# Use a more robust approach to extract Safe address
SAFE_ADDRESS=$(echo "$SAFE_DEPLOY_OUTPUT" | jq -r '.logs[] | select(contains("New Safe L1ProxyAdminSafe deployed at:")) | split(": ")[1]' 2>/dev/null | head -1)

if [ -z "$SAFE_ADDRESS" ] || [ "$SAFE_ADDRESS" = "null" ]; then
  echo " ❌ Failed to extract Safe address from deployment output"
  echo "Deployment output: $SAFE_DEPLOY_OUTPUT"
  exit 1
fi

echo " ✅ Gnosis Safe deployed at: $SAFE_ADDRESS"

# Update .env file with Safe address
sed_inplace "s/SAFE_ADDRESS=.*/SAFE_ADDRESS=$SAFE_ADDRESS/" .env
source .env
echo " ✅ Updated SAFE_ADDRESS in .env: $SAFE_ADDRESS"

echo "🔧 Bootstrapping superchain with op-deployer..."

docker run --rm \
  --network "$DOCKER_NETWORK" \
  -v "$(pwd)/$CONFIG_DIR:/deployments" \
  "${OP_CONTRACTS_IMAGE_TAG}" \
  bash -c "
    set -e
    /app/op-deployer/bin/op-deployer bootstrap superchain \
      --l1-rpc-url $L1_RPC_URL_IN_DOCKER \
      --private-key $DEPLOYER_PRIVATE_KEY \
      --artifacts-locator file:///app/packages/contracts-bedrock/forge-artifacts \
      --superchain-proxy-admin-owner $SAFE_ADDRESS \
      --protocol-versions-owner $ADMIN_OWNER_ADDRESS \
      --guardian $ADMIN_OWNER_ADDRESS \
      --outfile /deployments/superchain.json
  "

echo "🔧 Bootstrapping implementations with op-deployer..."

SUPERCHAIN_JSON="$CONFIG_DIR/superchain.json"
PROTOCOL_VERSIONS_PROXY=$(jq -r '.protocolVersionsProxyAddress' "$SUPERCHAIN_JSON")
SUPERCHAIN_CONFIG_PROXY=$(jq -r '.superchainConfigProxyAddress' "$SUPERCHAIN_JSON")
PROXY_ADMIN=$(jq -r '.proxyAdminAddress' "$SUPERCHAIN_JSON")

docker run --rm \
  --network "$DOCKER_NETWORK" \
  -v "$(pwd)/$CONFIG_DIR:/deployments" \
  "${OP_CONTRACTS_IMAGE_TAG}" \
  bash -c "
    set -e
    /app/op-deployer/bin/op-deployer bootstrap implementations \
      --artifacts-locator file:///app/packages/contracts-bedrock/forge-artifacts \
      --l1-rpc-url $L1_RPC_URL_IN_DOCKER \
      --outfile /deployments/implementations.json \
      --mips-version "7" \
      --private-key $DEPLOYER_PRIVATE_KEY \
      --protocol-versions-proxy $PROTOCOL_VERSIONS_PROXY \
      --superchain-config-proxy $SUPERCHAIN_CONFIG_PROXY \
      --superchain-proxy-admin $PROXY_ADMIN \
      --upgrade-controller $ADMIN_OWNER_ADDRESS \
      --challenger $CHALLENGER \
      --challenge-period-seconds $CHALLENGE_PERIOD_SECONDS \
      --withdrawal-delay-seconds $WITHDRAWAL_DELAY_SECONDS \
      --proof-maturity-delay-seconds $WITHDRAWAL_DELAY_SECONDS \
      --dispute-game-finality-delay-seconds $DISPUTE_GAME_FINALITY_DELAY_SECONDS
  "

cp ./config-op/intent.toml.bak ./config-op/intent.toml
cp ./config-op/state.json.bak ./config-op/state.json
CHAIN_ID_UINT256=$(cast to-uint256 $CHAIN_ID)
sed_inplace 's/id = .*/id = "'"$CHAIN_ID_UINT256"'"/' ./config-op/intent.toml
echo " ✅ Updated chain id in intent.toml: $CHAIN_ID_UINT256"

# Update intent.toml with Safe address for l1ProxyAdminOwner
sed_inplace "s/l1ProxyAdminOwner = \".*\"/l1ProxyAdminOwner = \"$SAFE_ADDRESS\"/" ./config-op/intent.toml
echo " ✅ Updated l1ProxyAdminOwner in intent.toml: $SAFE_ADDRESS"

# Read opcmAddress from implementations.json and write it into intent.toml
OPCM_ADDRESS=$(jq -r '.opcmAddress' ./config-op/implementations.json)
if [ -z "$OPCM_ADDRESS" ] || [ "$OPCM_ADDRESS" = "null" ]; then
  echo " ❌ Failed to read opcmAddress from implementations.json"
  exit 1
fi

# Replace the opcmAddress field in intent.toml with the new value
sed_inplace "s/^opcmAddress = \".*\"/opcmAddress = \"$OPCM_ADDRESS\"/" ./config-op/intent.toml
echo " ✅ Updated opcmAddress ($OPCM_ADDRESS) in intent.toml"

# deploy contracts, TODO, should we need to modify source code to deploy contracts?
docker run --rm \
  --network "$DOCKER_NETWORK" \
  -v "$(pwd)/$CONFIG_DIR:/deployments" \
  "${OP_CONTRACTS_IMAGE_TAG}" \
  bash -c "
    set -e
    echo '🔧 Starting contract deployment with op-deployer...'

    # Deploy using op-deployer, wait for completion before proceeding
    /app/op-deployer/bin/op-deployer apply \
      --workdir /deployments \
      --private-key $DEPLOYER_PRIVATE_KEY \
      --l1-rpc-url $L1_RPC_URL_IN_DOCKER

    echo ' 📄 Generating L2 genesis and rollup config...'

    # Generate L2 genesis using op-deployer
    /app/op-deployer/bin/op-deployer inspect genesis \
      --workdir /deployments \
      $CHAIN_ID > /deployments/genesis.json

    # Generate L2 rollup using op-node
    /app/op-deployer/bin/op-deployer inspect rollup \
      --workdir /deployments \
      $CHAIN_ID > /deployments/rollup.json

    echo ' ✅ Contract deployment completed successfully'
  "

echo "genesis.json and rollup.json are generated in deployments folder"

echo "🎉 OP Stack deployment preparation completed!"
