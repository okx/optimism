#!/bin/sh


set -e

USE_RETH="${USE_RETH:-false}"
L1_RPC_URL_IN_DOCKER="${L1_RPC_URL_IN_DOCKER:-http://l1-geth:8545}"
L1_BEACON_URL_IN_DOCKER="${L1_BEACON_URL_IN_DOCKER:-http://l1-beacon-chain:3500}"

echo "Starting op-node RPC..."
echo "USE_RETH: $USE_RETH"

if [ "$USE_RETH" = "true" ]; then
    L2_URL="http://op-reth-rpc:8552"
    echo "Connecting to Reth: $L2_URL"
else
    L2_URL="http://op-geth-rpc:8552"
    echo "Connecting to Geth: $L2_URL"
fi

exec /app/op-node/bin/op-node \
    --log.level=debug \
    --l2="$L2_URL" \
    --l2.jwt-secret=/jwt.txt \
    --sequencer.enabled=false \
    --verifier.l1-confs=1 \
    --rollup.config=/rollup.json \
    --rpc.addr=0.0.0.0 \
    --rpc.port=9545 \
    --p2p.listen.tcp=9223 \
    --p2p.listen.udp=9223 \
    --p2p.priv.raw=604557d042fbea9ed42f46c0c95c346a932b6a5ef0c0dd07a00dbf95801a2510 \
    --p2p.peerstore.path=/data/p2p/opnode_peerstore_db \
    --p2p.discovery.path=/data/p2p/opnode_discovery_db \
    --p2p.static=/dns4/op-seq/tcp/9223/p2p/16Uiu2HAkzHdkbmS2VrCsccLibsu7MvGHpmFUMJnMTkKifrtS5m65 \
    --p2p.no-discovery \
    --rpc.enable-admin=true \
    --l1="$L1_RPC_URL_IN_DOCKER" \
    --l1.beacon="$L1_BEACON_URL_IN_DOCKER" \
    --l1.rpckind=standard \
    --safedb.path=/data/safedb
