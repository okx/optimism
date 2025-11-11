#!/bin/bash

# Check if there's a fork between two RPC endpoints
# Usage: check-fork.sh [rpc1_url] [rpc2_url]
#   rpc1_url: first RPC endpoint (default: http://localhost:8123)
#   rpc2_url: second RPC endpoint (default: http://localhost:8124)
# Returns: 0 if no fork, 1 if fork detected, 2 if error

RPC1=${1:-http://localhost:8123}
RPC2=${2:-http://localhost:8124}

# Get latest block heights from both RPCs
HEIGHT1=$(cast bn -r $RPC1 2>/dev/null)
HEIGHT2=$(cast bn -r $RPC2 2>/dev/null)

if [ -z "$HEIGHT1" ] || [ "$HEIGHT1" = "0" ] || [ -z "$HEIGHT2" ] || [ "$HEIGHT2" = "0" ]; then
    exit 2
fi

# Use the smaller height
if [ "$HEIGHT1" -le "$HEIGHT2" ]; then
    HEIGHT=$HEIGHT1
else
    HEIGHT=$HEIGHT2
fi

# Get block hashes at the common height
HASH1=$(cast block $HEIGHT -r $RPC1 --json 2>/dev/null | jq -r '.hash' 2>/dev/null)
HASH2=$(cast block $HEIGHT -r $RPC2 --json 2>/dev/null | jq -r '.hash' 2>/dev/null)

if [ -z "$HASH1" ] || [ -z "$HASH2" ] || [ "$HASH1" = "null" ] || [ "$HASH2" = "null" ]; then
    exit 2
fi

if [ "$HASH1" = "$HASH2" ]; then
    echo "✅ No fork detected at height $HEIGHT"
    exit 0
else
    echo "❌ Fork detected at height $HEIGHT"
    echo "  RPC1 ($RPC1): $HASH1"
    echo "  RPC2 ($RPC2): $HASH2"
    exit 1
fi
