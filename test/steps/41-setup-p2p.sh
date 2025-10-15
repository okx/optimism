#!/bin/bash
# =============================================================================
# Step41: Setup P2P network
# Function: Setup static P2P connections between op-geth nodes
# =============================================================================
set -e
set -x

# Change to test directory (parent of steps/)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$(dirname "$SCRIPT_DIR")"

source .env
source tools.sh

echo "=========================================="
echo "Step41: Setup P2P network"
echo "=========================================="

PWD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PWD_DIR=$(dirname "$PWD_DIR")
cd $PWD_DIR

# Define helper functions
get_enode() {
    local container_name=$1
    local enode=$(docker exec $container_name geth attach \
        --exec "admin.nodeInfo.enode" \
        --datadir /datadir 2>/dev/null | tr -d '"')
    echo "$enode"
}

replace_enode_ip() {
    local enode=$1
    local container_name=$2
    echo "$enode" | sed "s/@127.0.0.1:/@$container_name:/"
}

add_peer() {
    local container_name=$1
    local peer_enode=$2
    echo "   Adding peer to $container_name"
    docker exec $container_name geth attach \
        --exec "admin.addPeer('$peer_enode')" \
        --datadir /datadir 2>/dev/null || true
}

echo " Settingop-geth P2PConnecting..."
echo ""

# Getallnodeenode
echo " Getenodeaddress..."

OP_GETH_SEQ_ENODE=$(get_enode "op-geth-seq")
OP_GETH_RPC_ENODE=$(get_enode "op-geth-rpc")

if [ "$CONDUCTOR_ENABLED" = "true" ]; then
    OP_GETH_SEQ2_ENODE=$(get_enode "op-geth-seq2")
    OP_GETH_SEQ3_ENODE=$(get_enode "op-geth-seq3")
fi

# IP
OP_GETH_SEQ_ENODE=$(replace_enode_ip "$OP_GETH_SEQ_ENODE" "op-geth-seq")
OP_GETH_RPC_ENODE=$(replace_enode_ip "$OP_GETH_RPC_ENODE" "op-geth-rpc")

if [ "$CONDUCTOR_ENABLED" = "true" ]; then
    OP_GETH_SEQ2_ENODE=$(replace_enode_ip "$OP_GETH_SEQ2_ENODE" "op-geth-seq2")
    OP_GETH_SEQ3_ENODE=$(replace_enode_ip "$OP_GETH_SEQ3_ENODE" "op-geth-seq3")
fi

echo "SUCCESS: Enodeaddress:"
echo "   op-geth-seq: ${OP_GETH_SEQ_ENODE:0:60}..."
echo "   op-geth-rpc: ${OP_GETH_RPC_ENODE:0:60}..."

if [ "$CONDUCTOR_ENABLED" = "true" ]; then
    echo "   op-geth-seq2: ${OP_GETH_SEQ2_ENODE:0:60}..."
    echo "   op-geth-seq3: ${OP_GETH_SEQ3_ENODE:0:60}..."
fi

echo ""

# SettingsequencerConnecting
if [ "$CONDUCTOR_ENABLED" = "true" ]; then
    echo " Connectingsequencernode..."

    # op-geth-seqConnectingsequencers
    add_peer "op-geth-seq" "$OP_GETH_SEQ2_ENODE"
    add_peer "op-geth-seq" "$OP_GETH_SEQ3_ENODE"

    # op-geth-seq2Connectingsequencers
    add_peer "op-geth-seq2" "$OP_GETH_SEQ_ENODE"
    add_peer "op-geth-seq2" "$OP_GETH_SEQ3_ENODE"

    # op-geth-seq3Connectingsequencers
    add_peer "op-geth-seq3" "$OP_GETH_SEQ_ENODE"
    add_peer "op-geth-seq3" "$OP_GETH_SEQ2_ENODE"

    echo ""
fi

# RPCnodeConnectingallsequencers
echo " ConnectingRPCnodesequencer..."
add_peer "op-geth-rpc" "$OP_GETH_SEQ_ENODE"

if [ "$CONDUCTOR_ENABLED" = "true" ]; then
    add_peer "op-geth-rpc" "$OP_GETH_SEQ2_ENODE"
    add_peer "op-geth-rpc" "$OP_GETH_SEQ3_ENODE"
fi

echo ""
echo "SUCCESS: Step41completed: P2PnetworkSetting"
echo "   Sequencernode"
echo "   RPCnodeConnectingallsequencer"

