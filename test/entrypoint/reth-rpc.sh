#!/bin/bash

set -e

source /.env

# Read the first argument (1 or 0), default to 0 if not provided
DISABLE_FLASHBLOCKS=${1:-0}

# Build the command with common arguments
CMD="op-reth node \
      --datadir=/datadir \
      --chain=/genesis.json \
      --config=/config.toml \
      --http \
      --http.corsdomain=* \
      --http.port=8545 \
      --http.addr=0.0.0.0 \
      --http.api=web3,debug,eth,txpool,net,miner,admin \
      --ws \
      --ws.addr=0.0.0.0 \
      --ws.port=7546 \
      --ws.origins=* \
      --ws.api=web3,debug,eth,txpool,net \
      --disable-discovery \
      --max-outbound-peers=10 \
      --max-inbound-peers=10 \
      --authrpc.addr=0.0.0.0 \
      --authrpc.port=8552 \
      --authrpc.jwtsecret=/jwt.txt \
      --rollup.disable-tx-pool-gossip \
      --rollup.sequencer-http=http://op-${SEQ_TYPE}-seq:8545"

# For flashblocks architecture
if [ "$FLASHBLOCK_ENABLED" = "true" ] && [ "$DISABLE_FLASHBLOCKS" = "0" ]; then
    CMD="$CMD --flashblocks-url=ws://rollup-boost:1111"
fi

exec $CMD
