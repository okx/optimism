#!/bin/bash
set -e

echo "🧪 Starting Basic CGT Functionality Tests..."

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

# Test results counter
TOTAL_TESTS=0
PASSED_TESTS=0
FAILED_TESTS=0

# Test function
run_test() {
    local test_name="$1"
    local test_command="$2"

    echo -e "\n${BLUE}🔍 Running test: $test_name${NC}"
    TOTAL_TESTS=$((TOTAL_TESTS + 1))

    if eval "$test_command"; then
        echo -e "${GREEN}✅ PASSED: $test_name${NC}"
        PASSED_TESTS=$((PASSED_TESTS + 1))
    else
        echo -e "${RED}❌ FAILED: $test_name${NC}"
        FAILED_TESTS=$((FAILED_TESTS + 1))
    fi
}

# Test RPC connections
test_rpc_connections() {
    echo "Testing RPC connections..."

    # Test L1 RPC
    L1_BLOCK=$(cast block-number --rpc-url "$L1_RPC_URL" 2>/dev/null)
    if [ $? -ne 0 ] || [ -z "$L1_BLOCK" ]; then
        echo "❌ L1 RPC connection failed"
        return 1
    fi
    echo "L1 block number: $L1_BLOCK"

    # Test L2 RPC
    L2_BLOCK=$(cast block-number --rpc-url "$L2_RPC_URL" 2>/dev/null)
    if [ $? -ne 0 ] || [ -z "$L2_BLOCK" ]; then
        echo "❌ L2 RPC connection failed"
        return 1
    fi
    echo "L2 block number: $L2_BLOCK"

    echo "✅ RPC connections successful"
    return 0
}

# Test CGT predeploy contracts
test_cgt_predeploys() {
    echo "Testing CGT predeploy contracts..."

    # LiquidityController predeploy address
    LIQUIDITY_CONTROLLER_ADDR="0x420000000000000000000000000000000000002a"

    # NativeAssetLiquidity predeploy address
    NATIVE_LIQUIDITY_ADDR="0x4200000000000000000000000000000000000029"

    # Check LiquidityController
    LIQUIDITY_CONTROLLER_CODE=$(cast code "$LIQUIDITY_CONTROLLER_ADDR" --rpc-url "$L2_RPC_URL")
    if [ "$LIQUIDITY_CONTROLLER_CODE" = "0x" ] || [ -z "$LIQUIDITY_CONTROLLER_CODE" ]; then
        echo "❌ LiquidityController not found at predeploy address"
        return 1
    fi
    echo "✅ LiquidityController found"

    # Check NativeAssetLiquidity
    NATIVE_LIQUIDITY_CODE=$(cast code "$NATIVE_LIQUIDITY_ADDR" --rpc-url "$L2_RPC_URL")
    if [ "$NATIVE_LIQUIDITY_CODE" = "0x" ] || [ -z "$NATIVE_LIQUIDITY_CODE" ]; then
        echo "❌ NativeAssetLiquidity not found at predeploy address"
        return 1
    fi
    echo "✅ NativeAssetLiquidity found"

    return 0
}

# Test L1Block CGT functionality
test_l1block_cgt() {
    echo "Testing L1Block CGT functionality..."

    # L1Block predeploy address
    L1BLOCK_ADDR="0x4200000000000000000000000000000000000015"

    # Check if L1Block exists
    L1BLOCK_CODE=$(cast code "$L1BLOCK_ADDR" --rpc-url "$L2_RPC_URL")
    if [ "$L1BLOCK_CODE" = "0x" ] || [ -z "$L1BLOCK_CODE" ]; then
        echo "❌ L1Block not found at predeploy address"
        return 1
    fi
    echo "✅ L1Block found"

    # Check if CGT is enabled
    IS_CGT=$(cast call "$L1BLOCK_ADDR" "isCustomGasToken()" --rpc-url "$L2_RPC_URL")
    if [ "$IS_CGT" != "0x0000000000000000000000000000000000000000000000000000000000000001" ]; then
        echo "❌ CGT not enabled in L1Block. Got: $IS_CGT"
        return 1
    fi
    echo "✅ CGT enabled in L1Block"

    # Check gas paying token info
    GAS_TOKEN_NAME=$(cast call "$L1BLOCK_ADDR" "gasPayingTokenName()" --rpc-url "$L2_RPC_URL")
    GAS_TOKEN_SYMBOL=$(cast call "$L1BLOCK_ADDR" "gasPayingTokenSymbol()" --rpc-url "$L2_RPC_URL")

    echo "Gas paying token name: $GAS_TOKEN_NAME"
    echo "Gas paying token symbol: $GAS_TOKEN_SYMBOL"

    return 0
}

# Test WOKB contracts (using existing L2 WETH)
test_wokb_contracts() {
    echo "Testing WOKB contracts..."

    # L2 WETH (acts as WOKB) predeploy address
    WOKB_L2_ADDRESS="0x4200000000000000000000000000000000000006"

    # Check L2 WETH/WOKB contract
    L2_CODE=$(cast code "$WOKB_L2_ADDRESS" --rpc-url "$L2_RPC_URL")
    if [ "$L2_CODE" = "0x" ] || [ -z "$L2_CODE" ]; then
        echo "❌ L2 WOKB contract code not found"
        return 1
    fi

    L2_NAME=$(cast call "$WOKB_L2_ADDRESS" "name()" --rpc-url "$L2_RPC_URL" | cast --to-ascii)
    L2_SYMBOL=$(cast call "$WOKB_L2_ADDRESS" "symbol()" --rpc-url "$L2_RPC_URL" | cast --to-ascii)

    echo "✅ L2 WOKB contract found"
    echo "L2 WOKB Name: $L2_NAME"
    echo "L2 WOKB Symbol: $L2_SYMBOL"
    echo "L2 WOKB Address: $WOKB_L2_ADDRESS"

    # Check if L1 OptimismMintableERC20 exists (from previous deployment)
    if [ -f "contracts/wokb_l1_info.json" ]; then
        WOKB_L1_ADDRESS=$(jq -r '.address' contracts/wokb_l1_info.json)
        if [ "$WOKB_L1_ADDRESS" != "null" ] && [ -n "$WOKB_L1_ADDRESS" ]; then
            echo "✅ L1 OptimismMintableERC20 deployment info found: $WOKB_L1_ADDRESS"
        fi
    else
        echo "⚠️  L1 OptimismMintableERC20 not deployed yet. Run deploy_wokb_cgt.sh to deploy."
    fi

    return 0
}

# Test WOKB contracts (basic verification only)
test_wokb_contracts() {
    echo "Testing WOKB contracts..."

    # L2 WETH (acts as WOKB) predeploy address
    WOKB_L2_ADDRESS="0x4200000000000000000000000000000000000006"

    # Check L2 WETH/WOKB contract
    L2_CODE=$(cast code "$WOKB_L2_ADDRESS" --rpc-url "$L2_RPC_URL")
    if [ "$L2_CODE" = "0x" ] || [ -z "$L2_CODE" ]; then
        echo "❌ L2 WOKB contract code not found"
        return 1
    fi

    L2_NAME=$(cast call "$WOKB_L2_ADDRESS" "name()" --rpc-url "$L2_RPC_URL" | cast --to-ascii)
    L2_SYMBOL=$(cast call "$WOKB_L2_ADDRESS" "symbol()" --rpc-url "$L2_RPC_URL" | cast --to-ascii)

    echo "✅ L2 WOKB contract found"
    echo "L2 WOKB Name: $L2_NAME"
    echo "L2 WOKB Symbol: $L2_SYMBOL"
    echo "L2 WOKB Address: $WOKB_L2_ADDRESS"

    # Check if L1 OptimismMintableERC20 exists (from deployment)
    if [ -f "config-op/state.json" ]; then
        FACTORY_ADDRESS=$(jq -r '.opChainDeployments[0].OptimismMintableErc20FactoryProxy' config-op/state.json)
        if [ "$FACTORY_ADDRESS" != "null" ] && [ -n "$FACTORY_ADDRESS" ]; then
            # Check if WOKB was deployed for our L2 token
            EXISTING_L1_WOKB=$(cast call "$FACTORY_ADDRESS" "deployments(address)" "$WOKB_L2_ADDRESS" --rpc-url "$L1_RPC_URL" 2>/dev/null || echo "0x0000000000000000000000000000000000000000000000000000000000000000")
            if [ "$EXISTING_L1_WOKB" != "0x0000000000000000000000000000000000000000000000000000000000000000" ]; then
                L1_WOKB_ADDR=$(echo "$EXISTING_L1_WOKB" | sed 's/0x000000000000000000000000/0x/')
                echo "✅ L1 OptimismMintableERC20 found: $L1_WOKB_ADDR"
            else
                echo "⚠️  L1 OptimismMintableERC20 not deployed yet. Run deploy_wokb_cgt.sh to deploy."
            fi
        fi
    else
        echo "⚠️  config-op/state.json not found. Run deploy_wokb_cgt.sh to deploy L1 contract."
    fi

    return 0
}

# Main test function
main() {
    echo -e "${YELLOW}🚀 Starting Basic CGT Test Suite${NC}"
    echo "=================================================="

    # Run all tests
    run_test "RPC Connections" "test_rpc_connections"
    run_test "CGT Predeploys" "test_cgt_predeploys"
    run_test "L1Block CGT Functionality" "test_l1block_cgt"
    run_test "WOKB Contracts" "test_wokb_contracts"

    # Output test results
    echo -e "\n${YELLOW}📊 Test Results Summary${NC}"
    echo "=================================================="
    echo -e "Total Tests: ${BLUE}$TOTAL_TESTS${NC}"
    echo -e "Passed: ${GREEN}$PASSED_TESTS${NC}"
    echo -e "Failed: ${RED}$FAILED_TESTS${NC}"

    if [ $FAILED_TESTS -eq 0 ]; then
        echo -e "\n${GREEN}🎉 All basic CGT tests passed!${NC}"
        echo -e "\n${YELLOW}📝 Next Steps:${NC}"
        echo "• Run cross-chain tests: ./test_cross_chain_cgt.sh"
        echo "• Deploy L1 contract if needed: ./deploy_wokb_cgt.sh"
        exit 0
    else
        echo -e "\n${RED}❌ Some basic CGT tests failed. Please check the output above.${NC}"
        exit 1
    fi
}

# Run Main function
main "$@"
