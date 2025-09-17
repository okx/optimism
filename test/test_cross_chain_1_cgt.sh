#!/bin/bash
set -e

echo "🧪 Starting CGT Cross-Chain Tests Part 1 (Before Challenge Period)..."

# Color definitions
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Load environment variables
if [ -f ".env" ]; then
    source .env
    echo "✅ Loaded environment variables from .env"
else
    echo "❌ .env file not found"
    exit 1
fi

# Convert Docker internal RPC URL to local address
if [ -n "$L2_RPC_URL_IN_DOCKER" ]; then
    L2_RPC_URL=$(echo "$L2_RPC_URL_IN_DOCKER" | sed "s|op-geth-seq:8545|localhost:8123|g")
    export L2_RPC_URL
    echo "✅ Converted L2 RPC URL: $L2_RPC_URL"
fi

# Ensure required environment variables are set
if [ -z "$L1_RPC_URL" ] || [ -z "$L2_RPC_URL" ]; then
    echo "❌ Missing required environment variables (L1_RPC_URL, L2_RPC_URL)"
    exit 1
fi

# Contract addresses
L1_WOKB_ADDRESS="0x5ceef1981dc38767d8b9bdc82e4dfd43ad103c87"  # Our deployed OptimismMintableERC20
L2_WETH_ADDRESS="0x4200000000000000000000000000000000000006"  # L2 WETH (acts as WOKB)
L2_STANDARD_BRIDGE="0x4200000000000000000000000000000000000010"  # L2StandardBridge

# Test user (using pre-funded test account)
TEST_PRIVATE_KEY="0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
TEST_ADDRESS="0x70997970C51812dc3A010C7d01b50e0d17dc79C8"

echo "Test user: $TEST_ADDRESS"

# Utility functions
wei_to_ether() {
    cast --to-unit "$1" ether
}

get_balances() {
    local prefix="$1"

    # L1 WOKB balance
    local l1_wokb_wei=$(cast call "$L1_WOKB_ADDRESS" "balanceOf(address)" "$TEST_ADDRESS" --rpc-url "$L1_RPC_URL")
    local l1_wokb_ether=$(wei_to_ether "$l1_wokb_wei")

    # L2 WOKB balance
    local l2_wokb_wei=$(cast call "$L2_WETH_ADDRESS" "balanceOf(address)" "$TEST_ADDRESS" --rpc-url "$L2_RPC_URL")
    local l2_wokb_ether=$(wei_to_ether "$l2_wokb_wei")

    # L2 native balance (OKB)
    local l2_okb_wei=$(cast balance "$TEST_ADDRESS" --rpc-url "$L2_RPC_URL")
    local l2_okb_ether=$(wei_to_ether "$l2_okb_wei")

    echo -e "\n${BLUE}💰 $prefix Balances:${NC}"
    echo "L1 WOKB: $l1_wokb_ether WOKB"
    echo "L2 WOKB: $l2_wokb_ether WOKB"
    echo "L2 OKB:  $l2_okb_ether OKB"
}

show_balance_change() {
    local before_prefix="$1"
    local after_prefix="$2"
    local operation="$3"

    echo -e "\n${YELLOW}📊 Balance Changes ($operation):${NC}"
    echo "✅ Operation completed - see balance comparison above"
}

# Step 1: Convert 10 OKB to WOKB on L2
step1_okb_to_wokb() {
    echo -e "\n${BLUE}🔄 Step 1: Converting 10 OKB to WOKB on L2${NC}"

    get_balances "BEFORE_STEP1"

    # Deposit 10 OKB to WETH contract
    local deposit_amount="10000000000000000000"  # 10 OKB

    echo "Depositing 10 OKB to L2 WETH contract..."
    local tx_hash=$(cast send "$L2_WETH_ADDRESS" "deposit()" \
        --value "$deposit_amount" \
        --rpc-url "$L2_RPC_URL" \
        --private-key "$TEST_PRIVATE_KEY" \
        --json | jq -r '.transactionHash')

    if [ "$tx_hash" != "null" ] && [ -n "$tx_hash" ]; then
        echo "✅ OKB to WOKB conversion successful, tx: $tx_hash"
        sleep 3
        get_balances "AFTER_STEP1"
        return 0
    else
        echo "❌ OKB to WOKB conversion failed"
        return 1
    fi
}

# Step 2: L2 -> L1 cross-chain 5 WOKB
step2_l2_to_l1() {
    echo -e "\n${BLUE}🔄 Step 2: L2 → L1 cross-chain 5 WOKB${NC}"

    get_balances "BEFORE_STEP2"

    local amount="5000000000000000000"  # 5 WOKB

    # Approve L2 bridge
    echo "Approving L2 bridge to spend 5 WOKB..."
    local approve_tx=$(cast send "$L2_WETH_ADDRESS" \
        "approve(address,uint256)" "$L2_STANDARD_BRIDGE" "$amount" \
        --rpc-url "$L2_RPC_URL" \
        --private-key "$TEST_PRIVATE_KEY" \
        --json | jq -r '.transactionHash')

    if [ "$approve_tx" = "null" ]; then
        echo "❌ Failed to approve L2 bridge"
        return 1
    fi
    echo "✅ L2 bridge approved, tx: $approve_tx"
    sleep 3

    # Initiate withdrawal
    echo "Initiating L2 → L1 withdrawal..."
    local withdraw_tx=$(cast send "$L2_STANDARD_BRIDGE" \
        "bridgeERC20(address,address,uint256,uint32,bytes)" \
        "$L2_WETH_ADDRESS" "$L1_WOKB_ADDRESS" "$amount" "200000" "0x" \
        --rpc-url "$L2_RPC_URL" \
        --private-key "$TEST_PRIVATE_KEY" \
        --json | jq -r '.transactionHash')

    if [ "$withdraw_tx" != "null" ] && [ -n "$withdraw_tx" ]; then
        echo "✅ L2 → L1 withdrawal initiated, tx: $withdraw_tx"
        sleep 5
        get_balances "AFTER_STEP2"

        # Save withdrawal info for part 2
        echo "$withdraw_tx" > withdrawal_tx.txt
        echo "✅ Withdrawal transaction saved to withdrawal_tx.txt"

        return 0
    else
        echo "❌ L2 → L1 withdrawal failed"
        return 1
    fi
}

# Main function
main() {
    echo -e "${GREEN}🚀 Starting CGT Cross-Chain Tests Part 1${NC}"
    echo "=================================================="
    echo "Test Account: $TEST_ADDRESS"
    echo "L1 WOKB Contract: $L1_WOKB_ADDRESS"
    echo "L2 WOKB Contract: $L2_WETH_ADDRESS"
    echo "=================================================="

    # Get initial balances
    get_balances "INITIAL"

    # Execute steps 1 and 2
    echo -e "\n${YELLOW}🔄 Executing Part 1 Steps:${NC}"

    if step1_okb_to_wokb; then
        echo -e "${GREEN}✅ Step 1 completed successfully${NC}"
    else
        echo -e "${RED}❌ Step 1 failed${NC}"
        exit 1
    fi

    if step2_l2_to_l1; then
        echo -e "${GREEN}✅ Step 2 completed successfully${NC}"
    else
        echo -e "${RED}❌ Step 2 failed${NC}"
        exit 1
    fi

    # Final summary for part 1
    echo -e "\n${GREEN}🎉 Part 1 Tests Completed Successfully!${NC}"
    echo -e "\n${YELLOW}📋 Part 1 Summary:${NC}"
    echo "1. ✅ Converted 10 OKB → WOKB on L2"
    echo "2. ✅ Initiated L2 → L1 withdrawal (5 WOKB)"

    get_balances "FINAL_PART1"

    echo -e "\n${YELLOW}⏳ Challenge Period Information:${NC}"
    echo "• Challenge period: $MAX_CLOCK_DURATION seconds ($(($MAX_CLOCK_DURATION / 60)) minutes)"
    echo "• L1 WOKB will be available after challenge period"
    echo "• Current time: $(date)"
    echo "• Estimated completion: $(date -d "+$MAX_CLOCK_DURATION seconds")"

    echo -e "\n${YELLOW}🚀 Next Steps:${NC}"
    echo "• Wait for challenge period to complete"
    echo "• Then run: ./test_cross_chain_2_cgt.sh"
    echo "• Or wait and run manually after $(($MAX_CLOCK_DURATION / 60)) minutes"
}

# Run Main function
main "$@"
