#!/bin/bash

set -e

# Function to display usage
usage() {
    echo "Usage: $0 [OPTIONS]"
    echo "Options:"
    echo "  --game-type TYPE             Game type (uint32) (required)"
    echo "  --init-bond WEI              Initial bond amount in wei (required)"
    echo "  --transactor ADDRESS         Transactor contract address (required)"
    echo "  --dispute-game-factory ADDRESS DisputeGameFactory contract address (required)"
    echo "  --private-key KEY            Private key for transaction (required)"
    echo "  --rpc-url URL               RPC URL (required)"
    echo "  --help                      Show this help message"
    echo ""
    echo "Example:"
    echo "  $0 --game-type 1 --init-bond 1000000000000000000 --transactor 0x456... --dispute-game-factory 0xabc... --private-key 0xdef... --rpc-url https://..."
    exit 1
}

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --game-type)
            GAME_TYPE="$2"
            shift 2
            ;;
        --init-bond)
            INIT_BOND="$2"
            shift 2
            ;;
        --transactor)
            TRANSACTOR_ADDRESS="$2"
            shift 2
            ;;
        --dispute-game-factory)
            DISPUTE_GAME_FACTORY="$2"
            shift 2
            ;;
        --private-key)
            PRIVATE_KEY="$2"
            shift 2
            ;;
        --rpc-url)
            RPC_URL="$2"
            shift 2
            ;;
        --help)
            usage
            ;;
        *)
            echo "Unknown option: $1"
            usage
            ;;
    esac
done

# Validate required parameters
if [[ -z "$GAME_TYPE" || -z "$INIT_BOND" || -z "$TRANSACTOR_ADDRESS" || -z "$DISPUTE_GAME_FACTORY" || -z "$PRIVATE_KEY" || -z "$RPC_URL" ]]; then
    echo "Error: Missing required parameters"
    echo ""
    usage
fi

echo "=== Setting Init Bond via Transactor ==="
echo "Game Type: $GAME_TYPE"
echo "Init Bond: $INIT_BOND wei"
echo "Dispute Game Factory: $DISPUTE_GAME_FACTORY"
echo "Transactor Address: $TRANSACTOR_ADDRESS"
echo "RPC URL: $RPC_URL"
echo ""

# Get sender address from private key
SENDER_ADDRESS=$(cast wallet address --private-key $PRIVATE_KEY)
echo "Sender Address: $SENDER_ADDRESS"
echo ""

# Check if game type exists
echo "Checking if game type exists..."
GAME_IMPL=$(cast call --rpc-url $RPC_URL $DISPUTE_GAME_FACTORY "gameImpls(uint32)(address)" $GAME_TYPE)
echo "Game Type $GAME_TYPE Implementation: $GAME_IMPL"

if [ "$GAME_IMPL" == "0x0000000000000000000000000000000000000000" ]; then
    echo "Error: Game type $GAME_TYPE does not exist. Cannot set init bond."
    exit 1
fi

# Get current init bond for comparison
echo "Retrieving current init bond..."
CURRENT_INIT_BOND_RAW=$(cast call --rpc-url $RPC_URL $DISPUTE_GAME_FACTORY "initBonds(uint32)(uint256)" $GAME_TYPE)
CURRENT_INIT_BOND=$(echo $CURRENT_INIT_BOND_RAW | sed 's/\[.*\]//' | xargs)
echo "Current Init Bond: $CURRENT_INIT_BOND wei"

if [ "$CURRENT_INIT_BOND" == "$INIT_BOND" ]; then
    echo "Warning: New init bond is the same as current init bond. No change needed."
    exit 0
fi

echo "Creating setInitBond calldata..."
echo "Game Type: $GAME_TYPE"
echo "New Init Bond: $INIT_BOND wei"

# Create calldata for setInitBond function
SETINITBOND_CALLDATA=$(cast calldata "setInitBond(uint32,uint256)" $GAME_TYPE $INIT_BOND)

echo "SetInitBond calldata: $SETINITBOND_CALLDATA"
echo ""

# Create calldata for Transactor's DELEGATECALL function
echo "Creating Transactor CALL calldata..."
TRANSACTOR_CALLDATA=$(cast calldata "CALL(address,bytes,uint256)" $DISPUTE_GAME_FACTORY $SETINITBOND_CALLDATA 0) 

echo "Transactor calldata: $TRANSACTOR_CALLDATA"
echo ""

# Execute the transaction through Transactor
echo "Executing transaction via Transactor..."
echo "Target: $TRANSACTOR_ADDRESS"
echo "From: $SENDER_ADDRESS"

cast send \
    --rpc-url $RPC_URL \
    --private-key $PRIVATE_KEY \
    --from $SENDER_ADDRESS \
    $TRANSACTOR_ADDRESS \
    $TRANSACTOR_CALLDATA \
    --json |jq

echo ""
echo "Transaction sent! Check the transaction hash above for confirmation."
echo ""

# Verify the init bond was updated
echo "Verifying init bond was updated..."
NEW_INIT_BOND=$(cast call --rpc-url $RPC_URL $DISPUTE_GAME_FACTORY "initBonds(uint32)(uint256)" $GAME_TYPE)

# Extract the numeric value from the response (remove scientific notation and trim spaces)
NEW_INIT_BOND_NUMERIC=$(echo $NEW_INIT_BOND | sed 's/\[.*\]//' | xargs)

if [ "$NEW_INIT_BOND_NUMERIC" == "$INIT_BOND" ]; then
    echo "✅ Success! Init bond updated successfully."
    echo "Previous Init Bond: $CURRENT_INIT_BOND wei"
    echo "Game Type $GAME_TYPE Init Bond updated to: $NEW_INIT_BOND_NUMERIC wei"
else
    echo "❌ Warning: Init bond was not updated as expected."
    echo "Expected: $INIT_BOND wei"
    echo "Actual: $NEW_INIT_BOND_NUMERIC wei"
fi

echo ""
echo "Script completed."
