set -e
set -x
source .env
source utils.sh
source tools.sh

PWD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$PWD_DIR")"
SCRIPTS_DIR=$ROOT_DIR/test/scripts

start_sequencer() {
  rm -rf "$OP_GETH_DATADIR2"
  cp -r "$OP_GETH_DATADIR" "$OP_GETH_DATADIR2"

  rm -rf "$OP_GETH_DATADIR3"
  cp -r "$OP_GETH_DATADIR" "$OP_GETH_DATADIR3"

  if [ "$ENV" = "testnet" ];then
    L1_RPC_URL="https://fullnode-inner.okg.com/sepolia/fork/okbc/rpc"
    L1_BEACON_URL_IN_DOCKER="https://fullnode-inner.okg.com/ethsepoliabeacon/native/layer1/rpc"
    sed_inplace "s|L1_RPC_URL=.*|L1_RPC_URL=$L1_RPC_URL|" .env
    sed_inplace "s|L1_RPC_URL_IN_DOCKER=.*|L1_RPC_URL_IN_DOCKER=$L1_RPC_URL|" .env
    sed_inplace "s|L1_BEACON_URL_IN_DOCKER=.*|L1_BEACON_URL_IN_DOCKER=$L1_BEACON_URL_IN_DOCKER|" .env
  fi

  if [ "$CONDUCTOR_ENABLED" = "true" ]; then
      ${DOCKER_COMPOSE_CMD} up -d op-conductor
      ${DOCKER_COMPOSE_CMD} up -d op-conductor2
      ${DOCKER_COMPOSE_CMD} up -d op-conductor3
      sleep 3
      $SCRIPTS_DIR/active-sequencer.sh
  else
      ${DOCKER_COMPOSE_CMD} up -d op-seq
  fi

  # Check for L2 genesis hash mismatch
  LOG_OUTPUT=$(${DOCKER_COMPOSE_CMD} logs op-seq 2>&1 | tail -20)
  if echo "$LOG_OUTPUT" | grep -q "expected L2 genesis hash to match L2 block at genesis block number"; then
      echo "❌ L2 genesis hash mismatch detected!"
      echo "Error details:"
      echo "$LOG_OUTPUT" | grep "expected L2 genesis hash to match L2 block at genesis block number"
      exit 1
  fi
}

start_rpc() {
  rm -rf "$OP_GETH_RPC_DATADIR"
  cp -r "$OP_GETH_DATADIR" "$OP_GETH_RPC_DATADIR"
  ${DOCKER_COMPOSE_CMD} up -d op-rpc
}

connect_static_peers() {

  # Setup P2P static connections between op-geth nodes
  echo "🔗 Setting up P2P static connections between op-geth nodes..."

  # Get enodes for all op-geth containers
  echo "📡 Getting enode addresses..."

  # Get enodes
  OP_GETH_SEQ_ENODE=$(get_enode "op-geth-seq" "8545")
  OP_GETH_RPC_ENODE=$(get_enode "op-geth-rpc" "8545")

  if [ "$CONDUCTOR_ENABLED" = "true" ]; then
      OP_GETH_SEQ2_ENODE=$(get_enode "op-geth-seq2" "8545")
      OP_GETH_SEQ3_ENODE=$(get_enode "op-geth-seq3" "8545")
  fi

  # Replace 127.0.0.1 with container names
  OP_GETH_SEQ_ENODE=$(replace_enode_ip "$OP_GETH_SEQ_ENODE" "op-geth-seq")
  OP_GETH_RPC_ENODE=$(replace_enode_ip "$OP_GETH_RPC_ENODE" "op-geth-rpc")

  if [ "$CONDUCTOR_ENABLED" = "true" ]; then
      OP_GETH_SEQ2_ENODE=$(replace_enode_ip "$OP_GETH_SEQ2_ENODE" "op-geth-seq2")
      OP_GETH_SEQ3_ENODE=$(replace_enode_ip "$OP_GETH_SEQ3_ENODE" "op-geth-seq3")
  fi

  echo "✅ Enode addresses:"
  echo "  op-geth-seq: $OP_GETH_SEQ_ENODE"
  echo "  op-geth-rpc: $OP_GETH_RPC_ENODE"
  if [ "$CONDUCTOR_ENABLED" = "true" ]; then
      echo "  op-geth-seq2: $OP_GETH_SEQ2_ENODE"
      echo "  op-geth-seq3: $OP_GETH_SEQ3_ENODE"
  fi


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
  echo "🔗 Setting up RPC node to connect to all sequencer nodes..."
  add_peer "op-geth-rpc" "$OP_GETH_SEQ_ENODE"
  if [ "$CONDUCTOR_ENABLED" = "true" ]; then
      add_peer "op-geth-rpc" "$OP_GETH_SEQ2_ENODE"
      add_peer "op-geth-rpc" "$OP_GETH_SEQ3_ENODE"
  fi

  echo "✅ P2P static connections established:"
  echo "  - Sequencer nodes (op-geth-seq, op-geth-seq2, op-geth-seq3) are connected to each other"
  echo "  - RPC node (op-geth-rpc) is connected to all sequencer nodes"

}

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

# Function to add peer to a geth container
add_peer() {
    local container_name=$1
    local peer_enode=$2
    echo "🔗 Adding peer to $container_name: $peer_enode"
    docker exec $container_name geth attach --exec "admin.addPeer('$peer_enode')" --datadir /datadir 2>/dev/null
}

start_batcher() {
  # Configure op-batcher endpoints based on conductor mode
  if [ "$CONDUCTOR_ENABLED" = "true" ]; then
      echo "🔧 Configuring op-batcher for conductor mode with conductor RPC endpoints..."
      # Set conductor mode endpoints
      export OP_BATCHER_L2_ETH_RPC="http://op-conductor:8547,http://op-conductor2:8547,http://op-conductor3:8547"
      export OP_BATCHER_ROLLUP_RPC="http://op-conductor:8547,http://op-conductor2:8547,http://op-conductor3:8547"
      echo "✅ op-batcher configured for conductor mode (connecting to conductor RPC endpoints)"
  else
      echo "🔧 Configuring op-batcher for single sequencer mode..."
      # Set single sequencer mode endpoints
      export OP_BATCHER_L2_ETH_RPC="http://op-geth-seq:8545"
      export OP_BATCHER_ROLLUP_RPC="http://op-seq:9545"
      echo "✅ op-batcher configured for single sequencer mode"
  fi

  ${DOCKER_COMPOSE_CMD} up -d op-batcher
}

start_sequencer
start_rpc
connect_static_peers
start_batcher
