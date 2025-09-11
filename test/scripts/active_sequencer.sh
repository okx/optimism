#!/bin/bash

CONNECTED=$(curl -sS -X POST -H "Content-Type: application/json" --data '{"jsonrpc":"2.0","method":"opp2p_peerStats","params":[],"id":1}' http://localhost:9545 | jq .result.connected)
if (( CONNECTED < 2 )); then
    echo "$CONNECTED peers connected, which is less than 2"
    echo 1
fi

# 2. try to resume conductor if it is paused
PAUSED=$(curl -sS -X POST -H "Content-Type: application/json" --data '{"jsonrpc":"2.0","method":"conductor_paused","params":[],"id":1}' http://localhost:8547 | jq -r .result)
if [ $PAUSED = "true" ]; then
    curl -sS -X POST -H "Content-Type: application/json" --data '{"jsonrpc":"2.0","method":"conductor_resume","params":[],"id":1}' http://localhost:8547
    PAUSED=$(curl -sS -X POST -H "Content-Type: application/json" --data '{"jsonrpc":"2.0","method":"conductor_paused","params":[],"id":1}' http://localhost:8547 | jq -r .result)
    if [ $PAUSED = "true" ]; then
        echo "conductor is paused due to resume failure"
        exit 1
    fi
fi



# 2. try to start sequencer if it is stopped
ACTIVE=$(curl -sS -X POST -H "Content-Type: application/json" --data '{"jsonrpc":"2.0","method":"admin_sequencerActive","params":[],"id":1}' http://localhost:8547 | jq -r .result)
if [ $ACTIVE = "false" ]; then
    BLOCK_HASH=$(curl -sS -X POST -H 'Content-Type: application/json' --data '{"jsonrpc":"2.0","method":"eth_getBlockByNumber","params":["latest",false],"id":1}'  http://localhost:8123 | jq -r .result.hash)
    if [ -z "$BLOCK_HASH" ] || [ "$BLOCK_HASH" = "null" ]; then
        echo "Failed to get latest block hash"
        exit 1
    fi
    echo "Got latest block hash: $BLOCK_HASH"

    # 3. Start sequencer with the block hash
    curl -sS -X POST -H "Content-Type: application/json" --data '{"jsonrpc":"2.0","method":"admin_startSequencer","params":["'"$BLOCK_HASH"'"],"id":1}' http://localhost:9545
    if [ $? -ne 0 ]; then
        echo "Failed to start sequencer"
        exit 1
    fi
fi


# 3. verify sequencer is active
sleep 1
ACTIVE=$(curl -sS -X POST -H 'Content-Type: application/json' --data '{"jsonrpc":"2.0","method":"admin_sequencerActive","params":[],"id":1}' http://localhost:9545 | jq -r .result)
if [ "$ACTIVE" != "true" ]; then
    echo "Failed to activate sequencer"
    exit 1
fi

echo "Sequencer successfully activated"

# 1. try to add other two conductors to raft consensus cluster
SERVER_COUNT=$(curl -sS -X POST -H "Content-Type: application/json" --data '{"jsonrpc":"2.0","method":"conductor_clusterMembership","params":[],"id":1}' http://localhost:8547  | jq '.result.servers | length')
if (( $SERVER_COUNT < 3 )); then
    curl -X POST -H "Content-Type: application/json" --data '{"jsonrpc":"2.0","method":"conductor_addServerAsVoter","params":["conductor-2", "op-conductor2:50050", 0],"id":1}' http://localhost:8547
    curl -X POST -H "Content-Type: application/json" --data '{"jsonrpc":"2.0","method":"conductor_addServerAsVoter","params":["conductor-3", "op-conductor3:50050", 0],"id":1}' http://localhost:8547
    SERVER_COUNT=$(curl -sS -X POST -H "Content-Type: application/json" --data '{"jsonrpc":"2.0","method":"conductor_clusterMembership","params":[],"id":1}' http://localhost:8547  | jq '.result.servers | length')
    if (( $SERVER_COUNT != 3 )); then
        echo "unexpected server count, expected: 3, real: $SERVER_COUNT"
        exit 1
    fi

    echo "add 2 new voters to raft consensus cluster successfully!"
fi
