#!/bin/bash

curl -X POST -H "Content-Type: application/json" --data '{"jsonrpc":"2.0","method":"conductor_resume","params":[],"id":1}' http://localhost:8547

curl -X POST -H 'Content-Type: application/json' --data '{"jsonrpc":"2.0","method":"admin_overrideLeader","params":[],"id":1}' http://localhost:9545
if [ $? -ne 0 ]; then
    echo "Failed to override conductor control"
    exit 1
fi

# 2. Get latest L2 block hash
BLOCK_HASH=$(curl -s -X POST -H 'Content-Type: application/json' --data '{"jsonrpc":"2.0","method":"eth_getBlockByNumber","params":["latest",false],"id":1}'  http://localhost:8123 | jq -r .result.hash)
if [ -z "$BLOCK_HASH" ] || [ "$BLOCK_HASH" = "null" ]; then
    echo "Failed to get latest block hash"
    exit 1
fi
echo "Got latest block hash: $BLOCK_HASH"

# 3. Start sequencer with the block hash
curl -X POST -H "Content-Type: application/json" --data '{"jsonrpc":"2.0","method":"admin_startSequencer","params":["'"$BLOCK_HASH"'"],"id":1}' http://localhost:9545
if [ $? -ne 0 ]; then
    echo "Failed to start sequencer"
    exit 1
fi

# 4. verify sequencer is active
sleep 1
ACTIVE=$(curl -s -X POST -H 'Content-Type: application/json' --data '{"jsonrpc":"2.0","method":"admin_sequencerActive","params":[],"id":1}' http://localhost:9545 | jq -r .result)
if [ "$ACTIVE" != "true" ]; then
    echo "Failed to activate sequencer"
    exit 1
fi

echo "Sequencer successfully activated"

#curl -X POST -H "Content-Type: application/json" --data '{"jsonrpc":"2.0","method":"conductor_addServerAsVoter","params":["conductor-2", "op-conductor2:50050", 0],"id":1}' http://localhost:8547
#curl -X POST -H "Content-Type: application/json" --data '{"jsonrpc":"2.0","method":"conductor_addServerAsVoter","params":["conductor-3", "op-conductor3:50050", 0],"id":1}' http://localhost:8547
