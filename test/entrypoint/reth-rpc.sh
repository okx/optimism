#!/bin/sh

set -e

L2_RPC_URL_IN_DOCKER="${L2_RPC_URL_IN_DOCKER:-http://op-geth-seq:8545}"

exec op-reth node \
    --datadir=/datadir \
    --chain=/genesis.json \
    --http \
    --http.port=8545 \
    --http.addr=0.0.0.0 \
    --http.api=admin,debug,eth,net,trace,txpool,web3,rpc,reth,ots,flashbots,miner,mev \
    --ws \
    --ws.addr=0.0.0.0 \
    --ws.port=7546 \
    --ws.api=admin,debug,eth,net,trace,txpool,web3,rpc,reth,ots,flashbots,miner,mev \
    --authrpc.addr=0.0.0.0 \
    --authrpc.port=8552 \
    --authrpc.jwtsecret=/jwt.txt \
    --rollup.sequencer-http="$L2_RPC_URL_IN_DOCKER" \
    --with-unused-ports
