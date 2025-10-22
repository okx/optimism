#!/bin/bash
# scripts/start.sh

set -e

# Parse command line arguments
NETWORK_TYPE=${1:-"testnet"}

# Validate network type
if [ "$NETWORK_TYPE" != "testnet" ] && [ "$NETWORK_TYPE" != "mainnet" ]; then
    echo "❌ Error: Invalid network type. Please use 'testnet' or 'mainnet'"
    echo "Usage: $0 [testnet|mainnet]"
    exit 1
fi

# Check if mainnet is supported
if [ "$NETWORK_TYPE" = "mainnet" ]; then
    echo "❌ Error: Mainnet is not currently supported"
    echo "Please use 'testnet' for now. Mainnet support will be available in future releases."
    exit 1
fi

echo "🚀 Starting X Layer Self-hosted RPC node for $NETWORK_TYPE network..."

# Check environment variables file
if [ ! -f .env ]; then
    echo "❌ Error: .env file does not exist"
    echo "Please copy env.example to .env and fill in the correct configuration"
    exit 1
fi

# Load environment variables
source .env

# Check required environment variables
required_vars=("L1_RPC_URL" "OP_NODE_BOOTNODE" "OP_GETH_BOOTNODE")
for var in "${required_vars[@]}"; do
    if [ -z "${!var}" ]; then
        echo "❌ Error: Environment variable $var is not set"
        exit 1
    fi
done

# Create necessary directories
echo "📁 Creating data directories..."
mkdir -p data/op-node/p2p
mkdir -p config

# Check configuration files
echo "🔍 Checking configuration files..."
config_files=("config/rollup.json" "config/genesis.json")
for file in "${config_files[@]}"; do
    if [ ! -f "$file" ]; then
        echo "❌ Error: Configuration file $file does not exist"
        echo "Please place X Layer configuration files in the config/ directory"
        exit 1
    fi
done

# Generate JWT secret (if it does not exist)
if [ ! -s config/jwt.txt ]; then
    echo "🔑 Generating JWT secret..."
    openssl rand -hex 32 > config/jwt.txt
fi

# Start services
echo "🐳 Starting Docker services..."
docker compose up -d

# Wait for services to start
echo "⏳ Waiting for services to start..."
sleep 10

# Check service status
echo "🔍 Checking service status..."
docker compose ps

echo "✅ X Layer RPC node startup completed!"

echo ""
echo "📋 Service Information:"
echo "======================"
echo ""
echo "🔍 View service logs:"
echo "  docker logs -f xlayer-op-node"
echo "  docker logs -f xlayer-op-geth"
echo ""
echo "🌐 Exposed Ports:"
echo "| Service | Port | Protocol | Purpose |"
echo "|---------|------|----------|---------|"
echo "| op-geth RPC | 8123 | HTTP | JSON-RPC API |"
echo "| op-geth WebSocket | 8546 | WebSocket | WebSocket API |"
echo "| op-node RPC | 9545 | HTTP | Consensus layer API |"
echo "| op-geth P2P | 30303 | TCP/UDP | P2P network |"
echo "| op-node P2P | 9223 | TCP/UDP | P2P network |"
echo ""
echo "🛑 Stop services:"
echo "  ./stop.sh"
echo ""
echo "🔍 Check if blocks are syncing:"
echo "  curl http://127.0.0.1:8123 \\"
echo "    -X POST \\"
echo "    -H \"Content-Type: application/json\" \\"
echo "    --data '{\"method\":\"eth_blockNumber\",\"params\":[],\"id\":1,\"jsonrpc\":\"2.0\"}'"
echo ""
