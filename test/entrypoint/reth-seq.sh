#!/bin/bash

set -e

source /.env

start_op_reth_node() {
    local log_path="$1"

    exec op-reth node \
          --datadir=/datadir \
          --chain=/genesis.json \
          --config=/config.toml \
          --http \
          --http.corsdomain=* \
          --http.port=8545 \
          --http.addr=0.0.0.0 \
          --http.api=web3,debug,eth,txpool,net,miner \
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
          --txpool.pending-max-count=500000 \
          --txpool.pending-max-size=209715200 \
          --txpool.basefee-max-count=500000 \
          --txpool.queued-max-count=500000 \
          --txpool.max-account-slots=10000 \
          --log.file.directory "$log_path" \
          --log.stdout.filter "info,txpool=trace,payload_builder=debug,engine=debug,network=debug" \
          --trusted-peers=enode://ef8135659def07b48b54fe2de7d0368e3eaa0a080ef13dde560169357900954be1a1e890b5973a821f9158e512a2da3ff600368f44e18e725a86931eaae5ef64@op-${SEQ_TYPE}-seq:30303
}

start_op_reth_node "/var/log/op-reth-seq${1:+-$1}"
