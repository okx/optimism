#!/bin/bash

# Script: Continuously send transactions and record hashes to hash.txt

HASH_FILE="hash.txt"

rm -rf "$HASH_FILE"

# Ensure hash.txt file exists
touch "$HASH_FILE"

echo "Starting to send transactions and record hashes..."
echo "Press Ctrl+C to stop"

while true; do
    # Send transaction and get hash
    HASH=$(cast send 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266 \
        --private-key=0x4bbbf85ce3377467afe5d46f804f221813b2bb87f24d81f60f1fcdbf7cbf4356 \
        --value=1 \
        -r http://localhost:8123 \
        --json 2>/dev/null | jq -r .transactionHash)

    # Check if hash was successfully obtained
    if [ -n "$HASH" ] && [ "$HASH" != "null" ]; then
        echo "$HASH" >> "$HASH_FILE"
        echo "Recorded hash: $HASH"
    else
        echo "Error: Failed to get transaction hash"
    fi

    # Optional: Add short delay to avoid sending too fast
    sleep 1
done

