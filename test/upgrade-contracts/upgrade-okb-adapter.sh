#!/bin/bash
set -e

ROOT_DIR=$(git rev-parse --show-toplevel)

cd $ROOT_DIR

STATE_JSON="$ROOT_DIR/test/config-op/state.json"
OP_PROXY_ADMIN=$(jq -r '.opChainDeployments[0].OpChainProxyAdminImpl' "$STATE_JSON")

source test/.env

echo "🔧 Upgrading SystemConfig, deploying new DepositedOKBAdapter, and updating SystemConfig to use it as gas paying token..."
cd $ROOT_DIR/packages/contracts-bedrock
export SYSTEM_CONFIG_PROXY_ADDRESS=$SYSTEM_CONFIG_PROXY_ADDRESS
export OPTIMISM_PORTAL_PROXY_ADDRESS=$OPTIMISM_PORTAL_PROXY_ADDRESS
export OKB_TOKEN_ADDRESS=$OKB_TOKEN_ADDRESS
export OP_PROXY_ADMIN=$OP_PROXY_ADMIN
export TRANSACTOR=$TRANSACTOR

forge script scripts/UpgradeSystemConfigToV4.s.sol:UpgradeSystemConfigToV4 \
  --rpc-url $L1_RPC_URL \
  --private-key $DEPLOYER_PRIVATE_KEY \
  --broadcast
