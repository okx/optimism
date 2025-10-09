#!/bin/bash
set -e

echo "🧪 Running Bridge Revert Tests..."

# Color definitions
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
PURPLE='\033[0;35m'
NC='\033[0m' # No Color

# Default RPC URLs
L1_RPC_URL=${L1_RPC_URL:-"http://127.0.0.1:8545"}
L2_RPC_URL=${L2_RPC_URL:-"http://127.0.0.1:8123"}

echo "🔧 Configuration:"
echo "L1 RPC: $L1_RPC_URL"
echo "L2 RPC: $L2_RPC_URL"

# Change to contracts directory
cd /Users/oker/Desktop/optimism/packages/contracts-bedrock

# Set up environment
export PATH="$HOME/.foundry/bin:$PATH"

echo -e "\n${GREEN}🚀 Starting Bridge Revert Tests${NC}"
echo "=================================================="

echo -e "\n${BLUE}🔍 Testing L1 Bridge Methods (Foundry)${NC}"
echo "=============================================="

# Run L1 tests
echo "Running L1 bridge tests..."
if forge test --match-contract CrossChainERC20_Test -v; then
    echo -e "${GREEN}✅ L1 bridge tests passed${NC}"
    L1_RESULT="PASS"
else
    echo -e "${RED}❌ L1 bridge tests failed${NC}"
    L1_RESULT="FAIL"
fi

echo -e "\n${BLUE}🔍 Testing L2 Bridge Methods (Foundry)${NC}"
echo "=============================================="

# Run L2 tests with L2 RPC
echo "Running L2 bridge tests..."
if L2_RPC_URL="$L2_RPC_URL" forge test --match-contract L2CrossChainERC20_Test -v; then
    echo -e "${GREEN}✅ L2 bridge tests passed${NC}"
    L2_RESULT="PASS"
else
    echo -e "${RED}❌ L2 bridge tests failed${NC}"
    L2_RESULT="FAIL"
fi

# Show summary
echo -e "\n${GREEN}🎉 Bridge Revert Testing Completed!${NC}"
echo "=================================================="

echo -e "\n${YELLOW}📋 Test Results Summary:${NC}"
echo "• L1 Bridge Tests: $L1_RESULT"
echo "• L2 Bridge Tests: $L2_RESULT"

echo -e "\n${YELLOW}🔗 Test Configuration:${NC}"
echo "• L1 Network: $L1_RPC_URL"
echo "• L2 Network: $L2_RPC_URL"
echo "• Test Framework: Foundry"
echo "• Test Files:"
echo "  - test/poc/CrossChainERC20.t.sol (L1 tests)"
echo "  - test/poc/L2CrossChainERC20.t.sol (L2 tests)"

echo -e "\n${YELLOW}💡 What was tested:${NC}"
echo "• L1 Bridge Methods:"
echo "  - bridgeERC20To (expects: 'not allow bridge')"
echo "  - bridgeETHTo (expects: 'not allow bridge')"
echo "  - depositERC20To (expects: 'not allow bridge')"
echo "  - finalizeERC20Withdrawal (expects: 'not allow bridge')"
echo "  - finalizeETHWithdrawal (expects: 'not allow bridge')"

echo "• L2 Bridge Methods:"
echo "  - bridgeERC20To (expects: 'not allow bridge')"
echo "  - withdrawTo (expects: revert)"
echo "  - withdraw (expects: 'onlyEOA')"
echo "  - finalizeBridgeERC20 (expects: 'onlyOtherBridge')"

if [ "$L1_RESULT" = "PASS" ] && [ "$L2_RESULT" = "PASS" ]; then
    echo -e "\n${GREEN}✅ All bridge methods are correctly disabled on both L1 and L2!${NC}"
    exit 0
else
    echo -e "\n${RED}❌ Some tests failed. Please check the output above.${NC}"
    exit 1
fi
