#!/bin/bash
# scripts/start.sh

set -e

echo "🚀 Starting X Layer Self-hosted RPC node..."

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
