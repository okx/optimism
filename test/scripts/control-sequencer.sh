#!/bin/bash

# Script: Control op-node sequencer start and stop
# Usage: ./control_sequencer.sh [stop|start|status] [RPC_URL] [BLOCK_HASH]

OP_NODE_RPC=${2:-"http://localhost:9545"}
ACTION=${1:-"status"}

# Check if cast is installed
if ! command -v cast &> /dev/null; then
    echo "Error: cast (Foundry) is required"
    echo "Installation: https://book.getfoundry.sh/getting-started/installation"
    exit 1
fi

echo "=== Control op-node Sequencer ==="
echo "OP-Node RPC: $OP_NODE_RPC"
echo ""

case "$ACTION" in
    stop)
        echo "Stopping sequencer..."
        result=$(cast rpc admin_stopSequencer --rpc-url "$OP_NODE_RPC" 2>/dev/null)

        if [ $? -eq 0 ] && [ -n "$result" ]; then
            echo "Sequencer stopped"
            echo "Latest block hash: $result"
        else
            echo "Error: Failed to stop sequencer or sequencer is already stopped"
            exit 1
        fi
        ;;

    start)
        # Check current status first
        echo "Checking sequencer current status..."
        is_active=$(cast rpc admin_sequencerActive --rpc-url "$OP_NODE_RPC" 2>/dev/null)

        if [ $? -ne 0 ]; then
            echo "Error: Failed to connect to RPC"
            exit 1
        fi

        if [ "$is_active" = "true" ]; then
            echo "Sequencer is already running, no need to start"
            exit 0
        fi

        echo "Sequencer is currently stopped, preparing to start..."

        sync_status=$(cast rpc optimism_syncStatus --rpc-url "$OP_NODE_RPC" 2>/dev/null)
        if [ $? -eq 0 ]; then
            BLOCK_HASH=$(echo "$sync_status" | jq -r '.unsafe_l2.hash' 2>/dev/null)
            if [ -n "$BLOCK_HASH" ] && [ "$BLOCK_HASH" != "null" ]; then
                echo "Got unsafe_l2 hash from syncStatus: $BLOCK_HASH"
            fi
        fi

        echo "Using block hash: $BLOCK_HASH"
        echo "Starting sequencer..."

        result=$(cast rpc admin_startSequencer "$BLOCK_HASH" --rpc-url "$OP_NODE_RPC" 2>&1)

        if [ $? -eq 0 ]; then
            echo "Sequencer started successfully"
        else
            echo "Error: Failed to start sequencer"
            echo "$result"
            echo ""
        fi
        ;;

    status)
        echo "Querying sequencer status..."
        is_active=$(cast rpc admin_sequencerActive --rpc-url "$OP_NODE_RPC" 2>/dev/null)

        if [ $? -eq 0 ]; then
            if [ "$is_active" = "true" ]; then
                echo "Sequencer status: Running"
            else
                echo "Sequencer status: Stopped"
            fi
        else
            echo "Error: Failed to connect to RPC"
            exit 1
        fi
        ;;

    *)
        echo "Usage: $0 [stop|start|status] [OP_NODE_RPC] [BLOCK_HASH] [L2_RPC]"
        echo ""
        echo "Examples:"
        echo "  $0 stop                           # Stop sequencer"
        echo "  $0 start                          # Start sequencer (auto-get latest block hash)"
        echo "  $0 start <block_hash>             # Start sequencer (use specified block hash)"
        echo "  $0 start <block_hash> <l2_rpc>   # Start sequencer (specify L2 RPC URL for getting block hash)"
        echo "  $0 status                         # Query sequencer status"
        echo ""
        echo "Default OP-Node RPC URL: http://localhost:9545"
        echo "Default L2 RPC URL: http://localhost:8545"
        exit 1
        ;;
esac

