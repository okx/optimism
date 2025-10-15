#!/bin/bash
# =============================================================================
# Step02: Fund OP Stack role accounts
# Function: Fund batcher/proposer/challenger from L1 rich account with ETH
# =============================================================================
set -e
set -x

# Change to test directory (parent of steps/)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$(dirname "$SCRIPT_DIR")"

source .env
source tools.sh

echo "=========================================="
echo "Step 02: Fund accounts"
echo "=========================================="

# Calculate addresses for all actors
OP_BATCHER_ADDR=$(cast wallet address $OP_BATCHER_PRIVATE_KEY)
OP_PROPOSER_ADDR=$(cast wallet address $OP_PROPOSER_PRIVATE_KEY)
OP_CHALLENGER_ADDR=$(cast wallet address $OP_CHALLENGER_PRIVATE_KEY)

echo "Account addresses:"
echo "   Batcher:    $OP_BATCHER_ADDR"
echo "   Proposer:   $OP_PROPOSER_ADDR"
echo "   Challenger: $OP_CHALLENGER_ADDR"
echo ""

# Fund all actor addresses
echo "Funding accounts..."
for addr in $OP_BATCHER_ADDR $OP_PROPOSER_ADDR $OP_CHALLENGER_ADDR; do
    echo "   Funding 100 ETH to $addr..."
    cast send \
        --private-key $RICH_PRIVATE_KEY \
        --value 100ether \
        --legacy \
        --rpc-url $L1_RPC_URL \
        $addr

    # Validate balance
    BALANCE=$(cast balance $addr --rpc-url $L1_RPC_URL)
    echo "   SUCCESS: current balance: $(cast --to-unit $BALANCE ether) ETH"
done

echo ""
echo "SUCCESS: Step 02 completed: all accounts funded"

