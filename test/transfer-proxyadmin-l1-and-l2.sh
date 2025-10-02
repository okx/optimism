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

if [ -z "$L1_CROSS_DOMAIN_MESSENGER" ]; then
    echo "❌ L1_CROSS_DOMAIN_MESSENGER not set in .env"
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
echo "  L1_CROSS_DOMAIN_MESSENGER:      $L1_CROSS_DOMAIN_MESSENGER"
echo "  L1_RPC_URL:                     $L1_RPC_URL"
echo "  L2_RPC_URL:                     $L2_RPC_URL"
echo ""

# Navigate to contracts-bedrock directory
cd ../packages/contracts-bedrock

echo "Step 1: Running Foundry script to transfer both L1 and L2 ProxyAdmin ownership..."
echo ""

# Run the Foundry script
PROXY_ADMIN=$PROXY_ADMIN \
TRANSACTOR=$TRANSACTOR \
L1_CROSS_DOMAIN_MESSENGER=$L1_CROSS_DOMAIN_MESSENGER \
forge script scripts/deploy/TransferProxyAdminL1AndL2.s.sol:TransferProxyAdminL1AndL2 \
  --rpc-url $L1_RPC_URL \
  --private-key $DEPLOYER_PRIVATE_KEY \
  # --broadcast

echo ""
echo "=========================================="
echo "Script Execution Complete!"
echo "=========================================="
echo ""
echo "The script has:"
echo "  ✅ Deployed FoundationUpgradeSafe"
echo "  ✅ Deployed SecurityCouncilSafe with Liveness protection"
echo "  ✅ Deployed ProxyAdminOwnerSafe (1/3 multisig)"
echo "  ✅ Transferred L1 ProxyAdmin ownership"
echo "  ✅ Sent cross-domain message to transfer L2 ProxyAdmin ownership"
echo ""
echo "=========================================="
echo "⏳ Waiting for L2 Cross-Domain Message"
echo "=========================================="
echo ""
echo "The L2 ownership transfer requires:"
echo "  1. L1 transaction to be mined"
echo "  2. op-node to pick up the deposit event"
echo "  3. Message to be relayed to L2 (~1-2 minutes)"
echo ""
