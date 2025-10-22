#!/bin/bash
# init.sh

set -e

echo "🚀 Initializing X Layer Self-hosted RPC node..."

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
    xlayer/op-geth:dev \
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
echo ""
echo "🚀 Next steps:"
echo "  1. Copy scripts/env.example to .env and configure your settings"
echo "  2. Run ./start.sh to start the RPC node"
