#!/bin/bash

# Check if there's a fork between two RPC endpoints at a given height
# Usage: check-fork.sh [height]
#   height: block height to check (default: safe height of RPC2)
# Returns: 0 if no fork, 1 if fork detected, 2 if error

HEIGHT=${1:-$(cast bn -r http://localhost:8124 safe 2>/dev/null)}

if [ -z "$HEIGHT" ] || [ "$HEIGHT" = "0" ]; then
    exit 2
fi

HASH1=$(cast block $HEIGHT -r http://localhost:8123 --json 2>/dev/null | jq -r '.hash' 2>/dev/null)
HASH2=$(cast block $HEIGHT -r http://localhost:8124 --json 2>/dev/null | jq -r '.hash' 2>/dev/null)

if [ -z "$HASH1" ] || [ -z "$HASH2" ] || [ "$HASH1" = "null" ] || [ "$HASH2" = "null" ]; then
    exit 2
fi

if [ "$HASH1" = "$HASH2" ]; then
    echo "✅ No fork detected at height $HEIGHT"
    exit 0
else
    echo "❌ Fork detected at height $HEIGHT"
    echo "  RPC1 (8123): $HASH1"
    echo "  RPC2 (8124): $HASH2"
    exit 1
fi
