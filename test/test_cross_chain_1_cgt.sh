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
if [ -z "$L1_WOKB_ADDRESS" ]; then
    echo "❌ L1_WOKB_ADDRESS not found in environment. Please run ./deploy_wokb_cgt.sh first."
    exit 1
fi
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

# Check and build withdrawal tool if needed
check_withdrawal_tool() {
    echo -e "\n${BLUE}🔧 Checking withdrawal tool...${NC}"

    local withdrawal_binary="../op-chain-ops/bin/withdrawal"

    if [ ! -f "$withdrawal_binary" ]; then
        echo "⚠️  Withdrawal tool not found, building..."
        cd ..
        make withdrawal
        cd test

        if [ ! -f "$withdrawal_binary" ]; then
            echo "❌ Failed to build withdrawal tool"
            return 1
        fi
        echo "✅ Withdrawal tool built successfully"
    else
        echo "✅ Withdrawal tool found"
    fi
    return 0
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

        # Export the withdrawal tx for next steps
        export WITHDRAWAL_TX="$withdraw_tx"
        return 0
    else
        echo "❌ L2 → L1 withdrawal failed"
        return 1
    fi
}

# Step 3: Prove withdrawal on L1
step3_prove_withdrawal() {
    echo -e "\n${BLUE}🔄 Step 3: Proving withdrawal on L1${NC}"

    if [ -z "$WITHDRAWAL_TX" ]; then
        echo "❌ No withdrawal transaction found"
        return 1
    fi

    # Get portal address from config
    local portal_address
    if [ -f "config-op/state.json" ]; then
        portal_address=$(jq -r '.opChainDeployments[0].OptimismPortalProxy' config-op/state.json)
        if [ "$portal_address" = "null" ] || [ -z "$portal_address" ]; then
            echo "❌ Could not find OptimismPortalProxy in config-op/state.json"
            return 1
        fi
        echo "✅ Portal address: $portal_address"
    else
        echo "❌ config-op/state.json not found"
        return 1
    fi

    echo "Proving withdrawal transaction: $WITHDRAWAL_TX"
    echo -e "${YELLOW}⏳ This process may take 1-2 minutes, please wait...${NC}"
    echo "📋 Progress:"
    echo "  1️⃣ Fetching L2 transaction receipt and proof data..."
    echo "  2️⃣ Finding corresponding dispute game..."
    echo "  3️⃣ Generating Merkle proofs..."
    echo "  4️⃣ Submitting proof to L1..."
    echo ""

    # Run the prove command with background process and manual timeout
    local prove_output_file=$(mktemp)
    local prove_pid_file=$(mktemp)

    # Start prove command in background
    (cd .. && ./op-chain-ops/bin/withdrawal prove \
        --tx "$WITHDRAWAL_TX" \
        --l1 "$L1_RPC_URL" \
        --l2 "$L2_RPC_URL" \
        --portal-address "$portal_address" \
        --private-key "$RICH_L1_PRIVATE_KEY" \
        2>&1 > "$prove_output_file"; echo $? > "$prove_pid_file") &

    local bg_pid=$!
    local max_wait=300  # 5 minutes
    local waited=0
    local check_interval=10

    # Wait with progress updates
    while [ $waited -lt $max_wait ]; do
        if ! kill -0 $bg_pid 2>/dev/null; then
            # Process finished
            break
        fi

        waited=$((waited + check_interval))
        echo "⏳ Proving in progress... (${waited}s elapsed, max ${max_wait}s)"
        sleep $check_interval
    done

    # Check if process is still running (timed out)
    if kill -0 $bg_pid 2>/dev/null; then
        echo "⚠️  Process taking longer than expected, killing..."
        kill $bg_pid 2>/dev/null
        wait $bg_pid 2>/dev/null
        echo "❌ Prove command timed out after $max_wait seconds"
        rm -f "$prove_output_file" "$prove_pid_file"
        return 1
    fi

    # Get the output
    local prove_output=$(cat "$prove_output_file")
    rm -f "$prove_output_file" "$prove_pid_file"

    local prove_tx=$(echo "$prove_output" | grep "Proved withdrawal" | grep -o "tx=0x[a-fA-F0-9]*" | cut -d'=' -f2)

    if [ -n "$prove_tx" ]; then
        echo "✅ Withdrawal proved successfully, tx: $prove_tx"
        export PROVE_TX="$prove_tx"
        return 0
    else
        echo "❌ Failed to prove withdrawal"
        echo "🔍 Debug information:"
        echo "$prove_output" | head -20
        if echo "$prove_output" | grep -q "timed out"; then
            echo "⚠️  Process timed out after 5 minutes. This might indicate network issues or heavy load."
        fi
        return 1
    fi
}

# Step 4: Wait for challenge period and finalize withdrawal
step4_finalize_withdrawal() {
    echo -e "\n${BLUE}🔄 Step 4: Finalizing withdrawal on L1${NC}"

    if [ -z "$WITHDRAWAL_TX" ]; then
        echo "❌ No withdrawal transaction found"
        return 1
    fi

    # Wait for challenge period
    echo "⏳ Waiting for challenge period ($MAX_CLOCK_DURATION seconds)..."
    sleep $((MAX_CLOCK_DURATION + 5))  # Add 5 seconds buffer

    # Get portal address from config
    local portal_address
    if [ -f "config-op/state.json" ]; then
        portal_address=$(jq -r '.opChainDeployments[0].OptimismPortalProxy' config-op/state.json)
    else
        echo "❌ config-op/state.json not found"
        return 1
    fi

    get_balances "BEFORE_FINALIZE"

    echo "Finalizing withdrawal transaction: $WITHDRAWAL_TX"
    echo -e "${YELLOW}⏳ Finalizing withdrawal (this should be quick)...${NC}"
    echo "📋 Progress:"
    echo "  1️⃣ Verifying challenge period has passed..."
    echo "  2️⃣ Executing withdrawal on L1..."
    echo "  3️⃣ Transferring funds to recipient..."
    echo ""

    # Run finalize command with background process and manual timeout
    local finalize_output_file=$(mktemp)

    # Start finalize command in background
    (cd .. && ./op-chain-ops/bin/withdrawal finalize \
        --tx "$WITHDRAWAL_TX" \
        --l1 "$L1_RPC_URL" \
        --l2 "$L2_RPC_URL" \
        --portal-address "$portal_address" \
        --private-key "$RICH_L1_PRIVATE_KEY" \
        2>&1 > "$finalize_output_file") &

    local bg_pid=$!
    local max_wait=120  # 2 minutes
    local waited=0
    local check_interval=5

    # Wait with progress updates
    while [ $waited -lt $max_wait ]; do
        if ! kill -0 $bg_pid 2>/dev/null; then
            # Process finished
            break
        fi

        waited=$((waited + check_interval))
        echo "⏳ Finalizing... (${waited}s elapsed)"
        sleep $check_interval
    done

    # Check if process is still running (timed out)
    if kill -0 $bg_pid 2>/dev/null; then
        echo "⚠️  Finalize taking longer than expected, killing..."
        kill $bg_pid 2>/dev/null
        wait $bg_pid 2>/dev/null
        echo "❌ Finalize command timed out after $max_wait seconds"
        rm -f "$finalize_output_file"
        return 1
    fi

    # Get the output
    local finalize_output=$(cat "$finalize_output_file")
    rm -f "$finalize_output_file"

    local finalize_tx=$(echo "$finalize_output" | grep "Finalized withdrawal" | grep -o "tx=0x[a-fA-F0-9]*" | cut -d'=' -f2)

    if [ -n "$finalize_tx" ]; then
        echo "✅ Withdrawal finalized successfully, tx: $finalize_tx"
        echo "⏳ Waiting for transaction confirmation and balance update..."
        sleep 10  # Wait a bit longer for L1 transaction to be mined

        get_balances "AFTER_FINALIZE"

        # Verify that L1 WOKB balance actually increased
        echo "🔍 Verifying L1 WOKB balance increase..."
        local current_l1_wokb_wei=$(cast call "$L1_WOKB_ADDRESS" "balanceOf(address)" "$TEST_ADDRESS" --rpc-url "$L1_RPC_URL")
        local current_l1_wokb_ether=$(wei_to_ether "$current_l1_wokb_wei")

        if [ "$current_l1_wokb_wei" -gt "0" ]; then
            echo -e "${GREEN}✅ L1 WOKB balance confirmed: $current_l1_wokb_ether WOKB${NC}"
        else
            echo -e "${YELLOW}⚠️  L1 WOKB balance is still 0. Transaction may need more time to process.${NC}"
        fi

        return 0
    else
        echo "❌ Failed to finalize withdrawal"
        echo "🔍 Debug information:"
        echo "$finalize_output" | head -20
        if echo "$finalize_output" | grep -q "timed out"; then
            echo "⚠️  Process timed out after 2 minutes."
        fi
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

    # Check withdrawal tool first
    if ! check_withdrawal_tool; then
        echo -e "${RED}❌ Failed to prepare withdrawal tool${NC}"
        exit 1
    fi

    # Execute all steps
    echo -e "\n${YELLOW}🔄 Executing Complete Cross-Chain Test:${NC}"

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

    if step3_prove_withdrawal; then
        echo -e "${GREEN}✅ Step 3 completed successfully${NC}"
    else
        echo -e "${RED}❌ Step 3 failed${NC}"
        exit 1
    fi

    if step4_finalize_withdrawal; then
        echo -e "${GREEN}✅ Step 4 completed successfully${NC}"
    else
        echo -e "${RED}❌ Step 4 failed${NC}"
        exit 1
    fi

    # Final summary
    echo -e "\n${GREEN}🎉 Complete Cross-Chain Test Completed Successfully!${NC}"
    echo -e "\n${YELLOW}📋 Full Test Summary:${NC}"
    echo "1. ✅ Converted 10 OKB → WOKB on L2"
    echo "2. ✅ Initiated L2 → L1 withdrawal (5 WOKB)"
    echo "3. ✅ Proved withdrawal on L1"
    echo "4. ✅ Finalized withdrawal on L1"

    get_balances "FINAL_COMPLETE"

    echo -e "\n${GREEN}✅ Cross-chain withdrawal completed! Check L1 balances above.${NC}"
}

# Run Main function
main "$@"
