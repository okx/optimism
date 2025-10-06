#!/bin/bash

set -e

source .env

echo "=========================================="
echo "L1 and L2 ProxyAdmin Ownership Transfer"
echo "=========================================="
echo ""

# Verify required environment variables
if [ -z "$PROXY_ADMIN" ]; then
    echo "❌ PROXY_ADMIN not set in .env"
    exit 1
fi

if [ -z "$TRANSACTOR" ]; then
    echo "❌ TRANSACTOR not set in .env"
    exit 1
fi

if [ -z "$L1_RPC_URL" ]; then
    echo "❌ L1_RPC_URL not set in .env"
    exit 1
fi

if [ -z "$L2_RPC_URL" ]; then
    echo "❌ L2_RPC_URL not set in .env"
    exit 1
fi

if [ -z "$DEPLOYER_PRIVATE_KEY" ]; then
    echo "❌ DEPLOYER_PRIVATE_KEY not set in .env"
    exit 1
fi

echo "Environment Variables:"
echo "  PROXY_ADMIN:                    $PROXY_ADMIN"
echo "  TRANSACTOR:                     $TRANSACTOR"
echo "  L1_RPC_URL:                     $L1_RPC_URL"
echo "  L2_RPC_URL:                     $L2_RPC_URL"
echo ""

# Navigate to contracts-bedrock directory
cd ../packages/contracts-bedrock

echo "Step 1: Deploy Safe wallets and transfer L1 ProxyAdmin ownership"
echo ""

# Run the Foundry script
PROXY_ADMIN=$PROXY_ADMIN \
TRANSACTOR=$TRANSACTOR \
forge script scripts/deploy/TransferProxyAdminL1.s.sol \
  --rpc-url $L1_RPC_URL \
  --private-key $DEPLOYER_PRIVATE_KEY \
  --broadcast

echo "Step 2: Transfer L2 ProxyAdmin ownership"
echo ""

if [ -z "$PROXY_ADMIN_OWNER_SAFE" ]; then
    echo "❌ PROXY_ADMIN_OWNER_SAFE not set in .env"
    exit 1
fi

# Fund the deployer address on L2
DEPLOYER_ADDRESS=$(cast wallet address --private-key $DEPLOYER_PRIVATE_KEY)
cast send --private-key $RICH_L1_PRIVATE_KEY --value 1ether $DEPLOYER_ADDRESS --rpc-url $L2_RPC_URL

PROXY_ADMIN_OWNER_SAFE=$PROXY_ADMIN_OWNER_SAFE \
forge script scripts/deploy/TransferProxyAdminL2.s.sol \
  --rpc-url $L2_RPC_URL \
  --private-key $DEPLOYER_PRIVATE_KEY \
  --broadcast
