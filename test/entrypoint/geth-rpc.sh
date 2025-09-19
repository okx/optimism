#!/bin/sh

set -e

DB_ENGINE="${DB_ENGINE:-pebble}"
L2_RPC_URL_IN_DOCKER="${L2_RPC_URL_IN_DOCKER:-http://op-geth-seq:8545}"

# 启动geth
exec geth \
    --datadir=/datadir \
    --db.engine="$DB_ENGINE" \
    --http \
    --http.corsdomain=* \
    --http.vhosts=* \
    --http.port=8545 \
    --http.addr=0.0.0.0 \
    --http.api=web3,debug,eth,txpool,net,engine,miner \
    --ws \
    --ws.addr=0.0.0.0 \
    --ws.port=7546 \
    --ws.origins=* \
    --ws.api=debug,eth,txpool,net,engine \
    --syncmode=full \
    --gcmode=archive \
    --nodiscover \
    --maxpeers=0 \
    --networkid=901 \
    --authrpc.vhosts=* \
    --authrpc.addr=0.0.0.0 \
    --authrpc.port=8552 \
    --authrpc.jwtsecret=/jwt.txt \
    --rollup.sequencerhttp="$L2_RPC_URL_IN_DOCKER" \
    --rollup.enabletxpooladmission
