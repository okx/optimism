#!/bin/bash

set -e

# Source environment variables
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENV_FILE="$(dirname "$SCRIPT_DIR")/.env"
source "$ENV_FILE"

# Setup P2P static connections between op-geth nodes
echo "🔗 Setting up P2P static connections between op-geth nodes..."

# Function to get enode from a geth container
get_enode() {
    local container_name=$1
    local rpc_port=$2
    local enode=$(docker exec $container_name geth attach --exec "admin.nodeInfo.enode" --datadir /datadir 2>/dev/null | tr -d '"')
    echo "$enode"
}

# Function to replace 127.0.0.1 with container name in enode
replace_enode_ip() {
    local enode=$1
    local container_name=$2
    echo "$enode" | sed "s/@127.0.0.1:/@$container_name:/"
}

# Get enodes for all op-geth containers
echo "📡 Getting enode addresses..."

# Get enodes
OP_GETH_SEQ_ENODE=$(get_enode "op-geth-seq" "8545")
if [ -z "$OP_GETH_SEQ_ENODE" ]; then
    echo "❌ Failed to get enode for op-geth-seq"
    exit 1
fi

if [ "$LAUNCH_RPC_NODE" = "true" ]; then
    OP_GETH_RPC_ENODE=$(get_enode "op-geth-rpc" "8545")
    if [ -z "$OP_GETH_RPC_ENODE" ]; then
        echo "❌ Failed to get enode for op-geth-rpc"
        exit 1
    fi
fi

if [ "$CONDUCTOR_ENABLED" = "true" ]; then
    OP_GETH_SEQ2_ENODE=$(get_enode "op-geth-seq2" "8545")
    if [ -z "$OP_GETH_SEQ2_ENODE" ]; then
        echo "❌ Failed to get enode for op-geth-seq2"
        exit 1
    fi
    OP_GETH_SEQ3_ENODE=$(get_enode "op-geth-seq3" "8545")
    if [ -z "$OP_GETH_SEQ3_ENODE" ]; then
        echo "❌ Failed to get enode for op-geth-seq3"
        exit 1
    fi
fi

# Replace 127.0.0.1 with container names
OP_GETH_SEQ_ENODE=$(replace_enode_ip "$OP_GETH_SEQ_ENODE" "op-geth-seq")

if [ "$LAUNCH_RPC_NODE" = "true" ]; then
    OP_GETH_RPC_ENODE=$(replace_enode_ip "$OP_GETH_RPC_ENODE" "op-geth-rpc")
fi

if [ "$CONDUCTOR_ENABLED" = "true" ]; then
    OP_GETH_SEQ2_ENODE=$(replace_enode_ip "$OP_GETH_SEQ2_ENODE" "op-geth-seq2")
    OP_GETH_SEQ3_ENODE=$(replace_enode_ip "$OP_GETH_SEQ3_ENODE" "op-geth-seq3")
fi

echo "✅ Enode addresses:"
echo "  op-geth-seq: $OP_GETH_SEQ_ENODE"
if [ "$LAUNCH_RPC_NODE" = "true" ]; then
    echo "  op-geth-rpc: $OP_GETH_RPC_ENODE"
fi
if [ "$CONDUCTOR_ENABLED" = "true" ]; then
    echo "  op-geth-seq2: $OP_GETH_SEQ2_ENODE"
    echo "  op-geth-seq3: $OP_GETH_SEQ3_ENODE"
fi

# Function to add peer to a geth container
add_peer() {
    local container_name=$1
    local peer_enode=$2
    echo "🔗 Adding peer to $container_name: $peer_enode"
    docker exec $container_name geth attach --exec "admin.addPeer('$peer_enode')" --datadir /datadir 2>/dev/null
}

# Setup static connections between sequencer nodes
echo "🔗 Setting up static connections between sequencer nodes..."

# Add peers to op-geth-seq (connect to other sequencers)
echo "🔗 Setting up peers for op-geth-seq..."
if [ "$CONDUCTOR_ENABLED" = "true" ]; then
    add_peer "op-geth-seq" "$OP_GETH_SEQ2_ENODE"
    add_peer "op-geth-seq" "$OP_GETH_SEQ3_ENODE"
fi

if [ "$CONDUCTOR_ENABLED" = "true" ]; then
    # Add peers to op-geth-seq2 (connect to other sequencers)
    echo "🔗 Setting up peers for op-geth-seq2..."
    add_peer "op-geth-seq2" "$OP_GETH_SEQ_ENODE"
    add_peer "op-geth-seq2" "$OP_GETH_SEQ3_ENODE"

    # Add peers to op-geth-seq3 (connect to other sequencers)
    echo "🔗 Setting up peers for op-geth-seq3..."
    add_peer "op-geth-seq3" "$OP_GETH_SEQ_ENODE"
    add_peer "op-geth-seq3" "$OP_GETH_SEQ2_ENODE"
fi

# Setup RPC node to connect to all sequencer nodes
if [ "$LAUNCH_RPC_NODE" = "true" ]; then
    echo "🔗 Setting up RPC node to connect to all sequencer nodes..."
    add_peer "op-geth-rpc" "$OP_GETH_SEQ_ENODE"
    if [ "$CONDUCTOR_ENABLED" = "true" ]; then
        add_peer "op-geth-rpc" "$OP_GETH_SEQ2_ENODE"
        add_peer "op-geth-rpc" "$OP_GETH_SEQ3_ENODE"
    fi
fi

echo "✅ P2P static connections established:"
if [ "$CONDUCTOR_ENABLED" = "true" ]; then
  echo "  - Sequencer nodes (op-geth-seq, op-geth-seq2, op-geth-seq3) are connected to each other"
fi
if [ "$LAUNCH_RPC_NODE" = "true" ]; then
    echo "  - RPC node (op-geth-rpc) is connected to all sequencer nodes"
fi
