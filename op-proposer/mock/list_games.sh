#!/usr/bin/env bash
# list_games.sh — Print the last N TEE dispute games from the factory.
# Usage: ./list_games.sh --rpc <L1_RPC_URL> --factory <FACTORY_ADDR> [--count <N>]
set -euo pipefail

usage() {
  echo "Usage: $0 --rpc <L1_RPC_URL> --factory <FACTORY_ADDR> [--count <N>]"
  echo ""
  echo "  --rpc      L1 RPC endpoint (e.g. http://localhost:8545)"
  echo "  --factory  DisputeGameFactory contract address"
  echo "  --count    Number of games to show, newest first (default: 10)"
  exit 1
}

COUNT=10
RPC=""
FACTORY=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --rpc)     RPC="$2";     shift 2 ;;
    --factory) FACTORY="$2"; shift 2 ;;
    --count)   COUNT="$2";   shift 2 ;;
    *) echo "Unknown argument: $1" >&2; usage ;;
  esac
done

[[ -z "$RPC" ]]     && { echo "ERROR: --rpc is required" >&2;     usage; }
[[ -z "$FACTORY" ]] && { echo "ERROR: --factory is required" >&2; usage; }

echo "Factory: $FACTORY"
TOTAL=$(cast call "$FACTORY" "gameCount()(uint256)" --rpc-url "$RPC")
echo "Total games: $TOTAL"
echo ""

if [[ "$TOTAL" -eq 0 ]]; then
  echo "No games yet."
  exit 0
fi

if [[ "$COUNT" -gt "$TOTAL" ]]; then
  COUNT="$TOTAL"
fi

for (( i = TOTAL - 1; i >= TOTAL - COUNT; i-- )); do
  INFO=$(cast call "$FACTORY" "gameAtIndex(uint256)(uint8,uint64,address)" "$i" --rpc-url "$RPC")
  GAME_TYPE=$(echo "$INFO" | awk 'NR==1')
  TIMESTAMP=$(echo "$INFO" | awk 'NR==2')
  ADDR=$(echo  "$INFO" | awk 'NR==3')

  PARENT_IDX=$(    cast call "$ADDR" "parentIndex()(uint32)"          --rpc-url "$RPC" 2>/dev/null || echo "N/A")
  STARTING_BN=$(   cast call "$ADDR" "startingBlockNumber()(uint256)" --rpc-url "$RPC" 2>/dev/null || echo "N/A")
  L2_BLOCK=$(      cast call "$ADDR" "l2BlockNumber()(uint256)"       --rpc-url "$RPC" 2>/dev/null || echo "N/A")
  STARTING_HASH=$( cast call "$ADDR" "startingRootHash()(bytes32)"    --rpc-url "$RPC" 2>/dev/null || echo "N/A")
  ROOT_CLAIM=$(    cast call "$ADDR" "rootClaim()(bytes32)"           --rpc-url "$RPC" 2>/dev/null || echo "N/A")
  BLOCK_HASH=$(    cast call "$ADDR" "blockHash()(bytes32)"           --rpc-url "$RPC" 2>/dev/null || echo "N/A")
  STATE_HASH=$(    cast call "$ADDR" "stateHash()(bytes32)"           --rpc-url "$RPC" 2>/dev/null || echo "N/A")
  PROPOSER_ADDR=$( cast call "$ADDR" "proposer()(address)"            --rpc-url "$RPC" 2>/dev/null || echo "N/A")
  GAME_CREATOR=$(  cast call "$ADDR" "gameCreator()(address)"         --rpc-url "$RPC" 2>/dev/null || echo "N/A")

  RAW_TS=$(echo "$TIMESTAMP" | awk '{print $1}')
  TS_HUMAN=$(date -r "$RAW_TS" "+%Y-%m-%d %H:%M:%S" 2>/dev/null || date -d "@$RAW_TS" "+%Y-%m-%d %H:%M:%S" 2>/dev/null || echo "$TIMESTAMP")

  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
  echo "Index:               $i"
  echo "GameType:            $GAME_TYPE"
  echo "Address:             $ADDR"
  echo "CreatedAt:           $TS_HUMAN"
  echo "GameCreator:         $GAME_CREATOR"
  echo "Proposer:            $PROPOSER_ADDR"
  echo "ParentIndex:         $PARENT_IDX"
  echo "StartingBlockNumber: $STARTING_BN"
  echo "L2BlockNumber:       $L2_BLOCK"
  echo "StartingRootHash:    $STARTING_HASH"
  echo "RootClaim:           $ROOT_CLAIM"
  echo "BlockHash:           $BLOCK_HASH"
  echo "StateHash:           $STATE_HASH"
done

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
