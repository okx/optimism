#!/bin/bash

set -e
set -x

BANKER_PRIVATE_KEY=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d
BANKER=$(cast wallet a $BANKER_PRIVATE_KEY)

# Source environment variables
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENV_FILE="$(dirname "$SCRIPT_DIR")/.env"
source "$ENV_FILE"

TO_ADDRESS=0x8f8E2d6cF621f30e9a11309D6A56A876281Fd534
AMOUNT=1000000ether

cast send $TO_ADDRESS --value $AMOUNT --legacy --rpc-url=$L2_RPC_URL --private-key=$BANKER_PRIVATE_KEY
