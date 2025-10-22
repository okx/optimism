#!/bin/bash
# init.sh

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

if [ ! -f .env ]; then
    echo "❌ Error: .env file does not exist"
    echo "Please copy env.example to .env and fill in the correct configuration"
    exit 1
fi

# Load environment variables
source .env

echo "🚀 Initializing X Layer Self-hosted RPC node for $NETWORK_TYPE network..."

mkdir -p data

# Download the genesis file
echo "📥 Downloading genesis file..."
wget -c https://okg-pub-hk.oss-cn-hongkong.aliyuncs.com/cdn/chain/xlayer/snapshot/merged.genesis.json.tar.gz -O merged.genesis.json.tar.gz

# Extract the genesis file
echo "📦 Extracting genesis file..."
tar -xzf merged.genesis.json.tar.gz -C config/
mv config/merged.genesis.json config/genesis.json

# Clean up the downloaded archive
echo "🧹 Cleaning up downloaded archive..."
rm merged.genesis.json.tar.gz

# Check if genesis.json exists
if [ ! -f "config/genesis.json" ]; then
    echo "❌ Error: Failed to extract genesis.json"
    exit 1
fi

echo "✅ Genesis file extracted successfully to config/genesis.json"

# Initialize op-geth with the genesis file
echo "🔧 Initializing op-geth with genesis file... (It may take a while, please wait patiently.)"
docker run --rm \
    -v "$(pwd)/data:/data" \
    -v "$(pwd)/config/genesis.json:/genesis.json" \
    ${OP_GETH_IMAGE_TAG} \
    --datadir /data \
    --gcmode=archive \
    --db.engine=pebble \
    --log.format json \
    init \
    --state.scheme=hash \
    /genesis.json

echo "✅ X Layer RPC node initialization completed!"
echo ""
echo "📁 Generated directories:"
echo "  - data/: Contains op-geth blockchain data"
echo "  - config/: Contains configuration files"
