#!/bin/bash
set -e
source .env
docker compose up -d l1-validator

sleep 3

OP_BATCHER_ADDR=$(cast wallet a $OP_BATCHER_PRIVATE_KEY)
OP_PROPOSER_ADDR=$(cast wallet a $OP_PROPOSER_PRIVATE_KEY)
OP_CHALLENGER_ADDR=$(cast wallet a $OP_CHALLENGER_PRIVATE_KEY)

while [[ "$(cast rpc eth_syncing)" != "false" ]]; do
    echo "Waiting for node to finish syncing..."
    sleep 1
done

cast send --private-key $RICH_L1_PRIVATE_KEY --value 100ether $OP_BATCHER_ADDR --legacy
cast send --private-key $RICH_L1_PRIVATE_KEY --value 100ether $OP_PROPOSER_ADDR --legacy
cast send --private-key $RICH_L1_PRIVATE_KEY --value 100ether $OP_CHALLENGER_ADDR --legacy
