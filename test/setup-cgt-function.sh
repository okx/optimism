#!/bin/bash

sed_inplace() {
  if [[ "$OSTYPE" == "darwin"* ]]; then
    sed -i '' "$@"
  else
    sed -i "$@"
  fi
}

# Setup Custom Gas Token (CGT) function
setup_cgt() {
  echo "🔧 Setting up Custom Gas Token (CGT) configuration..."
  echo ""

  # Check if OKB_TOKEN_ADDRESS is already set in environment
  if [ -n "$OKB_TOKEN_ADDRESS" ]; then
    echo "📝 Step 1: Using existing OKB token..."
    echo "   Found OKB_TOKEN_ADDRESS in environment: $OKB_TOKEN_ADDRESS"
    echo ""

    # Verify the token exists at the specified address
    cd $ROOT_DIR/packages/contracts-bedrock
    echo "   Verifying OKB token at address..."

    # Try to call a basic function to verify it's a valid ERC20
    if ! cast call "$OKB_TOKEN_ADDRESS" "name()(string)" --rpc-url "$L1_RPC_URL" >/dev/null 2>&1; then
      echo ""
      echo "❌ ERROR: Invalid OKB token address or token not deployed"
      echo "   Address: $OKB_TOKEN_ADDRESS"
      echo "   Please check the address or remove OKB_TOKEN_ADDRESS from .env to deploy a new MockOKB"
      echo ""
      return 1
    fi

    TOKEN_NAME=$(cast call "$OKB_TOKEN_ADDRESS" "name()(string)" --rpc-url "$L1_RPC_URL")
    TOKEN_SYMBOL=$(cast call "$OKB_TOKEN_ADDRESS" "symbol()(string)" --rpc-url "$L1_RPC_URL")
    echo "   ✅ Token verified: $TOKEN_NAME ($TOKEN_SYMBOL)"
    echo ""
  else
    # Deploy MockOKB token if not already set
    echo "📝 Step 1: Deploying MockOKB token..."
    cd $ROOT_DIR/packages/contracts-bedrock

    # Temporarily disable set -e to capture forge output properly
    set +e
    MOCK_OKB_OUTPUT=$(forge script scripts/DeployMockOKB.s.sol:DeployMockOKB \
      --rpc-url "$L1_RPC_URL" \
      --private-key "$DEPLOYER_PRIVATE_KEY" \
      --broadcast 2>&1 | tee /dev/tty)
    MOCK_OKB_EXIT_CODE=$?
    set -e

    # Check if MockOKB deployment failed
    if [ $MOCK_OKB_EXIT_CODE -ne 0 ]; then
      echo ""
      echo "❌ ERROR: MockOKB deployment failed with exit code $MOCK_OKB_EXIT_CODE"
      echo "Error output shown above ☝️"
      echo ""
      return $MOCK_OKB_EXIT_CODE
    fi

    # Extract MockOKB contract address from forge output
    OKB_TOKEN_ADDRESS=$(echo "$MOCK_OKB_OUTPUT" | grep "MockOKB deployed at:" | awk '{print $NF}')

    if [ -z "$OKB_TOKEN_ADDRESS" ]; then
      echo ""
      echo "❌ ERROR: Could not extract MockOKB address from deployment output"
      echo "Please check the deployment logs above"
      echo ""
      return 1
    fi

    echo ""
    echo "✅ MockOKB deployed successfully!"
    echo "   Address: $OKB_TOKEN_ADDRESS"
    echo ""
    echo "💡 TIP: Add this to your .env file to reuse in future runs:"
    echo "   export OKB_TOKEN_ADDRESS=$OKB_TOKEN_ADDRESS"
    echo ""
  fi

  # Export OKB_TOKEN_ADDRESS for the setup script
  export OKB_TOKEN_ADDRESS="$OKB_TOKEN_ADDRESS"

  # Get required addresses from state.json
  echo "📝 Step 2: Running Custom Gas Token setup script..."
  STATE_JSON="$PWD_DIR/config-op/state.json"
  SYSTEM_CONFIG_PROXY_ADDRESS=$(jq -r '.opChainDeployments[0].SystemConfigProxy' "$STATE_JSON")
  OPTIMISM_PORTAL_PROXY_ADDRESS=$(jq -r '.opChainDeployments[0].OptimismPortalProxy' "$STATE_JSON")

  # Export required environment variables for the setup script
  export SYSTEM_CONFIG_PROXY_ADDRESS="$SYSTEM_CONFIG_PROXY_ADDRESS"
  export OPTIMISM_PORTAL_PROXY_ADDRESS="$OPTIMISM_PORTAL_PROXY_ADDRESS"

  # Temporarily disable set -e to capture forge output properly
  set +e
  FORGE_OUTPUT=$(forge script scripts/SetupCustomGasToken.s.sol:SetupCustomGasToken \
    --rpc-url "$L1_RPC_URL" \
    --private-key "$DEPLOYER_PRIVATE_KEY" \
    --broadcast 2>&1 | tee /dev/tty)
  FORGE_EXIT_CODE=$?
  set -e

  # Check if forge script failed
  if [ $FORGE_EXIT_CODE -ne 0 ]; then
    echo ""
    echo "❌ ERROR: Custom Gas Token setup failed with exit code $FORGE_EXIT_CODE"
    echo "Error output shown above ☝️"
    echo ""
    return $FORGE_EXIT_CODE
  fi

  # Extract adapter address from setup script output
  ADAPTER_ADDRESS=$(echo "$FORGE_OUTPUT" | grep "DepositedOKBAdapter deployed at:" | awk '{print $NF}')

  # Use the already deployed OKB token address
  OKB_TOKEN="$OKB_TOKEN_ADDRESS"

  # Query initial OKB total supply
  INIT_TOTAL_SUPPLY=$(cast call "$OKB_TOKEN" "totalSupply()(uint256)" --rpc-url "$L1_RPC_URL")
  echo ""
  echo "📊 Initial OKB Total Supply: $INIT_TOTAL_SUPPLY"

  echo ""
  echo "✅ L1 Custom Gas Token setup complete!"
  echo ""
  echo "📋 Setup Contract Addresses:"
  echo "   OKB Token:          $OKB_TOKEN"
  echo "   Adapter:            $ADAPTER_ADDRESS"
  echo ""

  # Save OKB_TOKEN_ADDRESS to .env file for the test script to use
  echo "💾 Updating .env with OKB token address..."

  # Update OKB_TOKEN_ADDRESS in .env
  sed_inplace "s/^OKB_TOKEN_ADDRESS=.*/OKB_TOKEN_ADDRESS=$OKB_TOKEN_ADDRESS/" "$PWD_DIR/.env"

  echo "   ✅ OKB token address updated in .env file"
  echo "   💡 ADAPTER_ADDRESS can be queried from SystemConfig.gasPayingToken()"
  echo "   💡 INIT_TOTAL_SUPPLY can be queried from OKB token contract"
  echo ""
}
