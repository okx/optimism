#!/bin/bash
# =============================================================================
# Step14: Deploy Custom Gas Token (optional)
# Function: Deploy OKB Token and Adapter contracts
# =============================================================================
set -e
set -x

# Change to test directory (parent of steps/)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$(dirname "$SCRIPT_DIR")"

source .env
source tools.sh
source utils.sh

echo "=========================================="
echo "Step14: DeployCustom Gas Token (CGT)"
echo "=========================================="

# Check if CGT is enabled
if [ "${ENABLE_CGT:-false}" != "true" ]; then
    echo "CGT not enabled (ENABLE_CGT!=true)skipping this step"
    exit 0
fi

ROOT_DIR=$(git rev-parse --show-toplevel)
PWD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PWD_DIR=$(dirname "$PWD_DIR")

cd $PWD_DIR

echo " Setting up Custom Gas Token (CGT) configuration..."

SYSTEM_CONFIG_PROXY_ADDRESS=$(jq -r '.opChainDeployments[0].SystemConfigProxy' "$CONFIG_DIR/state.json")
OPTIMISM_PORTAL_PROXY_ADDRESS=$(jq -r '.opChainDeployments[0].OptimismPortalProxy' "$CONFIG_DIR/state.json")

if [ -z "$SYSTEM_CONFIG_PROXY_ADDRESS" ] || [ "$SYSTEM_CONFIG_PROXY_ADDRESS" = "null" ]; then
  echo " Failed to read SystemConfigProxy from state.json"
  exit 1
fi

if [ -z "$OPTIMISM_PORTAL_PROXY_ADDRESS" ] || [ "$OPTIMISM_PORTAL_PROXY_ADDRESS" = "null" ]; then
  echo " Failed to read OptimismPortalProxy from state.json"
  exit 1
fi

echo " Running Foundry setup script for Custom Gas Token..."

cd $ROOT_DIR/packages/contracts-bedrock
export SYSTEM_CONFIG_PROXY_ADDRESS=$SYSTEM_CONFIG_PROXY_ADDRESS
export OPTIMISM_PORTAL_PROXY_ADDRESS=$OPTIMISM_PORTAL_PROXY_ADDRESS

FORGE_OUTPUT=$(forge script scripts/SetupCustomGasToken.s.sol:SetupCustomGasToken \
  --rpc-url "$L1_RPC_URL" \
  --private-key "$DEPLOYER_PRIVATE_KEY" \
  --broadcast 2>&1)

echo "$FORGE_OUTPUT"

# Extract contract addresses
OKB_TOKEN=$(echo "$FORGE_OUTPUT" | grep "MockOKB deployed at:" | awk '{print $NF}')
ADAPTER_ADDRESS=$(echo "$FORGE_OUTPUT" | grep "DepositedOKBAdapter deployed at:" | awk '{print $NF}')

# Query initial OKB total supply
INIT_TOTAL_SUPPLY=$(cast call "$OKB_TOKEN" "totalSupply()(uint256)" --rpc-url "$L1_RPC_URL")

echo ""
echo "SUCCESS: Step14completed: Custom Gas TokenDeploy"
echo "   OKB Token:    $OKB_TOKEN"
echo "   Adapter:      $ADAPTER_ADDRESS"
echo "   Total Supply: $INIT_TOTAL_SUPPLY"

