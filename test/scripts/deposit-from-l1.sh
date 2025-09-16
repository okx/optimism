#!/bin/bash

set -e

# Source environment variables
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENV_FILE="$(dirname "$SCRIPT_DIR")/.env"
source "$ENV_FILE"

OP_PORTAL_ADDRESS=$(jq -r '.opChainDeployments[0].OptimismPortalProxy' config-op/state.json)
PRIVATE_KEY="0x815405dddb0e2a99b12af775fd2929e526704e1d1aea6a0b4e74dc33e2f7fcd2"
ADDRESS=$(cast wallet a $PRIVATE_KEY)
ETHER=100000000000000000
AMOUNT=$(python3 -c "print(3000 * $ETHER)")
AMOUNT_PLUS_FEE=$(python3 -c "print($AMOUNT + $ETHER)")

cast send --private-key $RICH_L1_PRIVATE_KEY --value $AMOUNT_PLUS_FEE $ADDRESS --legacy

PRE_BALANCE=$(cast balance 0x8f8E2d6cF621f30e9a11309D6A56A876281Fd534 --rpc-url=http://127.0.0.1:8123)

cast send \
    --legacy \
    --private-key $PRIVATE_KEY \
    --value $AMOUNT \
    $OP_PORTAL_ADDRESS \
    'function depositTransaction(address _target, uint256 _value, uint64 _gasLimit, bool _isCreation, bytes _data)' \
    $ADDRESS $AMOUNT 100000 false 0x

echo " 📋 Initial balance: $PRE_BALANCE"

START_TIME=$(date +%s)

while true; do
    NEW_BALANCE=$(cast balance 0x8f8E2d6cF621f30e9a11309D6A56A876281Fd534 --rpc-url=http://127.0.0.1:8123)

    if [ "$NEW_BALANCE" != "$PRE_BALANCE" ]; then
        CURRENT_TIME=$(date +%s)
        ELAPSED_TIME=$((CURRENT_TIME - START_TIME))
        echo " ✅ Balance changed!"
        echo " 📋 Previous balance: $PRE_BALANCE"
        echo " 📋 New balance: $NEW_BALANCE"
        echo " ⏱️ Total wait time: ${ELAPSED_TIME}s"
        break
    fi

    echo " ⏳ waiting for change..."
    sleep 3
done
