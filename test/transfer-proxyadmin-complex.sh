#!/bin/bash
# This demonstrates how to deploy the security council governance structure
# and transfer ProxyAdmin ownership to a 2/2 multisig

set -e

# Load environment variables
source .env

# # Set DEPLOY_CONFIG_PATH if not already set
# # This is required by the base Deployer contract even though we don't use most values
# if [ -z "$DEPLOY_CONFIG_PATH" ]; then
#   # Use the internal-devnet config as default (you can change this to your network's config)
#   export DEPLOY_CONFIG_PATH="deploy-config/hardhat.json"
#   echo "DEPLOY_CONFIG_PATH not set, using default: $DEPLOY_CONFIG_PATH"
# fi

echo "========================================="
echo "ProxyAdmin Ownership Transfer Example"
echo "========================================="
echo ""

# Verify required environment variables are set
if [ -z "$PROXY_ADMIN" ]; then
  echo "Error: PROXY_ADMIN environment variable is not set"
  exit 1
fi

if [ -z "$TRANSACTOR" ]; then
  echo "Error: TRANSACTOR environment variable is not set"
  exit 1
fi

echo "Configuration:"
echo "  L1 RPC URL:          $L1_RPC_URL"
echo "  ProxyAdmin:          $PROXY_ADMIN"
echo "  Transactor:          $TRANSACTOR"
echo "  Deploy Config Path:  $DEPLOY_CONFIG_PATH"
echo ""

# Verify current ProxyAdmin owner
echo "Current ProxyAdmin owner:"
CURRENT_OWNER=$(cast call $PROXY_ADMIN "owner()" --rpc-url $L1_RPC_URL)
echo "  $CURRENT_OWNER"

# Verify Transactor owner
echo "Current Transactor owner:"
TRANSACTOR_OWNER=$(cast call $TRANSACTOR "owner()" --rpc-url $L1_RPC_URL)
echo "  $TRANSACTOR_OWNER"
echo ""

# Navigate to contracts directory
cd ../packages/contracts-bedrock

echo "Step 1: Deploying governance structure..."
echo "This will:"
echo "  1. Deploy FoundationUpgradeSafe (5/7 multisig)"
echo "  2. Deploy SecurityCouncilSafe (10/13 multisig)"
echo "  3. Configure LivenessModule and LivenessGuard"
echo "  4. Deploy ProxyAdminOwnerSafe (2/2 multisig with above safes as owners)"
echo "  5. Transfer ProxyAdmin ownership via Transactor to ProxyAdminOwnerSafe"
echo ""

# Run the deployment script
# Note: Explicitly pass environment variables that the script needs
PROXY_ADMIN=$PROXY_ADMIN \
TRANSACTOR=$TRANSACTOR \
forge script scripts/deploy/TransferProxyAdmin.s.sol:TransferProxyAdmin \
  --rpc-url $L1_RPC_URL \
  --private-key $DEPLOYER_PRIVATE_KEY \
  --broadcast

echo ""
echo "========================================="
echo "Deployment Complete!"
echo "========================================="
echo ""

# Verify the ProxyAdmin ownership
echo "Verifying ProxyAdmin ownership..."
NEW_OWNER=$(cast call $PROXY_ADMIN "owner()" --rpc-url $L1_RPC_URL)

echo "ProxyAdmin owner:   $NEW_OWNER"
