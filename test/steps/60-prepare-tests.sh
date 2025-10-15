#!/bin/bash
# =============================================================================
# Step60: Prepare tests
# Function: Fund test accounts, test L1 to L2 bridging
# =============================================================================
set -e

# Change to test directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$(dirname "$SCRIPT_DIR")"
source .env

echo "=========================================="
echo "Step60: Prepare tests"
echo "=========================================="

PWD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PWD_DIR=$(dirname "$PWD_DIR")
cd $PWD_DIR

# Test account
L1_ADMIN_ADDRESS="0x8f8E2d6cF621f30e9a11309D6A56A876281Fd534"
L1_ADMIN_PRIVATE_KEY="0x815405dddb0e2a99b12af775fd2929e526704e1d1aea6a0b4e74dc33e2f7fcd2"

# Rich address (default test address)
RECIPIENT="0x14dC79964da2C08b23698B3D3cc7Ca32193d9955"
PRIVATE_KEY="0x4bbbf85ce3377467afe5d46f804f221813b2bb87f24d81f60f1fcdbf7cbf4356"

echo ""
echo "=========================================="
echo "1. Fund Test account on L1"
echo "=========================================="

echo "Funding test admin from L1 rich account..."
cast send $L1_ADMIN_ADDRESS \
    --rpc-url $L1_RPC_URL \
    --private-key $RICH_PRIVATE_KEY \
    --value 1000ether

# Verify balance
ADMIN_BALANCE_L1=$(cast balance $L1_ADMIN_ADDRESS --rpc-url $L1_RPC_URL)
echo "SUCCESS: L1 Admin balance: $(cast --to-unit $ADMIN_BALANCE_L1 ether) ETH"

echo ""
echo "=========================================="
echo "2. Test L1 to L2 bridging"
echo "=========================================="

# Get OptimismPortal address
OPTIMISM_PORTAL=$(cast call --rpc-url $L1_RPC_URL $SYSTEM_CONFIG_PROXY_ADDRESS 'optimismPortal()(address)')

echo "Bridge info:"
echo "   OptimismPortal: $OPTIMISM_PORTAL"
echo "   Recipient:      $RECIPIENT"

# Check L2 initial balance
echo ""
echo "Checking L2 initial balance..."
BALANCE_L2_BEFORE=$(cast balance $RECIPIENT --rpc-url $L2_RPC_URL 2>/dev/null || echo "0")
ADMIN_BALANCE_L2_BEFORE=$(cast balance $L1_ADMIN_ADDRESS --rpc-url $L2_RPC_URL 2>/dev/null || echo "0")

echo "   Rich address L2 balance: $(cast --to-unit $BALANCE_L2_BEFORE ether) ETH"
echo "   Admin L2 balance:        $(cast --to-unit $ADMIN_BALANCE_L2_BEFORE ether) ETH"

# Bridge 100 ETH from Rich address
echo ""
echo "Bridging 100 ETH (Rich address)..."
cast send $OPTIMISM_PORTAL \
    --rpc-url $L1_RPC_URL \
    --private-key $PRIVATE_KEY \
    --value 100ether

# Bridge 100 ETH from Admin address
echo "Bridging 100 ETH (Admin address)..."
cast send $OPTIMISM_PORTAL \
    --rpc-url $L1_RPC_URL \
    --private-key $L1_ADMIN_PRIVATE_KEY \
    --value 100ether

echo ""
echo "Waiting for bridge transactions to complete..."

# Wait for L2 balance to update
MAX_WAIT=300  # 5 minutes
WAIT_COUNT=0

while [ $WAIT_COUNT -lt $MAX_WAIT ]; do
    BALANCE=$(cast balance $RECIPIENT --rpc-url $L2_RPC_URL 2>/dev/null || echo "0")
    ADMIN_BALANCE=$(cast balance $L1_ADMIN_ADDRESS --rpc-url $L2_RPC_URL 2>/dev/null || echo "0")

    if [ "$BALANCE" != "0" ] && [ "$BALANCE" != "$BALANCE_L2_BEFORE" ] && \
       [ "$ADMIN_BALANCE" != "0" ] && [ "$ADMIN_BALANCE" != "$ADMIN_BALANCE_L2_BEFORE" ]; then
        echo "SUCCESS: Bridge transactions completed!"
        break
    fi

    echo "   Waiting for bridge... (${WAIT_COUNT}/${MAX_WAIT} seconds)"
    echo "   Rich L2 balance: $(cast --to-unit $BALANCE ether) ETH"
    echo "   Admin L2 balance: $(cast --to-unit $ADMIN_BALANCE ether) ETH"

    sleep 5
    WAIT_COUNT=$((WAIT_COUNT + 5))
done

if [ $WAIT_COUNT -ge $MAX_WAIT ]; then
    echo "WARNING: Bridge timeout, but may still be processing"
else
    BALANCE_L2_AFTER=$(cast balance $RECIPIENT --rpc-url $L2_RPC_URL)
    ADMIN_BALANCE_L2_AFTER=$(cast balance $L1_ADMIN_ADDRESS --rpc-url $L2_RPC_URL)

    echo ""
    echo "Final L2 balances:"
    echo "   Rich address: $(cast --to-unit $BALANCE_L2_AFTER ether) ETH"
    echo "   Admin:        $(cast --to-unit $ADMIN_BALANCE_L2_AFTER ether) ETH"
fi

echo ""
echo "=========================================="
echo "3. Validating L2 network"
echo "=========================================="

# Get L2 block height
L2_BLOCK=$(cast block-number --rpc-url $L2_RPC_URL 2>/dev/null || echo "0")
echo "L2 latest block: $L2_BLOCK"

# Get L2 Chain ID
L2_CHAIN_ID=$(cast chain-id --rpc-url $L2_RPC_URL 2>/dev/null || echo "unknown")
echo "L2 Chain ID: $L2_CHAIN_ID"

# Validate Chain ID
if [ "$L2_CHAIN_ID" = "$CHAIN_ID" ]; then
    echo "SUCCESS: Chain ID matches"
else
    echo "WARNING: Chain ID mismatch: expected $CHAIN_ID, got $L2_CHAIN_ID"
fi

echo ""
echo "SUCCESS: Step 60 completed: Test environment prepared"
echo ""
echo "Test account info:"
echo "   Rich address:  $RECIPIENT"
echo "   Admin address: $L1_ADMIN_ADDRESS"
echo ""
echo "Next steps:"
echo "   - Test L2 transaction: cast send <address> --value 0.1ether --rpc-url \$L2_RPC_URL --private-key \$PRIVATE_KEY"
echo "   - Check L2 block: cast block latest --rpc-url \$L2_RPC_URL"
echo "   - Check service log: docker logs op-geth-seq"

