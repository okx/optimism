#!/bin/bash
set -e

echo "🚀 Deploying WOKB Cross-Chain Infrastructure..."

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
if [ -z "$L1_RPC_URL" ] || [ -z "$L2_RPC_URL" ] || [ -z "$DEPLOYER_PRIVATE_KEY" ]; then
    echo "❌ Missing required environment variables (L1_RPC_URL, L2_RPC_URL, DEPLOYER_PRIVATE_KEY)"
    exit 1
fi

# Set deployer address
DEPLOYER_ADDRESS=$(cast wallet address "$DEPLOYER_PRIVATE_KEY")
echo "Deployer address: $DEPLOYER_ADDRESS"

# Check L1 balance only
check_l1_balance() {
    echo -e "\n${BLUE}💰 Checking L1 deployer balance...${NC}"

    L1_BALANCE=$(cast balance "$DEPLOYER_ADDRESS" --rpc-url "$L1_RPC_URL")
    echo "L1 balance: $L1_BALANCE wei"

    if [ "$L1_BALANCE" = "0" ]; then
        echo "❌ Deployer has no L1 balance. Please fund the account first."
        exit 1
    fi

    echo "✅ Deployer has sufficient L1 balance"
}

# Deploy L2 WOKB contract
deploy_l2_wokb() {
    echo -e "\n${BLUE}🚀 Step 1: Deploying L2 WOKB contract...${NC}"

    # Check if WOKB already deployed by checking .env file and verifying contract
    echo "Checking if L2 WOKB already deployed..."

    if grep -q "^L2_WOKB_ADDRESS=" .env; then
        EXISTING_WOKB_ADDRESS=$(grep "^L2_WOKB_ADDRESS=" .env | cut -d'=' -f2)
        echo "Found L2_WOKB_ADDRESS in .env: $EXISTING_WOKB_ADDRESS"

        if [ -n "$EXISTING_WOKB_ADDRESS" ] && [ "$EXISTING_WOKB_ADDRESS" != "0x0000000000000000000000000000000000000000" ]; then
            # Verify the address has contract code
            echo "Verifying contract code at address..."
            EXISTING_CODE=$(cast code "$EXISTING_WOKB_ADDRESS" --rpc-url "$L2_RPC_URL" 2>/dev/null || echo "0x")

            if [ "$EXISTING_CODE" != "0x" ] && [ -n "$EXISTING_CODE" ]; then
                echo "✅ Contract code found, verifying it's a valid WOKB contract..."

                # Verify it's actually a WOKB contract by checking basic functions
                CONTRACT_NAME=$(cast call "$EXISTING_WOKB_ADDRESS" "name()" --rpc-url "$L2_RPC_URL" 2>/dev/null | cast --to-ascii 2>/dev/null | xargs || echo "")
                CONTRACT_SYMBOL=$(cast call "$EXISTING_WOKB_ADDRESS" "symbol()" --rpc-url "$L2_RPC_URL" 2>/dev/null | cast --to-ascii 2>/dev/null | xargs || echo "")

                # Check if it matches our expected WOKB contract
                if [[ "$CONTRACT_NAME" == *"Wrapped"* ]] && [[ "$CONTRACT_SYMBOL" == *"WOKB"* ]]; then
                    echo "✅ L2 WOKB already deployed and verified at: $EXISTING_WOKB_ADDRESS"
                    echo "   Name: $CONTRACT_NAME"
                    echo "   Symbol: $CONTRACT_SYMBOL"
                    L2_WOKB_ADDRESS="$EXISTING_WOKB_ADDRESS"
                    return 0
                else
                    echo "⚠️  Contract at address doesn't match expected WOKB contract:"
                    echo "   Name: '$CONTRACT_NAME' (expected: contains 'Wrapped')"
                    echo "   Symbol: '$CONTRACT_SYMBOL' (expected: contains 'WOKB')"
                    echo "   Will deploy a new WOKB contract..."
                fi
            else
                echo "⚠️  Address in .env has no contract code (L2 chain may have been reset)"
                echo "   Will deploy a new WOKB contract..."
            fi
        else
            echo "⚠️  Invalid address in .env file"
        fi
    else
        echo "No L2_WOKB_ADDRESS found in .env file"
    fi

    echo "Proceeding with new L2 WOKB deployment..."

    # Compile WOKB contract
    echo "Compiling WOKB contract..."
    cd ../packages/contracts-bedrock
    if ! forge compile src/L2/xlayer/WOKB.sol --force; then
        echo "❌ Failed to compile WOKB contract"
        cd ../../test
        exit 1
    fi
    echo "✅ WOKB contract compiled successfully"
    cd ../../test

    # Deploy WOKB contract using forge create with PROPOSER credentials
    echo "Deploying WOKB contract to L2 using PROPOSER credentials..."
    echo "Using PROPOSER_ADDRESS: $PROPOSER_ADDRESS"

    cd ../packages/contracts-bedrock
    forge create --json --broadcast src/L2/xlayer/WOKB.sol:WOKB \
        --rpc-url "$L2_RPC_URL" \
        --private-key "$PROPOSER_PRIVATE_KEY" \
        > /tmp/forge_result.json 2>&1
    cd ../../test

    FORGE_EXIT_CODE=$?

    # Check deployment result
    if [ $FORGE_EXIT_CODE -ne 0 ]; then
        echo "❌ Failed to deploy WOKB contract - exit code: $FORGE_EXIT_CODE"
        if [ -f /tmp/forge_result.json ]; then
            echo "Error: $(cat /tmp/forge_result.json)"
        fi
        exit 1
    fi

    # Extract contract address and transaction hash
    if [ -f /tmp/forge_result.json ]; then
        DEPLOY_RESULT=$(cat /tmp/forge_result.json)
        L2_WOKB_ADDRESS=$(echo "$DEPLOY_RESULT" | jq -r '.deployedTo // empty' 2>/dev/null)
        TRANSACTION_HASH=$(echo "$DEPLOY_RESULT" | jq -r '.transactionHash' 2>/dev/null)
    else
        echo "❌ No deployment result file found"
        exit 1
    fi

    if [ "$TRANSACTION_HASH" = "null" ] || [ -z "$TRANSACTION_HASH" ] || [ "$L2_WOKB_ADDRESS" = "null" ] || [ -z "$L2_WOKB_ADDRESS" ]; then
        echo "❌ Failed to extract contract address from deployment result"
        echo "Raw output: $DEPLOY_RESULT"
        exit 1
    fi

    echo "✅ L2 WOKB contract deployed successfully"
    echo "   Address: $L2_WOKB_ADDRESS"
    echo "   Transaction: $TRANSACTION_HASH"

    # Verify deployment
    echo "Verifying L2 WOKB deployment..."
    L2_CODE=$(cast code "$L2_WOKB_ADDRESS" --rpc-url "$L2_RPC_URL")
    if [ "$L2_CODE" = "0x" ]; then
        echo "❌ L2 WOKB contract code is empty"
        exit 1
    fi

    # Test contract functions
    L2_NAME=$(cast call "$L2_WOKB_ADDRESS" "name()" --rpc-url "$L2_RPC_URL" | cast --to-ascii | xargs)
    L2_SYMBOL=$(cast call "$L2_WOKB_ADDRESS" "symbol()" --rpc-url "$L2_RPC_URL" | cast --to-ascii | xargs)

    echo "✅ L2 WOKB contract verified"
    echo "   Name: $L2_NAME"
    echo "   Symbol: $L2_SYMBOL"
}

# Deploy L1 OptimismMintableERC20
deploy_l1_wokb() {
    echo -e "\n${BLUE}🚀 Step 2: Creating L1 OptimismMintableERC20...${NC}"

    # Get L1 OptimismMintableERC20Factory address from deployment state
    if [ -f "config-op/state.json" ]; then
        FACTORY_ADDRESS=$(jq -r '.opChainDeployments[0].OptimismMintableErc20FactoryProxy' config-op/state.json)
        if [ "$FACTORY_ADDRESS" = "null" ] || [ -z "$FACTORY_ADDRESS" ]; then
            echo "❌ Could not find OptimismMintableErc20FactoryProxy in config-op/state.json"
            exit 1
        fi
        echo "✅ Factory address from state.json: $FACTORY_ADDRESS"
    else
        echo "❌ config-op/state.json not found"
        exit 1
    fi

    # Verify factory exists
    FACTORY_CODE=$(cast code "$FACTORY_ADDRESS" --rpc-url "$L1_RPC_URL")
    if [ "$FACTORY_CODE" = "0x" ]; then
        echo "❌ Factory contract not found at $FACTORY_ADDRESS"
        echo "💡 This usually means the network was restarted and contracts were redeployed"
        exit 1
    fi
    echo "✅ Factory contract verified"

    # Check if WOKB already deployed by checking .env file and verifying contract
    echo "Checking if WOKB already deployed..."

    if grep -q "^L1_WOKB_ADDRESS=" .env; then
        EXISTING_WOKB_ADDRESS=$(grep "^L1_WOKB_ADDRESS=" .env | cut -d'=' -f2)
        echo "Found L1_WOKB_ADDRESS in .env: $EXISTING_WOKB_ADDRESS"

        if [ -n "$EXISTING_WOKB_ADDRESS" ] && [ "$EXISTING_WOKB_ADDRESS" != "0x0000000000000000000000000000000000000000" ]; then
            # Verify the address has contract code
            echo "Verifying contract code at address..."
            EXISTING_CODE=$(cast code "$EXISTING_WOKB_ADDRESS" --rpc-url "$L1_RPC_URL" 2>/dev/null || echo "0x")

            if [ "$EXISTING_CODE" != "0x" ] && [ -n "$EXISTING_CODE" ]; then
                echo "✅ Contract code found, verifying it's a valid WOKB contract..."

                # Verify it's actually a WOKB contract by checking basic functions
                CONTRACT_NAME=$(cast call "$EXISTING_WOKB_ADDRESS" "name()" --rpc-url "$L1_RPC_URL" 2>/dev/null | cast --to-ascii 2>/dev/null | xargs || echo "")
                CONTRACT_SYMBOL=$(cast call "$EXISTING_WOKB_ADDRESS" "symbol()" --rpc-url "$L1_RPC_URL" 2>/dev/null | cast --to-ascii 2>/dev/null | xargs || echo "")
                REMOTE_TOKEN=$(cast call "$EXISTING_WOKB_ADDRESS" "remoteToken()" --rpc-url "$L1_RPC_URL" 2>/dev/null || echo "")

                # Check if it matches our expected WOKB contract
                EXPECTED_REMOTE="0x000000000000000000000000$(echo "$L2_WOKB_ADDRESS" | cut -c3-)"
                EXPECTED_REMOTE_LOWER=$(echo "$EXPECTED_REMOTE" | tr '[:upper:]' '[:lower:]')
                if [ "$CONTRACT_NAME" = "Wrapped OKB" ] && [ "$CONTRACT_SYMBOL" = "WOKB" ] && [ "$REMOTE_TOKEN" = "$EXPECTED_REMOTE_LOWER" ]; then
                    echo "✅ WOKB already deployed and verified at: $EXISTING_WOKB_ADDRESS"
                    echo "   Name: $CONTRACT_NAME"
                    echo "   Symbol: $CONTRACT_SYMBOL"
                    echo "   Remote Token: $REMOTE_TOKEN"
                    L1_WOKB_ADDRESS="$EXISTING_WOKB_ADDRESS"
                    return 0
                else
                    echo "⚠️  Contract at address doesn't match expected WOKB contract:"
                    echo "   Name: '$CONTRACT_NAME' (expected: 'Wrapped OKB')"
                    echo "   Symbol: '$CONTRACT_SYMBOL' (expected: 'WOKB')"
                    echo "   Remote Token: '$REMOTE_TOKEN' (expected: '$EXPECTED_REMOTE_LOWER')"
                    echo "   Will deploy a new WOKB contract..."
                fi
            else
                echo "⚠️  Address in .env has no contract code (L1 chain may have been reset)"
                echo "   Will deploy a new WOKB contract..."
            fi
        else
            echo "⚠️  Invalid address in .env file"
        fi
    else
        echo "No L1_WOKB_ADDRESS found in .env file"
    fi

    echo "Proceeding with new WOKB deployment..."

    # Get current block number for log filtering
    CURRENT_BLOCK=$(cast block-number --rpc-url "$L1_RPC_URL")
    echo "Current block number: $CURRENT_BLOCK"

    # Create OptimismMintableERC20 with correct function
    echo "Creating OptimismMintableERC20 for L2 WOKB..."

    # Try to create the token, capture both stdout and stderr
    TX_RESULT=$(cast send "$FACTORY_ADDRESS" \
        "createOptimismMintableERC20WithDecimals(address,string,string,uint8)" \
        "$L2_WOKB_ADDRESS" "Wrapped OKB" "WOKB" "18" \
        --rpc-url "$L1_RPC_URL" \
        --private-key "$DEPLOYER_PRIVATE_KEY" \
        --json 2>&1)

    # Check if the command succeeded
    if echo "$TX_RESULT" | grep -q "execution reverted"; then
        echo "⚠️  Transaction reverted - this usually means a token with the same parameters already exists"
        echo "💡 Trying to find existing deployment..."

        # Try to calculate the expected address or find it through events
        # For now, we'll exit with an informative error
        echo "❌ Cannot proceed: Token with same parameters (L2 token: $L2_WOKB_ADDRESS, name: 'Wrapped OKB', symbol: 'WOKB') may already exist"
        echo "💡 To fix this, either:"
        echo "   1. Use different token name/symbol parameters"
        echo "   2. Find and use the existing L1 token address"
        echo "   3. Reset the L1 chain if this is a test environment"
        exit 1
    fi

    TRANSACTION_HASH=$(echo "$TX_RESULT" | jq -r '.transactionHash' 2>/dev/null)

    if [ "$TRANSACTION_HASH" = "null" ] || [ -z "$TRANSACTION_HASH" ]; then
        echo "❌ Failed to create OptimismMintableERC20"
        echo "Raw output: $TX_RESULT"
        exit 1
    fi

    echo "✅ Transaction sent: $TRANSACTION_HASH"

    # Extract contract address from logs
    # OptimismMintableERC20Created event signature: 0x52fe89dd5930f343d25650b62fd367bae47088bcddffd2a88350a6ecdd620cdb
    L1_WOKB_ADDRESS=$(echo "$TX_RESULT" | jq -r '
        .logs[] |
        select(.topics[0] == "0x52fe89dd5930f343d25650b62fd367bae47088bcddffd2a88350a6ecdd620cdb") |
        .topics[1] |
        ltrimstr("0x000000000000000000000000") |
        "0x" + .'
    )

    if [ -z "$L1_WOKB_ADDRESS" ] || [ "$L1_WOKB_ADDRESS" = "0x" ]; then
        echo "❌ Could not extract contract address from transaction logs"
        exit 1
    fi

    echo "✅ WOKB L1 contract created at: $L1_WOKB_ADDRESS"
}

# Verify deployment
verify_deployment() {
    echo -e "\n${BLUE}🔍 Step 3: Verifying deployment...${NC}"

    # Check L2 WOKB contract
    L2_CODE=$(cast code "$L2_WOKB_ADDRESS" --rpc-url "$L2_RPC_URL")
    if [ "$L2_CODE" = "0x" ]; then
        echo "❌ L2 WOKB contract code is empty"
        exit 1
    fi
    echo "✅ L2 WOKB contract verified"

    # Check L1 contract
    L1_CODE=$(cast code "$L1_WOKB_ADDRESS" --rpc-url "$L1_RPC_URL")
    if [ "$L1_CODE" = "0x" ]; then
        echo "❌ L1 contract code is empty"
        exit 1
    fi
    echo "✅ L1 contract verified"

    # Test contract functions
    echo "Testing L1 contract functions..."
    L1_NAME=$(cast call "$L1_WOKB_ADDRESS" "name()" --rpc-url "$L1_RPC_URL" | cast --to-ascii | xargs)
    L1_SYMBOL=$(cast call "$L1_WOKB_ADDRESS" "symbol()" --rpc-url "$L1_RPC_URL" | cast --to-ascii | xargs)
    L1_REMOTE=$(cast call "$L1_WOKB_ADDRESS" "remoteToken()" --rpc-url "$L1_RPC_URL")

    echo "   Name: $L1_NAME"
    echo "   Symbol: $L1_SYMBOL"
    echo "   Remote Token: $L1_REMOTE"

    # Verify remote token matches L2 WOKB (case-insensitive comparison)
    EXPECTED_REMOTE="0x000000000000000000000000$(echo "$L2_WOKB_ADDRESS" | cut -c3-)"
    EXPECTED_REMOTE_LOWER=$(echo "$EXPECTED_REMOTE" | tr '[:upper:]' '[:lower:]')

    if [ "$L1_REMOTE" != "$EXPECTED_REMOTE_LOWER" ]; then
        echo "❌ Remote token address mismatch"
        echo "   Expected: $EXPECTED_REMOTE_LOWER"
        echo "   Got: $L1_REMOTE"
        exit 1
    fi
    echo "✅ Remote token verification passed"
}

# Show summary
show_summary() {
    echo -e "\n${GREEN}🎉 WOKB Cross-Chain Infrastructure Deployed Successfully!${NC}"
    echo -e "\n${YELLOW}📋 Deployment Summary:${NC}"
    echo "L1 WOKB (OptimismMintableERC20): $L1_WOKB_ADDRESS"
    echo "L2 WOKB (Custom Contract): $L2_WOKB_ADDRESS"
    echo "Transaction Hash: $TRANSACTION_HASH"
    echo "Factory Address: $FACTORY_ADDRESS"

    echo -e "\n${YELLOW}🔗 Cross-Chain Architecture:${NC}"
    echo "• L2: Users deposit OKB → get WOKB (via WOKB contract)"
    echo "• L1: OptimismMintableERC20 represents L2 WOKB"
    echo "• Bridge: StandardBridge handles L1 ↔ L2 transfers"

    echo -e "\n${YELLOW}🚀 Next Steps:${NC}"
    echo "1. Test basic functions: ./test_basic_cgt.sh"
    echo "2. Test cross-chain transfers: ./test_cross_chain_cgt.sh"
    echo "3. Update test scripts with new L1 address: $L1_WOKB_ADDRESS"
}

# Main function
main() {
    echo -e "${GREEN}Starting WOKB deployment...${NC}"

    # Check L1 balance only
    check_l1_balance

    # Deploy L2 WOKB
    deploy_l2_wokb

    # Deploy L1 WOKB
    deploy_l1_wokb

    # Verify deployment
    verify_deployment

    # Show summary
    show_summary

    echo -e "\n${YELLOW}📝 Updating .env file with the new WOKB addresses...${NC}"

    # Update .env file with WOKB addresses
    echo "Updating .env file with WOKB addresses..."

    # Function to update or add environment variable
    update_env_var() {
        local var_name="$1"
        local var_value="$2"

        if grep -q "^${var_name}=" .env; then
            # Update existing line
            sed -i "" "s/^${var_name}=.*/${var_name}=${var_value}/" .env
            echo "✅ Updated existing ${var_name} in .env"
        else
            # Add new line
            echo "${var_name}=${var_value}" >> .env
            echo "✅ Added ${var_name} to .env"
        fi
    }

    # Check if this is the first WOKB variable being added
    local has_l1_wokb=$(grep -q "^L1_WOKB_ADDRESS=" .env && echo "true" || echo "false")
    local has_l2_wokb=$(grep -q "^L2_WOKB_ADDRESS=" .env && echo "true" || echo "false")

    if [ "$has_l1_wokb" = "false" ] && [ "$has_l2_wokb" = "false" ]; then
        echo "# WOKB Contract Address (auto-generated)" >> .env
    fi

    # Update both addresses
    update_env_var "L1_WOKB_ADDRESS" "$L1_WOKB_ADDRESS"
    update_env_var "L2_WOKB_ADDRESS" "$L2_WOKB_ADDRESS"

    echo -e "\n${GREEN}✅ .env file updated with WOKB addresses:${NC}"
    echo "   L1 WOKB: $L1_WOKB_ADDRESS"
    echo "   L2 WOKB: $L2_WOKB_ADDRESS"
    echo -e "${YELLOW}💡 Test scripts will now read WOKB addresses from environment variables${NC}"
}

# Run main function
main "$@"
