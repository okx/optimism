#!/bin/bash
set -e
set -x

source .env
source tools.sh
source utils.sh

ROOT_DIR=$(git rev-parse --show-toplevel)

# Set global OP_CONTRACTS_IMAGE_TAG based on environment
if [ "$ENV" = "local" ]; then
    # Use local image tag for local environment
    DOCKER_NETWORK_ARG="$DOCKER_NETWORK"
else
    # Use cert image tag for non-local environments
    DOCKER_NETWORK_ARG="host"
fi

if [ -z "$CHAIN_ID" ]; then
  echo "❌ ERROR: CHAIN_ID is not set. Set it explicitly or derive it from intent.toml before proceeding."
  exit 1
fi
if ! [[ "$CHAIN_ID" =~ ^[0-9]+$ ]]; then
  echo "❌ ERROR: CHAIN_ID must be a numeric value, got: '$CHAIN_ID'"
  exit 1
fi

cd $PWD_DIR

deploy_safe() {
    echo "=== Deploying Gnosis Safe ==="

    # Use deployer as single owner with threshold 1
    echo "Using deployer as single owner with threshold 1"

    # Execute Safe deployment
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

    # Extract Safe address
    SAFE_ADDRESS=$(echo "$SAFE_DEPLOY_OUTPUT" | jq -r '.logs[] | select(contains("New Safe L1ProxyAdminSafe deployed at:")) | split(": ")[1]' 2>/dev/null | head -1)

    if [ -z "$SAFE_ADDRESS" ] || [ "$SAFE_ADDRESS" = "null" ]; then
        echo "❌ Failed to deploy Safe"
        exit 1
    fi

    echo "✅ Safe deployed at: $SAFE_ADDRESS"
    echo "   Owner: $(cast wallet address --private-key $DEPLOYER_PRIVATE_KEY)"
    echo "   Threshold: 1"

    # Update .env file
    sed_inplace "s/SAFE_ADDRESS=.*/SAFE_ADDRESS=$SAFE_ADDRESS/" .env
    source .env
    echo " ✅ Updated SAFE_ADDRESS in .env: $SAFE_ADDRESS"
}

deploy_transactor() {
  # Deploy Transactor contract first
  echo "🔧 Deploying Transactor contract..."

  # Debug: Show environment variables
  echo "ENV: $ENV"
  echo "CHAIN_ID: $CHAIN_ID"
  echo "DOCKER_NETWORK: $DOCKER_NETWORK"
  echo "L1_RPC_URL_IN_DOCKER: $L1_RPC_URL_IN_DOCKER"
  echo "DEPLOYER_PRIVATE_KEY: ${DEPLOYER_PRIVATE_KEY:0:10}..."
  echo "ADMIN_OWNER_ADDRESS: $ADMIN_OWNER_ADDRESS"
  echo "OP_CONTRACTS_IMAGE_TAG: $OP_CONTRACTS_IMAGE_TAG"

  # Build docker run command with conditional network flag
  DOCKER_ARGS=()
  DOCKER_ARGS+=("--rm")
  DOCKER_ARGS+=("-v" "$(pwd)/$CONFIG_DIR:/deployments")
  DOCKER_ARGS+=("-w" "/app/packages/contracts-bedrock")

  if [ "$ENV" = "local" ]; then
    DOCKER_ARGS+=("--network" "$DOCKER_NETWORK")
    echo "✅ Using Docker network: $DOCKER_NETWORK"
  else
    DOCKER_ARGS+=("--network" "host")
    echo "✅ Skipping Docker network (ENV=$ENV)"
  fi

  DOCKER_ARGS+=("$OP_CONTRACTS_IMAGE_TAG")

  # Create the forge create command
  FORGE_CMD="forge create --json --broadcast --legacy \
    --rpc-url $L1_RPC_URL_IN_DOCKER \
    --private-key $DEPLOYER_PRIVATE_KEY \
    src/periphery/Transactor.sol:Transactor.0.8.30 \
    --constructor-args $ADMIN_OWNER_ADDRESS"

  echo "🔧 Executing Docker command..."
  echo "Command: docker run ${DOCKER_ARGS[*]} $FORGE_CMD"

  TRANSACTOR_DEPLOY_OUTPUT=$(docker run "${DOCKER_ARGS[@]}" $FORGE_CMD)

  echo "Raw deployment output:"
  echo "$TRANSACTOR_DEPLOY_OUTPUT"
  echo "--- End of raw output ---"

  # Extract contract address from deployment output
  TRANSACTOR_ADDRESS=$(echo "$TRANSACTOR_DEPLOY_OUTPUT" | jq -r '.deployedTo // empty' 2>/dev/null || echo "")
  if [ -z "$TRANSACTOR_ADDRESS" ] || [ "$TRANSACTOR_ADDRESS" = "null" ]; then
    echo "❌ Failed to extract Transactor contract address from deployment output"
    echo "Deployment output: $TRANSACTOR_DEPLOY_OUTPUT"
    echo "Trying to extract address manually..."

    # Try alternative extraction methods for forge output
    TRANSACTOR_ADDRESS=$(echo "$TRANSACTOR_DEPLOY_OUTPUT" | jq -r '.deployedTo' 2>/dev/null || echo "")
    if [ -z "$TRANSACTOR_ADDRESS" ]; then
      TRANSACTOR_ADDRESS=$(echo "$TRANSACTOR_DEPLOY_OUTPUT" | grep -o '"deployedTo":"[^"]*"' | cut -d'"' -f4 || echo "")
    fi
    if [ -z "$TRANSACTOR_ADDRESS" ]; then
      TRANSACTOR_ADDRESS=$(echo "$TRANSACTOR_DEPLOY_OUTPUT" | grep -o '0x[a-fA-F0-9]\{40\}' | head -1 || echo "")
    fi

    if [ -z "$TRANSACTOR_ADDRESS" ]; then
      echo "❌ Still failed to extract contract address"
      exit 1
    else
      echo "✅ Extracted address manually: $TRANSACTOR_ADDRESS"
    fi
  fi

  echo "✅ Transactor contract deployed at: $TRANSACTOR_ADDRESS"

  # Update .env file with Transactor address
  sed_inplace "s/TRANSACTOR=.*/TRANSACTOR=$TRANSACTOR_ADDRESS/" .env
  source .env
  echo "✅ Updated TRANSACTOR address in .env: $TRANSACTOR_ADDRESS"
}

# Bootstrapping superchain with op-deployer
# Output: after deploy, it will output `superchain.json` under config-op
# e.g. {
#  "protocolVersionsImplAddress": "0x37e15e4d6dffa9e5e320ee1ec036922e563cb76c",
#  "protocolVersionsProxyAddress": "0xfb5a7622e23e0f807b97a8ed608d50d56d202688",
#  "superchainConfigImplAddress": "0xce28685eb204186b557133766eca00334eb441e4",
#  "superchainConfigProxyAddress": "0x8c15b9d397b5bf29e114aebff0663fdd34976756",
#  "proxyAdminAddress": "0x210879bec4c74c7e4e6df5e919f9525d75e15183"
# }
deploy_op_stack_bootstrap_superchain() {
  source .env
  echo "🔧 Bootstrapping superchain with op-deployer..."

  # Build docker run command with conditional network flag
  DOCKER_ARGS=()
  DOCKER_ARGS+=("-v" "$(pwd)/$CONFIG_DIR:/deployments")
  DOCKER_ARGS+=("-w" "/app")
  DOCKER_ARGS+=("-e" "CURL_CA_BUNDLE=")
  DOCKER_ARGS+=("-e" "GIT_SSL_NO_VERIFY=true")
  DOCKER_ARGS+=("-e" "NODE_TLS_REJECT_UNAUTHORIZED=0")
  DOCKER_ARGS+=("-e" "GODEBUG=x509ignoreCN=1,x509ignoreUnknownCA=1,x509ignoreSystemRoots=1")
  DOCKER_ARGS+=("--network" "$DOCKER_NETWORK_ARG")

  DOCKER_ARGS+=("$OP_CONTRACTS_IMAGE_TAG")

  BASH_CMD="set -e && /app/op-deployer/bin/op-deployer bootstrap superchain --l1-rpc-url $L1_RPC_URL_IN_DOCKER --private-key $DEPLOYER_PRIVATE_KEY --artifacts-locator file:///app/packages/contracts-bedrock/forge-artifacts --superchain-proxy-admin-owner $L1_PROXY_ADMIN_OWNER --protocol-versions-owner $ADMIN_OWNER_ADDRESS --guardian $ADMIN_OWNER_ADDRESS --outfile /deployments/superchain.json"

  docker run "${DOCKER_ARGS[@]}" bash -c "$BASH_CMD"
}

deploy_op_stack_bootstrap_implementations() {
  source .env
  echo "🔧 Bootstrapping implementations with op-deployer..."
  SUPERCHAIN_JSON="$CONFIG_DIR/superchain.json"
  PROTOCOL_VERSIONS_PROXY=$(jq -r '.protocolVersionsProxyAddress' "$SUPERCHAIN_JSON")
  SUPERCHAIN_CONFIG_PROXY=$(jq -r '.superchainConfigProxyAddress' "$SUPERCHAIN_JSON")
  PROXY_ADMIN=$(jq -r '.proxyAdminAddress' "$SUPERCHAIN_JSON")
  # Build docker run command with conditional network flag
  DOCKER_ARGS=()
  DOCKER_ARGS+=("-v" "$(pwd)/$CONFIG_DIR:/deployments")
  DOCKER_ARGS+=("-w" "/app")
  DOCKER_ARGS+=("-e" "CURL_CA_BUNDLE=")
  DOCKER_ARGS+=("-e" "GIT_SSL_NO_VERIFY=true")
  DOCKER_ARGS+=("-e" "NODE_TLS_REJECT_UNAUTHORIZED=0")
  DOCKER_ARGS+=("--network" "$DOCKER_NETWORK_ARG")

  DOCKER_ARGS+=("$OP_CONTRACTS_IMAGE_TAG")

  # Build the base command
  BASH_CMD="set -e && export CURL_CA_BUNDLE= && export GIT_SSL_NO_VERIFY=true && /app/op-deployer/bin/op-deployer bootstrap implementations --artifacts-locator file:///app/packages/contracts-bedrock/forge-artifacts --l1-rpc-url $L1_RPC_URL_IN_DOCKER --outfile /deployments/implementations.json --mips-version \"7\" --private-key $DEPLOYER_PRIVATE_KEY --protocol-versions-proxy $PROTOCOL_VERSIONS_PROXY --superchain-config-proxy $SUPERCHAIN_CONFIG_PROXY --superchain-proxy-admin $PROXY_ADMIN --upgrade-controller $ADMIN_OWNER_ADDRESS --challenger $CHALLENGER_ADDRESS --challenge-period-seconds $CHALLENGE_PERIOD_SECONDS --withdrawal-delay-seconds $WITHDRAWAL_DELAY_SECONDS --proof-maturity-delay-seconds $PROOF_MATURITY_DELAY_SECONDS --dispute-game-finality-delay-seconds $DISPUTE_GAME_FINALITY_DELAY_SECONDS"

  # Add dev-feature-bitmap only when CGT_ENABLED=true
  if [ "$CGT_ENABLED" = "true" ]; then
    BASH_CMD="$BASH_CMD --dev-feature-bitmap 0x0000000000000000000000000000000000000000000000000000000000001000"
  fi

  docker run "${DOCKER_ARGS[@]}" bash -c "$BASH_CMD"

  # Update intent.toml
  sed_inplace "s/l1ProxyAdminOwner = .*/l1ProxyAdminOwner = \"$L1_PROXY_ADMIN_OWNER\"/" "$CONFIG_DIR/intent.toml"
  echo " ✅ Updated intent.toml with $OWNER_TYPE owner: $L1_PROXY_ADMIN_OWNER"

  # Read opcmAddress from implementations.json and write it into intent.toml
  OPCM_ADDRESS=$(jq -r '.opcmAddress' ./config-op/implementations.json)
  if [ -z "$OPCM_ADDRESS" ] || [ "$OPCM_ADDRESS" = "null" ]; then
    echo "❌ Failed to read opcmAddress from implementations.json"
    exit 1
  fi

  # Replace the opcmAddress field in intent.toml with the new value
  sed_inplace "s/^opcmAddress = \".*\"/opcmAddress = \"$OPCM_ADDRESS\"/" ./config-op/intent.toml
  echo "✅ Updated opcmAddress ($OPCM_ADDRESS) in intent.toml"
}

deploy_op_stack_contracts() {
  # Deploy contracts, TODO: should we need to modify source code to deploy contracts?
  # Build docker run command with conditional network flag
  DOCKER_ARGS=()
  DOCKER_ARGS+=("-v" "$(pwd)/$CONFIG_DIR:/deployments")
  DOCKER_ARGS+=("-w" "/app")
  DOCKER_ARGS+=("-e" "CURL_CA_BUNDLE=")
  DOCKER_ARGS+=("-e" "GIT_SSL_NO_VERIFY=true")
  DOCKER_ARGS+=("-e" "NODE_TLS_REJECT_UNAUTHORIZED=0")
  DOCKER_ARGS+=("--network" "$DOCKER_NETWORK_ARG")

  DOCKER_ARGS+=("$OP_CONTRACTS_IMAGE_TAG")

  BASH_CMD="set -e && export CURL_CA_BUNDLE= && export GIT_SSL_NO_VERIFY=true && echo '🔧 Starting contract deployment with op-deployer...' && echo '' && echo 'Deploy using op-deployer, wait for completion before proceeding' && /app/op-deployer/bin/op-deployer apply --workdir /deployments --private-key $DEPLOYER_PRIVATE_KEY --l1-rpc-url $L1_RPC_URL_IN_DOCKER && echo '' && echo '📄 Generating L2 genesis and rollup config...' && echo '' && echo 'Generate L2 genesis using op-deployer' && /app/op-deployer/bin/op-deployer inspect genesis --workdir /deployments $CHAIN_ID > /deployments/genesis.json && echo '' && echo 'Generate L2 rollup using op-node' && /app/op-deployer/bin/op-deployer inspect rollup --workdir /deployments $CHAIN_ID > /deployments/rollup.json && echo '' && echo '✅ Contract deployment completed successfully'"

  docker run "${DOCKER_ARGS[@]}" bash -c "$BASH_CMD"

  echo "genesis.json and rollup.json are generated in deployments folder"
  echo "🎉 OP Stack deployment preparation completed!"
}

deploy_custom_gas_token() {
  echo "🔧 Setting up Custom Gas Token (CGT) configuration..."
  echo ""

  SYSTEM_CONFIG_PROXY_ADDRESS=$(jq -r '.opChainDeployments[0].SystemConfigProxy' "$CONFIG_DIR/state.json")
  OPTIMISM_PORTAL_PROXY_ADDRESS=$(jq -r '.opChainDeployments[0].OptimismPortalProxy' "$CONFIG_DIR/state.json")

  if [ -z "$SYSTEM_CONFIG_PROXY_ADDRESS" ] || [ "$SYSTEM_CONFIG_PROXY_ADDRESS" = "null" ]; then
    echo "❌ Failed to read systemConfigProxyAddress from state.json"
    exit 1
  fi
  if [ -z "$OPTIMISM_PORTAL_PROXY_ADDRESS" ] || [ "$OPTIMISM_PORTAL_PROXY_ADDRESS" = "null" ]; then
    echo "❌ Failed to read optimismPortalProxyAddress from state.json"
    exit 1
  fi
  echo "📝 Running Foundry setup script for Custom Gas Token..."

  cd $ROOT_DIR/packages/contracts-bedrock
  export SYSTEM_CONFIG_PROXY_ADDRESS=$SYSTEM_CONFIG_PROXY_ADDRESS
  export OPTIMISM_PORTAL_PROXY_ADDRESS=$OPTIMISM_PORTAL_PROXY_ADDRESS

  FORGE_OUTPUT=$(forge script scripts/SetupCustomGasToken.s.sol:SetupCustomGasToken \
    --rpc-url "$L1_RPC_URL" \
    --private-key "$DEPLOYER_PRIVATE_KEY" \
    --broadcast 2>&1)

  echo "$FORGE_OUTPUT"

  # Extract contract addresses from forge output
  ADAPTER_ADDRESS=$(echo "$FORGE_OUTPUT" | grep "DepositedOKBAdapter deployed at:" | awk '{print $NF}')

  # Query initial OKB total supply
  INIT_TOTAL_SUPPLY=$(cast call "$OKB_TOKEN_ADDRESS" "totalSupply()(uint256)" --rpc-url "$L1_RPC_URL")
  echo ""
  echo "📊 Initial OKB Total Supply: $INIT_TOTAL_SUPPLY"

  echo ""
  echo "✅ L1 Custom Gas Token setup complete!"
  echo "📋 Deployed Contract Addresses:"
  echo ""
  echo "   OKB Token:          $OKB_TOKEN"
  echo "   Adapter:            $ADAPTER_ADDRESS"
  echo ""

}

echo "CGT_ENABLED: ${CGT_ENABLED}"

cp ./config-op/intent.${ENV}.toml.bak ./config-op/intent.toml
cp ./config-op/state.json.bak ./config-op/state.json

CHAIN_ID_UINT256=$(cast to-uint256 $CHAIN_ID)
sed_inplace 's/id = .*/id = "'"$CHAIN_ID_UINT256"'"/' ./config-op/intent.toml
echo " ✅ Updated chain id in intent.toml: $CHAIN_ID_UINT256"

# Validate OWNER_TYPE configuration
if [ "$OWNER_TYPE" != "transactor" ] && [ "$OWNER_TYPE" != "safe" ]; then
    echo "❌ Error: Invalid OWNER_TYPE '$OWNER_TYPE'. Must be 'transactor' or 'safe'"
    exit 1
fi

echo "=== Deploying with OWNER_TYPE: $OWNER_TYPE ==="

# Deploy owner contract based on OWNER_TYPE
if [ "$OWNER_TYPE" = "safe" ]; then
    echo "🔧 Deploying Gnosis Safe for l1ProxyAdminOwner..."
    deploy_safe
    L1_PROXY_ADMIN_OWNER=$SAFE_ADDRESS
elif [ "$OWNER_TYPE" = "transactor" ]; then
    echo "🔧 Deploying Transactor for l1ProxyAdminOwner..."
    deploy_transactor
    L1_PROXY_ADMIN_OWNER=$TRANSACTOR_ADDRESS
fi

echo "Using $OWNER_TYPE as l1ProxyAdminOwner: $L1_PROXY_ADMIN_OWNER"

deploy_op_stack_bootstrap_superchain
deploy_op_stack_bootstrap_implementations
deploy_op_stack_contracts

if [ "$CGT_ENABLED" = "true" ]; then
  deploy_custom_gas_token
fi
