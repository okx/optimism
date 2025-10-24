#!/bin/bash
set -e

ROOT_DIR=$(git rev-parse --show-toplevel)

cd $ROOT_DIR

source test/.env

STATE_JSON="$ROOT_DIR/test/config-op/state.json"
OP_PROXY_ADMIN=$(jq -r '.opChainDeployments[0].OpChainProxyAdminImpl' "$STATE_JSON")
SYSTEM_CONFIG_PROXY_ADDRESS=$(jq -r '.opChainDeployments[0].SystemConfigProxy' "$STATE_JSON")
OPTIMISM_PORTAL_PROXY_ADDRESS=$(jq -r '.opChainDeployments[0].OptimismPortalProxy' "$STATE_JSON")
# Query ADAPTER_ADDRESS from SystemConfig.gasPayingToken()
ADAPTER_ADDRESS=$(cast call "$SYSTEM_CONFIG_PROXY_ADDRESS" "gasPayingToken()(address,uint8)" --rpc-url "$L1_RPC_URL" | head -n1)
if [ -z "$ADAPTER_ADDRESS" ] || [ "$ADAPTER_ADDRESS" = "0x0000000000000000000000000000000000000000" ]; then
  echo "❌ ERROR: Could not query ADAPTER_ADDRESS from SystemConfig or CGT not configured"
  echo "   SystemConfig address: $SYSTEM_CONFIG_PROXY_ADDRESS"
  exit 1
fi

# Query OKB_TOKEN_ADDRESS from the adapter
OKB_TOKEN_ADDRESS=$(cast call "$ADAPTER_ADDRESS" "OKB()(address)" --rpc-url "$L1_RPC_URL")
if [ -z "$OKB_TOKEN_ADDRESS" ] || [ "$OKB_TOKEN_ADDRESS" = "0x0000000000000000000000000000000000000000" ]; then
  echo "❌ ERROR: Could not query OKB_TOKEN_ADDRESS from adapter"
  echo "   Adapter address: $ADAPTER_ADDRESS"
  exit 1
fi

echo "🔧 Upgrading SystemConfig, deploying new DepositedOKBAdapter, and updating SystemConfig to use it as gas paying token..."
cd $ROOT_DIR/packages/contracts-bedrock
export SYSTEM_CONFIG_PROXY_ADDRESS=$SYSTEM_CONFIG_PROXY_ADDRESS
export OPTIMISM_PORTAL_PROXY_ADDRESS=$OPTIMISM_PORTAL_PROXY_ADDRESS
export OKB_TOKEN_ADDRESS=$OKB_TOKEN_ADDRESS
export OP_PROXY_ADMIN=$OP_PROXY_ADMIN
export TRANSACTOR=$TRANSACTOR
export OKB_ADAPTER_OWNER_ADDRESS=$OKB_ADAPTER_OWNER_ADDRESS

forge script scripts/UpgradeSystemConfigToV312.s.sol:UpgradeSystemConfig \
  --rpc-url $L1_RPC_URL \
  --private-key $DEPLOYER_PRIVATE_KEY \
  --broadcast
