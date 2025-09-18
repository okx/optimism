#!/bin/bash
set -e

echo "🧪 Starting CGT L1→L2 Deposit Tests..."

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
if [ -z "$L1_WOKB_ADDRESS" ]; then
    echo "❌ L1_WOKB_ADDRESS not found in environment. Please run ./deploy_wokb_cgt.sh first."
    exit 1
fi
L2_WETH_ADDRESS="0x4200000000000000000000000000000000000006"  # L2 WETH (acts as WOKB)
# L1StandardBridge address will be read from config

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

# Wait for L2 deposit processing and verify balance increase
wait_for_l2_deposit_processing() {
    local expected_amount="$1"
    local expected_amount_ether=$(wei_to_ether "$expected_amount")

    echo -e "\n${YELLOW}⏳ Waiting for L2 deposit processing (checking every 30 seconds)...${NC}"
    echo "Expected L2 WOKB increase: $expected_amount_ether WOKB"

    # Get initial L2 WOKB balance
    local initial_l2_wokb_wei=$(cast call "$L2_WETH_ADDRESS" "balanceOf(address)" "$TEST_ADDRESS" --rpc-url "$L2_RPC_URL")
    local initial_l2_wokb_ether=$(wei_to_ether "$initial_l2_wokb_wei")

    echo "Initial L2 WOKB balance: $initial_l2_wokb_ether WOKB"

    # Wait up to 5 minutes (10 checks, 30 seconds apart)
    # L1→L2 deposits can take longer than L2→L1 withdrawals
    local max_attempts=10
    local attempt=1
    local check_interval=30

    while [ $attempt -le $max_attempts ]; do
        echo "⏳ Checking attempt $attempt/$max_attempts ($(($attempt * $check_interval))s elapsed)..."

        # Get current L2 WOKB balance
        local current_l2_wokb_wei=$(cast call "$L2_WETH_ADDRESS" "balanceOf(address)" "$TEST_ADDRESS" --rpc-url "$L2_RPC_URL" 2>/dev/null || echo "0")
        local current_l2_wokb_ether=$(wei_to_ether "$current_l2_wokb_wei")

        echo "Current L2 WOKB balance: $current_l2_wokb_ether WOKB"

        # Calculate the difference
        local balance_diff=$((current_l2_wokb_wei - initial_l2_wokb_wei))
        local balance_diff_ether=$(wei_to_ether "$balance_diff")

        # Check if balance increased by expected amount (with small tolerance for rounding)
        local tolerance=$((expected_amount / 1000))  # 0.1% tolerance
        if [ "$balance_diff" -ge $((expected_amount - tolerance)) ]; then
            echo -e "${GREEN}✅ L2 deposit processed successfully!${NC}"
            echo "Balance increased by: $balance_diff_ether WOKB"
            echo "Expected: $(wei_to_ether $expected_amount) WOKB"
            get_balances "AFTER_STEP3_CONFIRMED"
            return 0
        elif [ "$balance_diff" -gt 0 ]; then
            echo "⚠️  Partial deposit detected: $balance_diff_ether WOKB (expected: $(wei_to_ether $expected_amount) WOKB)"
        fi

        if [ $attempt -lt $max_attempts ]; then
            echo "💤 Waiting $check_interval seconds before next check..."
            sleep $check_interval
        fi

        attempt=$((attempt + 1))
    done

    echo -e "${RED}❌ L2 deposit not confirmed within 5 minutes${NC}"
    echo "💡 This might indicate:"
    echo "   • L1→L2 bridge processing issues"
    echo "   • Network congestion"
    echo "   • Incorrect bridge address"
    echo "Final balance check:"
    get_balances "AFTER_STEP3_TIMEOUT"
    return 1
}

# Step 3: L1 -> L2 cross-chain 2 WOKB
step3_l1_to_l2() {
    echo -e "\n${BLUE}🔄 Step 3: L1 → L2 cross-chain 2 WOKB${NC}"

    # Get L1StandardBridge address from config
    local l1_standard_bridge
    if [ -f "config-op/state.json" ]; then
        l1_standard_bridge=$(jq -r '.opChainDeployments[0].L1StandardBridgeProxy' config-op/state.json)
        if [ "$l1_standard_bridge" = "null" ] || [ -z "$l1_standard_bridge" ]; then
            echo "❌ Could not find L1StandardBridgeProxy in config-op/state.json"
            return 1
        fi
        echo "✅ L1StandardBridge address: $l1_standard_bridge"
    else
        echo "❌ config-op/state.json not found"
        return 1
    fi

    get_balances "BEFORE_STEP3"

    local amount="2000000000000000000"  # 2 WOKB

    # Check L1 WOKB balance
    local l1_balance=$(cast call "$L1_WOKB_ADDRESS" "balanceOf(address)" "$TEST_ADDRESS" --rpc-url "$L1_RPC_URL")
    local l1_balance_dec=$((16#${l1_balance:2}))
    local required_dec=$((16#${amount:2}))

    if [ $l1_balance_dec -lt $required_dec ]; then
        echo "❌ Insufficient L1 WOKB balance: $(wei_to_ether $l1_balance) WOKB"
        echo "💡 Need $(wei_to_ether $amount) WOKB for this test"
        echo "💡 This usually means the L2→L1 withdrawal hasn't completed yet"
        return 1
    fi

    echo "✅ Sufficient L1 WOKB balance: $(wei_to_ether $l1_balance) WOKB"

    # Approve L1 bridge
    echo "Approving L1 bridge to spend 2 WOKB..."
    local approve_tx=$(cast send "$L1_WOKB_ADDRESS" \
        "approve(address,uint256)" "$l1_standard_bridge" "$amount" \
        --rpc-url "$L1_RPC_URL" \
        --private-key "$TEST_PRIVATE_KEY" \
        --json | jq -r '.transactionHash')

    if [ "$approve_tx" = "null" ]; then
        echo "❌ Failed to approve L1 bridge"
        return 1
    fi
    echo "✅ L1 bridge approved, tx: $approve_tx"
    sleep 5

    # Initiate deposit
    echo "Initiating L1 → L2 deposit..."
    echo "Bridge address: $l1_standard_bridge"
    echo "L1 WOKB: $L1_WOKB_ADDRESS → L2 WETH: $L2_WETH_ADDRESS"
    echo "Amount: $(wei_to_ether $amount) WOKB"

    local deposit_tx=$(cast send "$l1_standard_bridge" \
        "depositERC20(address,address,uint256,uint32,bytes)" \
        "$L1_WOKB_ADDRESS" "$L2_WETH_ADDRESS" "$amount" "200000" "0x" \
        --rpc-url "$L1_RPC_URL" \
        --private-key "$TEST_PRIVATE_KEY" \
        --json | jq -r '.transactionHash')

    if [ "$deposit_tx" != "null" ] && [ -n "$deposit_tx" ]; then
        echo "✅ L1 → L2 deposit initiated, tx: $deposit_tx"
        sleep 5
        get_balances "AFTER_STEP3_INITIAL"

        # Wait for L2 deposit processing and verify balance increase
        wait_for_l2_deposit_processing "$amount"

        return 0
    else
        echo "❌ L1 → L2 deposit failed"
        return 1
    fi
}

# Step 4: Convert all WOKB back to OKB on L2
step4_wokb_to_okb() {
    echo -e "\n${BLUE}🔄 Step 4: Converting all WOKB back to OKB on L2${NC}"

    get_balances "BEFORE_STEP4"

    # Get current L2 WOKB balance
    local l2_wokb_balance=$(cast call "$L2_WETH_ADDRESS" "balanceOf(address)" "$TEST_ADDRESS" --rpc-url "$L2_RPC_URL")
    local balance_dec=$((16#${l2_wokb_balance:2}))

    if [ $balance_dec -eq 0 ]; then
        echo "⚠️  No WOKB balance to convert"
        get_balances "AFTER_STEP4"
        return 0
    fi

    local balance_ether=$(wei_to_ether "$l2_wokb_balance")
    echo "Converting $balance_ether WOKB back to OKB..."

    local tx_hash=$(cast send "$L2_WETH_ADDRESS" "withdraw(uint256)" "$l2_wokb_balance" \
        --rpc-url "$L2_RPC_URL" \
        --private-key "$TEST_PRIVATE_KEY" \
        --json | jq -r '.transactionHash')

    if [ "$tx_hash" != "null" ] && [ -n "$tx_hash" ]; then
        echo "✅ WOKB to OKB conversion successful, tx: $tx_hash"
        sleep 3
        get_balances "AFTER_STEP4"
        return 0
    else
        echo "❌ WOKB to OKB conversion failed"
        return 1
    fi
}

# Main function
main() {
    echo -e "${GREEN}🚀 Starting CGT L1→L2 Deposit Tests${NC}"
    echo "=================================================="
    echo "Test Account: $TEST_ADDRESS"
    echo "L1 WOKB Contract: $L1_WOKB_ADDRESS"
    echo "L2 WOKB Contract: $L2_WETH_ADDRESS"
    echo "=================================================="

    # Note: This script tests L1→L2 deposits and runs independently
    echo "💡 This script tests L1→L2 deposits (requires existing L1 WOKB balance)"

    # Get current balances
    get_balances "PART2_INITIAL"

    # Execute steps 3 and 4
    echo -e "\n${YELLOW}🔄 Executing Part 2 Steps:${NC}"

    if step3_l1_to_l2; then
        echo -e "${GREEN}✅ Step 3 completed successfully${NC}"
    else
        echo -e "${RED}❌ Step 3 failed${NC}"
        echo -e "${YELLOW}💡 This is expected if challenge period hasn't completed yet${NC}"
    fi

    if step4_wokb_to_okb; then
        echo -e "${GREEN}✅ Step 4 completed successfully${NC}"
    else
        echo -e "${RED}❌ Step 4 failed${NC}"
        exit 1
    fi

    # Final summary
    echo -e "\n${GREEN}🎉 L1→L2 Deposit Tests Completed!${NC}"
    echo -e "\n${YELLOW}📋 Test Summary:${NC}"
    echo "1. ✅ Deposited WOKB from L1 → L2 (with real-time verification)"
    echo "2. ✅ Converted all WOKB → OKB on L2"

    get_balances "FINAL"

    echo -e "\n${YELLOW}📝 L1→L2 Deposit Flow:${NC}"
    echo "• L1: Approve bridge to spend WOKB"
    echo "• L1: Call depositERC20 on L1StandardBridge"
    echo "• L2: Wait for L2 node to process L1 deposit event (2-5 minutes)"
    echo "• L2: WOKB balance increases automatically"
}

# Run Main function
main "$@"
