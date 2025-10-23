#!/bin/bash
set -e
set -x
source .env

BRIDGE_VALUE_BIG="1000000000000000000"  # 1 ETH in wei
BRIDGE_VALUE_SMALL="100000000000000000"  # 0.1 ETH in wei

# Get Global Exit Root Manager address
GER_MGR=$(cast call "$BRIDGE_ADDRESS" "globalExitRootManager()(address)" --rpc-url "$L1_RPC_URL")
L2GER_MGR1=$(cast call "$BRIDGE_ADDRESS" "globalExitRootManager()(address)" --rpc-url "$L2_SEQ_URL")
echo "GlobalExitRootManager Address:"
echo "  L1   ==> $GER_MGR"
echo "  L2   ==> $L2GER_MGR1"

# Get initial Global Exit Root
GER=$(cast call "$GER_MGR" "getLastGlobalExitRoot()" --rpc-url "$L1_RPC_URL")
echo "Initial GER on L1: $GER"

# =============================================================================
# Bridge from L1 to L2
# =============================================================================

# Check balance before bridging
L2_BALANCE_BEFORE_BRIDGE=$(cast call "$L2_WETH" "balanceOf(address)(uint256)" "$DEPLOYER_ADDRESS" --rpc-url "$L2_SEQ_URL" | awk '{print $1}')

cast send \
    --legacy \
    --rpc-url $L1_RPC_URL \
    --private-key $DEPLOYER_PRIVATE_KEY \
    --value $BRIDGE_VALUE_BIG \
    $BRIDGE_ADDRESS \
    'function bridgeAsset(uint32 destinationNetwork, address destinationAddress, uint256 amount, address token, bool forceUpdateGlobalExitRoot, bytes permitData) returns()' \
    1 $DEPLOYER_ADDRESS $BRIDGE_VALUE_BIG $L1_ETH_ADDRESS true 0x

# Wait for GER update on L1
echo "Waiting for GER to be updated on L1..."
while true; do
    GER_NEW=$(cast call "$GER_MGR" "getLastGlobalExitRoot()" --rpc-url "$L1_RPC_URL")
    if [ "$GER_NEW" != "$GER" ]; then
        GER=$GER_NEW
        echo "GER updated to $GER on L1"
        break
    fi
    sleep 10
done

# Wait for GER to sync to L2
echo "Waiting for GER to sync to L2..."
start_time=$(date +%s)
while true; do
    timestamp=$(cast call "$L2GER_MGR1" "globalExitRootMap(bytes32)(uint256)" "$GER" --rpc-url "$L2_SEQ_URL")
    if [ "$timestamp" != "0" ]; then
        break
    fi
    sleep 15
done
end_time=$(date +%s)
total_elapsed=$((end_time - start_time))
echo "GER synced to L2, took $total_elapsed seconds"

# Wait for assets to be claimed automatically by sponsor
echo "Waiting for assets to be claimed by sponsor..."
start_time=$(date +%s)
while true; do
    sleep 60
    balance=$(cast call "$L2_WETH" "balanceOf(address)(uint256)" "$DEPLOYER_ADDRESS" --rpc-url "$L2_SEQ_URL" | awk '{print $1}')
    balance=${balance:-0}
    increment=$(echo "$balance - $L2_BALANCE_BEFORE_BRIDGE" | bc)
    echo "Current balance on L2: $balance (increment: $increment)"
    if [ "$increment" -gt 0 ]; then
        echo "Assets successfully claimed on L2"
        break
    fi
done
end_time=$(date +%s)
total_elapsed=$((end_time - start_time))
echo "Balance on L2 is $balance, claim took $total_elapsed seconds"

# Check balance after bridging
L2_BALANCE_AFTER_BRIDGE=$(cast balance "$DEPLOYER_ADDRESS" --rpc-url "$L2_SEQ_URL")
echo "Balance on L2(ETH):"
echo "  Before bridge = $L2_BALANCE_BEFORE_BRIDGE"
echo "  After bridge  = $L2_BALANCE_AFTER_BRIDGE"

# =============================================================================
# Bridge from L2 to L1
# =============================================================================
echo -e "\n========== Bridging Assets: L2 -> L1 BRIDGE_VALUE_SMALL =========="

# Approve token
echo "Approving token..."
cast send \
  --legacy \
  --rpc-url $L2_SEQ_URL \
  --private-key $DEPLOYER_PRIVATE_KEY \
  "$L2_WETH" \
  "function approve(address spender, uint256 amount) returns (bool)" \
  $BRIDGE_ADDRESS \
  $BRIDGE_VALUE_BIG

# Initiate bridging transaction
TX_HASH=$(cast send \
    --legacy \
    --private-key $DEPLOYER_PRIVATE_KEY \
    --rpc-url $L2_SEQ_URL \
    --json \
    $BRIDGE_ADDRESS \
    'function bridgeAsset(uint32 destinationNetwork, address destinationAddress, uint256 amount, address token, bool forceUpdateGlobalExitRoot, bytes permitData) returns()' \
    0 $DEPLOYER_ADDRESS $BRIDGE_VALUE_SMALL $L2_WETH true "0x" \
    | jq -r ' .transactionHash')
echo "Bridge transaction hash: $TX_HASH"

# Wait for GER update on L1
echo "Waiting for GER to be updated on L1..."
start_time=$(date +%s)
while true; do
    GER_NEW=$(cast call "$GER_MGR" "getLastGlobalExitRoot()" --rpc-url "$L1_RPC_URL")
    if [ "$GER_NEW" != "$GER" ]; then
        GER=$GER_NEW
        break
    fi
    sleep 10
done
end_time=$(date +%s)
total_elapsed=$((end_time - start_time))
echo "GER updated to $GER on L1, took $total_elapsed seconds"

sleep 40
echo "Getting deposit count and network ID from bridge service..."
result=$(curl -s "$BRIDGE_SERVICE/bridges/$DEPLOYER_ADDRESS?limit=20000&offset=0" | \
   jq -r '.deposits[] | select(.ready_for_claim == true and .claim_tx_hash == "" and .tx_hash=="'$TX_HASH'")')
DEPOSIT_CNT=$(echo "$result" | jq -r '.deposit_cnt')
NETWORK_ID=$(echo "$result" | jq -r '.network_id')
GLOBAL_INDEX=$(echo "$result" | jq -r '.global_index')
ORINGIN_NETWORK=$(echo "$result" | jq -r '.orig_net')
ORINGIN_ADDRESS=$(echo "$result" | jq -r '.orig_addr')
DESTINATION_NETWORK=$(echo "$result" | jq -r '.dest_net')
IN_AMOUNT=$(echo "$result" | jq -r '.amount')
METADATA=$(echo "$result" | jq -r '.metadata')
echo "Deposit Count: $DEPOSIT_CNT"
echo "Network ID: $NETWORK_ID"
echo "Global Index: $GLOBAL_INDEX"
echo "Origin Network: $ORINGIN_NETWORK"
echo "Origin Address: $ORINGIN_ADDRESS"
echo "Destination Network: $DESTINATION_NETWORK"
echo "In Amount: $IN_AMOUNT"
echo "Metadata: $METADATA"

proof=$(curl -s "$BRIDGE_SERVICE/merkle-proof?deposit_cnt=$DEPOSIT_CNT&net_id=$NETWORK_ID" | jq -r '.')
MERKLE_PROOF=$(echo "$proof" | jq -r -c '.proof | .merkle_proof' | tr -d '"')
ROLLUP_MERKLE_PROOF=$(echo "$proof" | jq -r -c '.proof | .rollup_merkle_proof' | tr -d '"')
MER=$(echo "$proof" | jq -r '.proof | .main_exit_root')
RER=$(echo "$proof" | jq -r '.proof | .rollup_exit_root')
echo "Merkle Proof: $MERKLE_PROOF"
echo "Rollup Merkle Proof: $ROLLUP_MERKLE_PROOF"
echo "Main Exit Root: $MER"
echo "Rollup Exit Root: $RER"

# Claim assets on L1
echo "Claiming assets on L1..."
L1_BALANCE_BEFORE_CLAIM=$(cast balance "$DEPLOYER_ADDRESS" --rpc-url "$L1_RPC_URL")
cmd="cast send --legacy --rpc-url $L1_RPC_URL --private-key $DEPLOYER_PRIVATE_KEY $BRIDGE_ADDRESS 'claimAsset(bytes32[32],bytes32[32],uint256,bytes32,bytes32,uint32,address,uint32,address,uint256,bytes)' $MERKLE_PROOF $ROLLUP_MERKLE_PROOF $GLOBAL_INDEX $MER $RER $ORINGIN_NETWORK $ORINGIN_ADDRESS $DESTINATION_NETWORK $DEPLOYER_ADDRESS $IN_AMOUNT $METADATA"

echo "Warning!!!!!!!!!! Claim asset on L1, Executing: $cmd"
eval "$cmd"

L1_BALANCE_AFTER_CLAIM=$(cast balance "$DEPLOYER_ADDRESS" --rpc-url "$L1_RPC_URL")
echo "Balance on L1:"
echo "  Before claiming: $L1_BALANCE_BEFORE_CLAIM"
echo "  After claiming: $L1_BALANCE_AFTER_CLAIM"
