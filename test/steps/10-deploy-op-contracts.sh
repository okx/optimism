#!/bin/bash
# =============================================================================
# Step10-13: Deploy OP Stack contracts (complete flow)
# Function: Deploy Transactor, Superchain, Implementations, OpChain
# Output: genesis.json  rollup.json
#
# Notes: This script contains the complete contract deployment flow, as these steps have tight dependencies
# For finer-grained splitting, functions like deploy_transactor_contract can be separated
# =============================================================================
set -e
set -x

# Change to test directory (parent of steps/)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$(dirname "$SCRIPT_DIR")"

source .env
source tools.sh
source utils.sh

ROOT_DIR=$(git rev-parse --show-toplevel)
PWD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PWD_DIR=$(dirname "$PWD_DIR")  # testdirectory

echo "=========================================="
echo "Step10-13: DeployOP Stackcontract"
echo "=========================================="

if [ "$ENV" = "local" ]; then
    DOCKER_NETWORK_ARG="$DOCKER_NETWORK"
else
    DOCKER_NETWORK_ARG="host"
fi

if [ -z "$CHAIN_ID" ]; then
  echo " ERROR: CHAIN_ID is not set"
  exit 1
fi

if ! [[ "$CHAIN_ID" =~ ^[0-9]+$ ]]; then
  echo " ERROR: CHAIN_ID must be a numeric value, got: '$CHAIN_ID'"
  exit 1
fi

cd $PWD_DIR

# PreparingConfigfile
echo " PreparingConfigfile..."
cp ./config-op/intent.${ENV}.toml.bak ./config-op/intent.toml
cp ./config-op/state.json.bak ./config-op/state.json

CHAIN_ID_UINT256=$(cast to-uint256 $CHAIN_ID)
sed_inplace 's/id = .*/id = "'"$CHAIN_ID_UINT256"'"/' ./config-op/intent.toml
echo "SUCCESS: Updated chain id in intent.toml: $CHAIN_ID_UINT256"

# DeployTransactorcontract
echo ""
echo "Step10: DeployTransactorcontract..."
deploy_transactor_contract() {
  echo " Deploying Transactor contract..."

  DOCKER_ARGS=("--rm")
  DOCKER_ARGS+=("-v" "$(pwd)/$CONFIG_DIR:/deployments")
  DOCKER_ARGS+=("-w" "/app/packages/contracts-bedrock")

  if [ "$ENV" = "local" ]; then
    DOCKER_ARGS+=("--network" "$DOCKER_NETWORK")
  else
    DOCKER_ARGS+=("--network" "host")
  fi

  DOCKER_ARGS+=("$OP_CONTRACTS_IMAGE_TAG")

  FORGE_CMD="forge create --json --broadcast --legacy \
    --rpc-url $L1_RPC_URL_IN_DOCKER \
    --private-key $DEPLOYER_PRIVATE_KEY \
    src/periphery/Transactor.sol:Transactor \
    --constructor-args $ADMIN_OWNER_ADDRESS"

  TRANSACTOR_DEPLOY_OUTPUT=$(docker run "${DOCKER_ARGS[@]}" $FORGE_CMD)

  # Extractcontractaddress
  TRANSACTOR_ADDRESS=$(echo "$TRANSACTOR_DEPLOY_OUTPUT" | jq -r '.deployedTo // empty' 2>/dev/null || echo "")
  if [ -z "$TRANSACTOR_ADDRESS" ] || [ "$TRANSACTOR_ADDRESS" = "null" ]; then
    TRANSACTOR_ADDRESS=$(echo "$TRANSACTOR_DEPLOY_OUTPUT" | grep -o '0x[a-fA-F0-9]\{40\}' | head -1 || echo "")
  fi

  if [ -z "$TRANSACTOR_ADDRESS" ]; then
    echo " Failed to extract Transactor contract address"
    exit 1
  fi

  echo "SUCCESS: Transactor contract deployed at: $TRANSACTOR_ADDRESS"

  sed_inplace "s/TRANSACTOR=.*/TRANSACTOR=$TRANSACTOR_ADDRESS/" .env
  source .env
}

deploy_transactor_contract

# DeploySuperchain
echo ""
echo "Step11: Bootstrap Superchain..."
deploy_op_stack_bootstrap_superchain() {
  source .env
  TRANSACTOR_ADDRESS=${TRANSACTOR}
  echo " Bootstrapping superchain with op-deployer..."

  DOCKER_ARGS=("-v" "$(pwd)/$CONFIG_DIR:/deployments")
  DOCKER_ARGS+=("-w" "/app")
  DOCKER_ARGS+=("-e" "CURL_CA_BUNDLE=")
  DOCKER_ARGS+=("-e" "GIT_SSL_NO_VERIFY=true")
  DOCKER_ARGS+=("-e" "NODE_TLS_REJECT_UNAUTHORIZED=0")
  DOCKER_ARGS+=("-e" "GODEBUG=x509ignoreCN=1,x509ignoreUnknownCA=1,x509ignoreSystemRoots=1")
  DOCKER_ARGS+=("--network" "$DOCKER_NETWORK_ARG")
  DOCKER_ARGS+=("$OP_CONTRACTS_IMAGE_TAG")

  BASH_CMD="set -e && /app/op-deployer/bin/op-deployer bootstrap superchain --l1-rpc-url $L1_RPC_URL_IN_DOCKER --private-key $DEPLOYER_PRIVATE_KEY --artifacts-locator file:///app/packages/contracts-bedrock/forge-artifacts --superchain-proxy-admin-owner $TRANSACTOR_ADDRESS --protocol-versions-owner $ADMIN_OWNER_ADDRESS --guardian $ADMIN_OWNER_ADDRESS --outfile /deployments/superchain.json"

  docker run "${DOCKER_ARGS[@]}" bash -c "$BASH_CMD"
}

deploy_op_stack_bootstrap_superchain

# DeployImplementations
echo ""
echo "Step12: Bootstrap Implementations..."
deploy_op_stack_bootstrap_implementations() {
  source .env
  TRANSACTOR_ADDRESS=${TRANSACTOR}
  echo " Bootstrapping implementations with op-deployer..."

  SUPERCHAIN_JSON="$CONFIG_DIR/superchain.json"
  PROTOCOL_VERSIONS_PROXY=$(jq -r '.protocolVersionsProxyAddress' "$SUPERCHAIN_JSON")
  SUPERCHAIN_CONFIG_PROXY=$(jq -r '.superchainConfigProxyAddress' "$SUPERCHAIN_JSON")
  PROXY_ADMIN=$(jq -r '.proxyAdminAddress' "$SUPERCHAIN_JSON")

  DOCKER_ARGS=("-v" "$(pwd)/$CONFIG_DIR:/deployments")
  DOCKER_ARGS+=("-w" "/app")
  DOCKER_ARGS+=("-e" "CURL_CA_BUNDLE=")
  DOCKER_ARGS+=("-e" "GIT_SSL_NO_VERIFY=true")
  DOCKER_ARGS+=("-e" "NODE_TLS_REJECT_UNAUTHORIZED=0")
  DOCKER_ARGS+=("--network" "$DOCKER_NETWORK_ARG")
  DOCKER_ARGS+=("$OP_CONTRACTS_IMAGE_TAG")

  BASH_CMD="set -e && export CURL_CA_BUNDLE= && export GIT_SSL_NO_VERIFY=true && /app/op-deployer/bin/op-deployer bootstrap implementations --artifacts-locator file:///app/packages/contracts-bedrock/forge-artifacts --l1-rpc-url $L1_RPC_URL_IN_DOCKER --outfile /deployments/implementations.json --mips-version \"7\" --private-key $DEPLOYER_PRIVATE_KEY --protocol-versions-proxy $PROTOCOL_VERSIONS_PROXY --superchain-config-proxy $SUPERCHAIN_CONFIG_PROXY --superchain-proxy-admin $PROXY_ADMIN --upgrade-controller $ADMIN_OWNER_ADDRESS --challenger $CHALLENGER_ADDRESS --challenge-period-seconds $CHALLENGE_PERIOD_SECONDS --withdrawal-delay-seconds $WITHDRAWAL_DELAY_SECONDS --proof-maturity-delay-seconds $PROOF_MATURITY_DELAY_SECONDS --dispute-game-finality-delay-seconds $DISPUTE_GAME_FINALITY_DELAY_SECONDS --dev-feature-bitmap 0x0000000000000000000000000000000000000000000000000000000000001000"

  docker run "${DOCKER_ARGS[@]}" bash -c "$BASH_CMD"

  # Update intent.toml
  sed_inplace "s/l1ProxyAdminOwner = \".*\"/l1ProxyAdminOwner = \"$TRANSACTOR_ADDRESS\"/" ./config-op/intent.toml

  OPCM_ADDRESS=$(jq -r '.opcmAddress' ./config-op/implementations.json)
  if [ -z "$OPCM_ADDRESS" ] || [ "$OPCM_ADDRESS" = "null" ]; then
    echo " Failed to read opcmAddress from implementations.json"
    exit 1
  fi

  sed_inplace "s/^opcmAddress = \".*\"/opcmAddress = \"$OPCM_ADDRESS\"/" ./config-op/intent.toml
  echo "SUCCESS: Updated opcmAddress ($OPCM_ADDRESS) in intent.toml"
}

deploy_op_stack_bootstrap_implementations

# DeployOpChainGenerategenesis.jsonrollup.json
echo ""
echo "Step13: DeployOpChainGenerategenesis.jsonrollup.json..."
deploy_op_stack_contracts() {
  DOCKER_ARGS=("-v" "$(pwd)/$CONFIG_DIR:/deployments")
  DOCKER_ARGS+=("-w" "/app")
  DOCKER_ARGS+=("-e" "CURL_CA_BUNDLE=")
  DOCKER_ARGS+=("-e" "GIT_SSL_NO_VERIFY=true")
  DOCKER_ARGS+=("-e" "NODE_TLS_REJECT_UNAUTHORIZED=0")
  DOCKER_ARGS+=("--network" "$DOCKER_NETWORK_ARG")
  DOCKER_ARGS+=("$OP_CONTRACTS_IMAGE_TAG")

  BASH_CMD="set -e && export CURL_CA_BUNDLE= && export GIT_SSL_NO_VERIFY=true && echo ' Starting contract deployment with op-deployer...' && /app/op-deployer/bin/op-deployer apply --workdir /deployments --private-key $DEPLOYER_PRIVATE_KEY --l1-rpc-url $L1_RPC_URL_IN_DOCKER && echo ' Generating L2 genesis and rollup config...' && /app/op-deployer/bin/op-deployer inspect genesis --workdir /deployments $CHAIN_ID > /deployments/genesis.json && /app/op-deployer/bin/op-deployer inspect rollup --workdir /deployments $CHAIN_ID > /deployments/rollup.json && echo 'SUCCESS: Contract deployment completed successfully'"

  docker run "${DOCKER_ARGS[@]}" bash -c "$BASH_CMD"

  echo "SUCCESS: genesis.json and rollup.json generated in $CONFIG_DIR/"
}

deploy_op_stack_contracts

echo ""
echo "SUCCESS: Step10-13completed: OP StackcontractDeploy"
echo "   file:"
echo "   - $CONFIG_DIR/genesis.json"
echo "   - $CONFIG_DIR/rollup.json"
echo "   - $CONFIG_DIR/state.json"
echo "   - $CONFIG_DIR/implementations.json"
echo "   - $CONFIG_DIR/superchain.json"

