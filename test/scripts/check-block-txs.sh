#!/bin/bash

# Script: Check transaction count for each block in specified range
# If transaction count > 1, print block number

START_HEIGHT=8593921
RPC_URL="http://localhost:8123"

# Get latest safe height
echo "Getting latest safe height..."
LATEST_SAFE=$(cast block-number --rpc-url "$RPC_URL" safe 2>/dev/null)

if [ $? -ne 0 ] || [ -z "$LATEST_SAFE" ]; then
    echo "Error: Failed to get safe height"
    exit 1
fi

echo "Start height: $START_HEIGHT"
echo "Latest safe height: $LATEST_SAFE"
echo "Starting block check..."
echo ""

# Check transaction count for each block
current=$START_HEIGHT
count=0

while [ $current -le $LATEST_SAFE ]; do
    # Get transaction count for the block
    tx_count=$(cast block "$current" --rpc-url "$RPC_URL" --json 2>/dev/null | jq -r '.transactions | length' 2>/dev/null)

    if [ -z "$tx_count" ] || [ "$tx_count" = "null" ]; then
        echo "Warning: Failed to get transaction count for block $current, skipping"
        ((current++))
        continue
    fi

    # If transaction count > 1, print block number
    if [ "$tx_count" -gt 1 ]; then
        echo "Block $current: $tx_count transactions"
        ((count++))
    fi

    # Show progress every 1000 blocks
    if [ $((current % 1000)) -eq 0 ]; then
        echo "Progress: $current / $LATEST_SAFE (found $count blocks)"
    fi

    ((current++))
done

echo ""
echo "Check completed!"
echo "Found $count blocks with transaction count > 1"

