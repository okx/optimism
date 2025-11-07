#!/bin/bash

# Find the fork point between op-rpc and op-seq
# Using binary search

RPC1="http://localhost:8123"
RPC2="http://localhost:8124"
START_HEIGHT=8593921

# Get safe height from both nodes
SAFE1=$(cast bn -r $RPC1 safe)
SAFE2=$(cast bn -r $RPC2 safe)
MIN_SAFE=$((SAFE1 < SAFE2 ? SAFE1 : SAFE2))

echo "=== Finding Fork Point ==="
echo "RPC1 safe height: $SAFE1"
echo "RPC2 safe height: $SAFE2"
echo "Min safe height: $MIN_SAFE"
echo "Start height: $START_HEIGHT"
echo ""

# Check if there's a fork at safe height
echo "Checking for fork at safe height..."
# Compare safe block hashes at min safe height (the lowest safe height both nodes have reached)
MIN_SAFE_HASH1=$(cast block $MIN_SAFE -r $RPC1 --json 2>/dev/null | jq -r '.hash' 2>/dev/null)
MIN_SAFE_HASH2=$(cast block $MIN_SAFE -r $RPC2 --json 2>/dev/null | jq -r '.hash' 2>/dev/null)

if [ -z "$MIN_SAFE_HASH1" ] || [ -z "$MIN_SAFE_HASH2" ]; then
    echo "  Error: Failed to get min safe block hash"
    exit 1
fi

if [ "$MIN_SAFE_HASH1" = "$MIN_SAFE_HASH2" ]; then
    echo "  ✅ No fork at safe height, both nodes have same safe block hash"
    echo "  Safe block hash at height $MIN_SAFE: $MIN_SAFE_HASH1"
    echo ""
    echo "No fork detected at safe height. Exiting."
    exit 0
else
    echo "  ❌ Fork detected at safe height!"
    echo "  RPC1 safe block hash at height $MIN_SAFE: $MIN_SAFE_HASH1"
    echo "  RPC2 safe block hash at height $MIN_SAFE: $MIN_SAFE_HASH2"
    echo ""
    echo "Proceeding with binary search to find fork point..."
    echo ""
fi

# Binary search
left=$START_HEIGHT
right=$MIN_SAFE
fork_height=$START_HEIGHT

while [ $left -le $right ]; do
    mid=$(((left + right) / 2))

    echo "Checking block $mid..."

    # Get block hash from both nodes
    hash1=$(cast block $mid -r $RPC1 --json 2>/dev/null | jq -r '.hash' 2>/dev/null)
    hash2=$(cast block $mid -r $RPC2 --json 2>/dev/null | jq -r '.hash' 2>/dev/null)

    if [ -z "$hash1" ] || [ -z "$hash2" ]; then
        echo "  Error: Failed to get hash for block $mid"
        break
    fi

    if [ "$hash1" = "$hash2" ]; then
        echo "  Match: $hash1"
        # Match, fork point is after this
        fork_height=$mid
        left=$((mid + 1))
    else
        echo "  Mismatch!"
        echo "  RPC1: $hash1"
        echo "  RPC2: $hash2"
        # Mismatch, fork point is at or before this
        right=$((mid - 1))
    fi
done

echo ""
echo "=== Fork Point Result ==="
echo "Last matching block: $fork_height"
echo "Fork height: $((fork_height + 1))"

# Verify fork point
echo ""
echo "=== Verifying Fork Point ==="
echo "Block $fork_height (last matching):"
hash1=$(cast block $fork_height -r $RPC1 --json 2>/dev/null | jq -r '.hash')
hash2=$(cast block $fork_height -r $RPC2 --json 2>/dev/null | jq -r '.hash')
echo "  RPC1: $hash1"
echo "  RPC2: $hash2"
if [ "$hash1" = "$hash2" ]; then
    echo "  ✅ Match"
else
    echo "  ❌ Mismatch"
fi

fork_block=$((fork_height + 1))
if [ $fork_block -le $MIN_SAFE ]; then
    echo ""
    echo "Block $fork_block (first fork):"
    hash1=$(cast block $fork_block -r $RPC1 --json 2>/dev/null | jq -r '.hash')
    hash2=$(cast block $fork_block -r $RPC2 --json 2>/dev/null | jq -r '.hash')
    echo "  RPC1: $hash1"
    echo "  RPC2: $hash2"
    if [ "$hash1" = "$hash2" ]; then
        echo "  ✅ Still matching"
    else
        echo "  ❌ Fork confirmed"
    fi
fi

