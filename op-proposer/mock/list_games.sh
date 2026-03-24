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

# ── Helpers ──────────────────────────────────────────────────────────────────

# Map ProposalStatus enum (uint8) to name
# 0=Unchallenged, 1=Challenged, 2=UnchallengedAndValidProofProvided,
# 3=ChallengedAndValidProofProvided, 4=Resolved
proposal_status_name() {
  case "$1" in
    0) echo "Unchallenged" ;;
    1) echo "Challenged" ;;
    2) echo "UnchallengedAndValidProofProvided" ;;
    3) echo "ChallengedAndValidProofProvided" ;;
    4) echo "Resolved" ;;
    *) echo "Unknown($1)" ;;
  esac
}

# Map GameStatus enum (uint8) to name
# 0=IN_PROGRESS, 1=CHALLENGER_WINS, 2=DEFENDER_WINS
game_status_name() {
  case "$1" in
    0) echo "IN_PROGRESS" ;;
    1) echo "CHALLENGER_WINS" ;;
    2) echo "DEFENDER_WINS" ;;
    *) echo "Unknown($1)" ;;
  esac
}

# Map BondDistributionMode enum (uint8) to name
# 0=UNDECIDED, 1=NORMAL, 2=REFUND
bond_mode_name() {
  case "$1" in
    0) echo "UNDECIDED" ;;
    1) echo "NORMAL" ;;
    2) echo "REFUND" ;;
    *) echo "Unknown($1)" ;;
  esac
}

# Format a unix timestamp as human-readable; "0" or "N/A" → "N/A"
fmt_ts() {
  local ts="$1"
  if [[ "$ts" == "0" || "$ts" == "N/A" ]]; then echo "N/A"; return; fi
  date -r "$ts" "+%Y-%m-%d %H:%M:%S" 2>/dev/null \
    || date -d "@$ts" "+%Y-%m-%d %H:%M:%S" 2>/dev/null \
    || echo "$ts"
}

# Format a duration in seconds as "Xh Ym Zs"
fmt_duration() {
  local secs="$1"
  if [[ "$secs" == "N/A" ]]; then echo "N/A"; return; fi
  local h=$(( secs / 3600 ))
  local m=$(( (secs % 3600) / 60 ))
  local s=$(( secs % 60 ))
  printf "%dh %dm %ds" "$h" "$m" "$s"
}

# Print a labeled table row: "  Key:                    Value"
row() { printf "  %-32s %s\n" "$1" "$2"; }

# Section header / footer
section() { echo "  ┌─── $1"; }
section_end() { echo "  └$(printf '─%.0s' {1..100})┘"; }

# Tree-style phase node and indented child row for section [3]
phase() { printf "  ├─ %s\n" "$1"; }
trow()  { printf "  │      %-26s %s\n" "$1" "$2"; }

# Format a unix timestamp field: extract numeric part from cast output,
# then append human-readable time.  Returns "N/A" when value is 0 or N/A.
fmt_ts_field() {
  local raw="$1"
  local num
  num=$(echo "$raw" | awk '{print $1}')   # strip cast's "[1.774e9]" suffix
  if [[ "$num" == "0" || "$num" == "N/A" || -z "$num" ]]; then
    echo "N/A"
    return
  fi
  local human
  human=$(date -r "$num" "+%Y-%m-%d %H:%M:%S" 2>/dev/null \
          || date -d "@$num" "+%Y-%m-%d %H:%M:%S" 2>/dev/null \
          || echo "?")
  echo "${num}  (${human})"
}

# ── Main ─────────────────────────────────────────────────────────────────────

echo "Factory : $FACTORY"
TOTAL=$(cast call "$FACTORY" "gameCount()(uint256)" --rpc-url "$RPC")
echo "Total   : $TOTAL games"
echo ""

if [[ "$TOTAL" -eq 0 ]]; then
  echo "No games yet."
  exit 0
fi

if [[ "$COUNT" -gt "$TOTAL" ]]; then
  COUNT="$TOTAL"
fi

for (( i = TOTAL - 1; i >= TOTAL - COUNT; i-- )); do

  # ── Factory record ──────────────────────────────────────────────────────
  INFO=$(cast call "$FACTORY" "gameAtIndex(uint256)(uint8,uint64,address)" "$i" --rpc-url "$RPC")
  GAME_TYPE=$(echo "$INFO" | awk 'NR==1')
  ADDR=$(echo  "$INFO" | awk 'NR==3')

  echo "╔══════════════════════════════════════════════════════════════╗"
  printf "║  GAME #%-6s  │  GameType: %-33s║\n" "$i" "$GAME_TYPE"
  echo "╚══════════════════════════════════════════════════════════════╝"

  # ── Fetch all fields ────────────────────────────────────────────────────

  # Immutables
  MAX_CHAL_DUR=$( cast call "$ADDR" "maxChallengeDuration()(uint64)" --rpc-url "$RPC" 2>/dev/null || echo "N/A")
  MAX_PROVE_DUR=$(cast call "$ADDR" "maxProveDuration()(uint64)"     --rpc-url "$RPC" 2>/dev/null || echo "N/A")
  CHAL_BOND=$(    cast call "$ADDR" "challengerBond()(uint256)"      --rpc-url "$RPC" 2>/dev/null || echo "N/A")

  # Identity
  GAME_CREATOR=$(  cast call "$ADDR" "gameCreator()(address)"                        --rpc-url "$RPC" 2>/dev/null || echo "N/A")
  PROPOSER_ADDR=$( cast call "$ADDR" "proposer()(address)"                           --rpc-url "$RPC" 2>/dev/null || echo "N/A")
  WAS_RESPECTED=$( cast call "$ADDR" "wasRespectedGameTypeWhenCreated()(bool)"        --rpc-url "$RPC" 2>/dev/null || echo "N/A")

  # Proposal range
  L2_BLOCK=$(      cast call "$ADDR" "l2BlockNumber()(uint256)"       --rpc-url "$RPC" 2>/dev/null || echo "N/A")
  PARENT_IDX=$(    cast call "$ADDR" "parentIndex()(uint32)"          --rpc-url "$RPC" 2>/dev/null || echo "N/A")
  STARTING_BN=$(   cast call "$ADDR" "startingBlockNumber()(uint256)" --rpc-url "$RPC" 2>/dev/null || echo "N/A")
  STARTING_HASH=$( cast call "$ADDR" "startingRootHash()(bytes32)"    --rpc-url "$RPC" 2>/dev/null || echo "N/A")
  ROOT_CLAIM=$(    cast call "$ADDR" "rootClaim()(bytes32)"           --rpc-url "$RPC" 2>/dev/null || echo "N/A")
  BLOCK_HASH=$(    cast call "$ADDR" "blockHash()(bytes32)"           --rpc-url "$RPC" 2>/dev/null || echo "N/A")
  STATE_HASH=$(    cast call "$ADDR" "stateHash()(bytes32)"           --rpc-url "$RPC" 2>/dev/null || echo "N/A")

  # ClaimData struct: (uint32 parentIndex, address counteredBy, address prover,
  #                    bytes32 claim, uint8 status, uint64 deadline)
  CLAIM_RAW=$(cast call "$ADDR" "claimData()(uint32,address,address,bytes32,uint8,uint64)" \
                --rpc-url "$RPC" 2>/dev/null || echo "N/A")
  if [[ "$CLAIM_RAW" != "N/A" ]]; then
    CD_COUNTERED=$(  echo "$CLAIM_RAW" | awk 'NR==2')
    CD_PROVER=$(     echo "$CLAIM_RAW" | awk 'NR==3')
    CD_STATUS_RAW=$( echo "$CLAIM_RAW" | awk 'NR==5')
    CD_DEADLINE=$(   echo "$CLAIM_RAW" | awk 'NR==6')
  else
    CD_COUNTERED="N/A"; CD_PROVER="N/A"; CD_STATUS_RAW="N/A"; CD_DEADLINE="N/A"
  fi

  # Game-level state
  GAME_STATUS_RAW=$( cast call "$ADDR" "status()(uint8)"               --rpc-url "$RPC" 2>/dev/null || echo "N/A")
  CREATED_AT_RAW=$(  cast call "$ADDR" "createdAt()(uint64)"           --rpc-url "$RPC" 2>/dev/null || echo "N/A")
  RESOLVED_AT_RAW=$( cast call "$ADDR" "resolvedAt()(uint64)"          --rpc-url "$RPC" 2>/dev/null || echo "N/A")
  BOND_MODE_RAW=$(   cast call "$ADDR" "bondDistributionMode()(uint8)" --rpc-url "$RPC" 2>/dev/null || echo "N/A")
  GAME_OVER=$(       cast call "$ADDR" "gameOver()(bool)"              --rpc-url "$RPC" 2>/dev/null || echo "N/A")

  # ── Derived values ──────────────────────────────────────────────────────

  CD_STATUS=$(proposal_status_name "$CD_STATUS_RAW")
  GAME_STATUS=$(game_status_name   "$GAME_STATUS_RAW")
  BOND_MODE=$(bond_mode_name       "$BOND_MODE_RAW")

  CREATED_AT_FMT=$(fmt_ts_field  "$CREATED_AT_RAW")
  RESOLVED_AT_FMT=$(fmt_ts_field "$RESOLVED_AT_RAW")
  DEADLINE_FMT=$(fmt_ts_field    "$(echo "$CD_DEADLINE" | awk '{print $1}')")

  MAX_CHAL_FMT="N/A"
  MAX_PROVE_FMT="N/A"
  if [[ "$MAX_CHAL_DUR"  != "N/A" ]]; then MAX_CHAL_FMT="${MAX_CHAL_DUR}s  ($(fmt_duration "$MAX_CHAL_DUR"))"; fi
  if [[ "$MAX_PROVE_DUR" != "N/A" ]]; then MAX_PROVE_FMT="${MAX_PROVE_DUR}s  ($(fmt_duration "$MAX_PROVE_DUR"))"; fi

  CHAL_BOND_FMT="N/A"
  if [[ "$CHAL_BOND" != "N/A" ]]; then
    CHAL_BOND_ETH=$(cast to-unit "$CHAL_BOND" ether 2>/dev/null || echo "?")
    CHAL_BOND_FMT="${CHAL_BOND_ETH} ETH  (${CHAL_BOND} wei)"
  fi

  # ── Section 1: Identity & Config ────────────────────────────────────────
  echo ""
  section "[1] Identity & Config ──────────────────────────────────────────────────────────────────────────┐"
  phase "Identity"
  trow "Address:"               "$ADDR"
  trow "GameType:"              "$GAME_TYPE"
  trow "GameCreator:"           "$GAME_CREATOR"
  trow "Proposer:"              "$PROPOSER_ADDR"
  trow "WasRespectedGameType:"  "$WAS_RESPECTED"
  phase "Config"
  trow "MaxChallengeDuration:"  "$MAX_CHAL_FMT"
  trow "MaxProveDuration:"      "$MAX_PROVE_FMT"
  trow "ChallengerBond:"        "$CHAL_BOND_FMT"
  section_end

  # ── Section 2: Proposal (L2 block range & hashes) ───────────────────────
  echo ""
  section "[2] Proposal  ──────────────────────────────────────────────────────────────────────────────────┐"
  phase "Starting State"
  trow "ParentIndex:"           "$PARENT_IDX"
  trow "StartingBlockNumber:"   "$STARTING_BN"
  trow "StartingRootHash:"      "$STARTING_HASH"
  phase "Target State"
  trow "L2BlockNumber:"         "$L2_BLOCK"
  trow "BlockHash:"             "$BLOCK_HASH"
  trow "StateHash:"             "$STATE_HASH"
  trow "RootClaim:"             "$ROOT_CLAIM"
  section_end

  # ── Section 3: Lifecycle State ──────────────────────────────────────────
  echo ""
  section "[3] Lifecycle State ────────────────────────────────────────────────────────────────────────────┐"
  phase "Initialize"
  trow "CreatedAt:"              "$CREATED_AT_FMT"
  phase "Challenge Window"
  trow "CounteredBy:"            "$CD_COUNTERED"
  trow "ClaimData.status:"       "$CD_STATUS"
  trow "ClaimData.deadline:"     "$DEADLINE_FMT"
  phase "Prove"
  trow "Prover:"                 "$CD_PROVER"
  trow "ClaimData.status:"       "$CD_STATUS"
  trow "GameOver:"               "$GAME_OVER"
  phase "Resolve"
  trow "GameStatus:"             "$GAME_STATUS"
  trow "ResolvedAt:"             "$RESOLVED_AT_FMT"
  phase "CloseGame/ClaimCredit"
  trow "BondDistributionMode:"   "$BOND_MODE"
  section_end

  echo ""
done

echo "════════════════════════════════════════════════════════════════════════════════════════════════════════"
echo "Done."
