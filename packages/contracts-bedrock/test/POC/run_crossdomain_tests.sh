#!/bin/bash
set -e

echo "🔗 Running CrossDomain Messenger Tests..."

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

echo -e "\n${GREEN}🚀 Starting CrossDomain Messenger Tests${NC}"
echo "=================================================="

echo -e "\n${BLUE}🔍 Testing CrossDomain Messenger Functionality (Foundry)${NC}"
echo "=============================================="

# Run CrossDomain tests
echo "Running CrossDomain messenger tests..."
if L1_RPC_URL="$L1_RPC_URL" L2_RPC_URL="$L2_RPC_URL" forge test --match-contract POC_CrossDomainMessenger_Test -vv; then
    echo -e "${GREEN}✅ CrossDomain messenger tests passed${NC}"
    CROSSDOMAIN_RESULT="PASS"
else
    echo -e "${RED}❌ CrossDomain messenger tests failed${NC}"
    CROSSDOMAIN_RESULT="FAIL"
fi

# Show summary
echo -e "\n${GREEN}🎉 CrossDomain Messenger Testing Completed!${NC}"
echo "=================================================="

echo -e "\n${YELLOW}📋 Test Results Summary:${NC}"
echo "• CrossDomain Messenger Tests: $CROSSDOMAIN_RESULT"

echo -e "\n${YELLOW}🔗 Test Configuration:${NC}"
echo "• L1 Network: $L1_RPC_URL"
echo "• L2 Network: $L2_RPC_URL"
echo "• Test Framework: Foundry"
echo "• Test File: test/poc/CrossDomainMessenger.t.sol"

echo -e "\n${YELLOW}💡 What was tested:${NC}"
echo "• L1 to L2 Message Sending:"
echo "  - sendMessage functionality"
echo "  - Message nonce increment"
echo "  - Basic messenger functions"

echo "• L2 to L1 Message Sending:"
echo "  - sendMessage functionality"
echo "  - Message nonce increment"
echo "  - Basic messenger functions"

echo "• Utility Functions:"
echo "  - Gas calculation for messages"
echo "  - SimpleReceiver contract functionality"

if [ "$CROSSDOMAIN_RESULT" = "PASS" ]; then
    echo -e "\n${GREEN}✅ All CrossDomain messenger tests passed!${NC}"
    exit 0
else
    echo -e "\n${RED}❌ Some tests failed. Please check the output above.${NC}"
    exit 1
fi
