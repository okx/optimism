#!/bin/bash
set -e

echo "🧪 Starting CGT Cross-Chain Tests Part 2 (After Challenge Period)..."

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
L1_STANDARD_BRIDGE="0x0013c64b9aec2f228c772d2449f64c070264854f"  # L1StandardBridge

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

# Step 3: L1 -> L2 cross-chain 2 WOKB
step3_l1_to_l2() {
    echo -e "\n${BLUE}🔄 Step 3: L1 → L2 cross-chain 2 WOKB${NC}"

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
        "approve(address,uint256)" "$L1_STANDARD_BRIDGE" "$amount" \
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
    local deposit_tx=$(cast send "$L1_STANDARD_BRIDGE" \
        "depositERC20(address,address,uint256,uint32,bytes)" \
        "$L1_WOKB_ADDRESS" "$L2_WETH_ADDRESS" "$amount" "200000" "0x" \
        --rpc-url "$L1_RPC_URL" \
        --private-key "$TEST_PRIVATE_KEY" \
        --json | jq -r '.transactionHash')

    if [ "$deposit_tx" != "null" ] && [ -n "$deposit_tx" ]; then
        echo "✅ L1 → L2 deposit initiated, tx: $deposit_tx"
        sleep 5
        get_balances "AFTER_STEP3"
        echo -e "${YELLOW}⏳ Note: L2 WOKB will increase after deposit relay (few minutes)${NC}"
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
    echo -e "${GREEN}🚀 Starting CGT Cross-Chain Tests Part 2${NC}"
    echo "=================================================="
    echo "Test Account: $TEST_ADDRESS"
    echo "L1 WOKB Contract: $L1_WOKB_ADDRESS"
    echo "L2 WOKB Contract: $L2_WETH_ADDRESS"
    echo "=================================================="

    # Check if withdrawal from part 1 exists
    if [ -f "withdrawal_tx.txt" ]; then
        WITHDRAWAL_TX=$(cat withdrawal_tx.txt)
        echo "Previous L2→L1 withdrawal tx: $WITHDRAWAL_TX"
    else
        echo "⚠️  No withdrawal_tx.txt found. Please run test_cross_chain_1_cgt.sh first."
    fi

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
    echo -e "\n${GREEN}🎉 Part 2 Tests Completed!${NC}"
    echo -e "\n${YELLOW}📋 Part 2 Summary:${NC}"
    echo "3. ✅ Cross-chain 2 WOKB from L1 → L2 (if L1 balance available)"
    echo "4. ✅ Converted all remaining WOKB → OKB on L2"

    get_balances "FINAL"

    echo -e "\n${YELLOW}📝 Complete Test Flow:${NC}"
    echo "• Part 1: OKB→WOKB conversion + L2→L1 withdrawal initiation"
    echo "• Part 2: L1→L2 deposit (after challenge period) + WOKB→OKB conversion"
    echo "• Total test demonstrates full cross-chain cycle"
}

# Run Main function
main "$@"
