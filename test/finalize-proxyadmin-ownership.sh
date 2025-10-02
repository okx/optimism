#!/bin/bash

set -e

source .env

echo "=========================================="
echo "Finalizing ProxyAdminOwnerSafe (2/2)"
echo "=========================================="
echo ""

# Verify required environment variables
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

# L2 ProxyAdmin predeploy address
L2_PROXY_ADMIN="0x4200000000000000000000000000000000000018"

echo "Step 1: Verifying L2 ProxyAdmin ownership transfer..."
echo ""

L2_OWNER=$(cast call $L2_PROXY_ADMIN "owner()" --rpc-url $L2_RPC_URL)
echo "Current L2 ProxyAdmin owner: $L2_OWNER"
echo ""

# Check if it's been transferred (not zero address)
if [ "$L2_OWNER" == "0x0000000000000000000000000000000000000000" ]; then
    echo "❌ L2 ProxyAdmin owner is still zero address"
    echo "   The cross-domain message may not have been relayed yet."
    echo "   Please wait and try again."
    exit 1
fi

echo "✅ L2 ProxyAdmin ownership has been transferred"
echo ""

read -p "Are you ready to finalize ProxyAdminOwnerSafe? This will remove the deployer and set threshold to 2/2. (y/N) " -n 1 -r
echo
if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    echo "Aborted by user"
    exit 0
fi

echo ""
echo "Step 2: Finalizing ProxyAdminOwnerSafe..."
echo ""

# Navigate to contracts-bedrock directory
cd packages/contracts-bedrock

# Run the finalization function
forge script scripts/deploy/TransferProxyAdminL1AndL2.s.sol:TransferProxyAdminL1AndL2 \
    --sig "finalizeProxyAdminOwnerSafe()" \
    --rpc-url "$L1_RPC_URL" \
    --private-key "$DEPLOYER_PRIVATE_KEY" \
    --broadcast \
    -vvv

echo ""
echo "=========================================="
echo "Finalization Complete!"
echo "=========================================="
echo ""
echo "ProxyAdminOwnerSafe has been finalized:"
echo "  ✅ Deployer removed"
echo "  ✅ Threshold changed to 2/2"
echo "  ✅ Only SecurityCouncilSafe + FoundationUpgradeSafe can now act"
echo ""
echo "The governance structure is now fully operational!"
echo ""
