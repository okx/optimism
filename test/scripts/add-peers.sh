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
    local enode=$(docker logs $container_name 2>&1 | head -n 200 | grep "enode" | tail -1 | cut -d '=' -f 2 | tr -d '"')
    echo "$enode"
}

# Function to replace 127.0.0.1 with container name in enode
replace_enode_ip() {
    local enode=$1
    local container_name=$2
    echo "$enode" | sed "s/@127.0.0.1:/@$container_name:/"
}

sed_inplace() {
  if [[ "$OSTYPE" == "darwin"* ]]; then
    sed -i '' "$@"
  else
    sed -i "$@"
  fi
}

# Get enodes for all op-geth containers
echo "📡 Getting enode addresses..."

# Get enodes
OP_GETH_SEQ_ENODE=$(get_enode "op-geth-seq")
if [ -z "$OP_GETH_SEQ_ENODE" ]; then
    echo "❌ Failed to get enode for op-geth-seq"
    exit 1
fi

if [ "$CONDUCTOR_ENABLED" = "true" ]; then
    OP_GETH_SEQ2_ENODE=$(get_enode "op-geth-seq2")
    if [ -z "$OP_GETH_SEQ2_ENODE" ]; then
        echo "❌ Failed to get enode for op-geth-seq2"
        exit 1
    fi
    OP_GETH_SEQ3_ENODE=$(get_enode "op-geth-seq3")
    if [ -z "$OP_GETH_SEQ3_ENODE" ]; then
        echo "❌ Failed to get enode for op-geth-seq3"
        exit 1
    fi
fi

# Replace 127.0.0.1 with container names
OP_GETH_SEQ_ENODE=$(replace_enode_ip "$OP_GETH_SEQ_ENODE" "op-geth-seq")

if [ "$CONDUCTOR_ENABLED" = "true" ]; then
    OP_GETH_SEQ2_ENODE=$(replace_enode_ip "$OP_GETH_SEQ2_ENODE" "op-geth-seq2")
    OP_GETH_SEQ3_ENODE=$(replace_enode_ip "$OP_GETH_SEQ3_ENODE" "op-geth-seq3")
fi

echo "✅ Enode addresses:"
echo "  op-geth-seq: $OP_GETH_SEQ_ENODE"
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
    OP_RPC_TRUSTED_NODES="\"$OP_GETH_SEQ_ENODE\""
    if [ "$CONDUCTOR_ENABLED" = "true" ]; then
        OP_RPC_TRUSTED_NODES="\"$OP_GETH_SEQ_ENODE\",\"$OP_GETH_SEQ2_ENODE\",\"$OP_GETH_SEQ3_ENODE\""
    fi
    if [ "$RPC_TYPE" = "op-geth-rpc" ]; then
        cp ./config-op/test.geth.rpc.config.toml ./config-op/gen.test.geth.rpc.config.toml
        cp ./config-op/test.geth.rpc.rt.config.toml ./config-op/gen.test.geth.rpc.rt.config.toml
        # Here we use # as delimiter to avoid escaping // in enode URLs
        sed_inplace 's#TrustedNodes = \[\]#TrustedNodes = \['"$OP_RPC_TRUSTED_NODES"'\]#' ./config-op/gen.test.geth.rpc.config.toml
        sed_inplace 's#TrustedNodes = \[\]#TrustedNodes = \['"$OP_RPC_TRUSTED_NODES"'\]#' ./config-op/gen.test.geth.rpc.rt.config.toml
    elif [ "$RPC_TYPE" = "op-reth-rpc" ]; then
        cp ./config-op/test.reth.rpc.config.toml ./config-op/gen.test.reth.rpc.config.toml
        # Here we use # as delimiter to avoid escaping // in enode URLs
        sed_inplace 's#trusted_nodes = \[\]#trusted_nodes = \['"$OP_RPC_TRUSTED_NODES"'\]#' ./config-op/gen.test.reth.rpc.config.toml
    fi
fi

echo "✅ P2P static connections established:"
if [ "$CONDUCTOR_ENABLED" = "true" ]; then
  echo "  - Sequencer nodes (op-geth-seq, op-geth-seq2, op-geth-seq3) are connected to each other"
fi
if [ "$LAUNCH_RPC_NODE" = "true" ]; then
    echo "  - RPC node ($RPC_TYPE) is connected to all sequencer nodes"
fi
