#!/bin/bash

# Check if the unsafe head reaches the target height, then restart op-rpc
# Usage: restart-op-rpc-on-target-height.sh
# This script will loop continuously until the unsafe head reaches target height, then restart op-rpc

TARGET_UNSAFE_HEIGHT=8594289
PRE=$((TARGET_UNSAFE_HEIGHT - 1000))
RPC_URL="http://localhost:8124"

echo "⏳ Waiting for unsafe head to reach height $TARGET_UNSAFE_HEIGHT..."

echo $PRE

# Loop until condition is met
while true; do
    # Get current unsafe height
    CURRENT_UNSAFE_HEIGHT=$(cast bn -r $RPC_URL 2>/dev/null)
    if [ -z "$CURRENT_UNSAFE_HEIGHT" ] || [ "$CURRENT_UNSAFE_HEIGHT" = "0" ]; then
        echo "   Waiting for RPC to be ready... (current: unavailable)"
        sleep 1
        continue
    fi

    # Check if unsafe height reaches target
    if [ "$CURRENT_UNSAFE_HEIGHT" -ge "$TARGET_UNSAFE_HEIGHT" ]; then
        echo "✅ Unsafe head reached target height: $CURRENT_UNSAFE_HEIGHT"
        docker compose restart op-rpc
        exit 0
    fi

    echo "   Current unsafe height: $CURRENT_UNSAFE_HEIGHT (target: >= $TARGET_UNSAFE_HEIGHT)"

     if [ "$CURRENT_UNSAFE_HEIGHT" -lt "$PRE" ]; then
         sleep 1
     fi
done

