#!/bin/bash
# test-tx.sh — Send a test transaction and poll it through all OP Stack phases
#
# Sends an ETH transfer, then polls unsafe → safe → finalized.
# Exits 0 if the TX reaches finalized within timeout. Exits 1 on timeout/error.
#
# Usage:
#   ./scripts/devnet/test-tx.sh                     # one TX with defaults
#   ./scripts/devnet/test-tx.sh --count 5           # send 5 TXs
#   ./scripts/devnet/test-tx.sh --value 0.5ether    # different value
#   ./scripts/devnet/test-tx.sh --timeout 300       # wait up to 5 minutes per phase
#   ./scripts/devnet/test-tx.sh --verbose           # per-component timing breakdown

set -e
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
check_deps cast jq curl

# ── Parse flags ───────────────────────────────────────────────────────────────
TX_COUNT=1
TX_VALUE="0.01ether"
PHASE_TIMEOUT=180   # seconds to wait for each phase transition
VERBOSE=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --count)   TX_COUNT="$2";      shift 2 ;;
        --value)   TX_VALUE="$2";      shift 2 ;;
        --timeout) PHASE_TIMEOUT="$2"; shift 2 ;;
        --verbose) VERBOSE=true;       shift   ;;
        *) echo "Unknown flag: $1"; exit 1 ;;
    esac
done

# ── Pre-flight ────────────────────────────────────────────────────────────────
if [ -z "${TEST_SENDER_KEY:-}" ]; then
    fail "TEST_SENDER_KEY not set in config/devnet/.env"
    exit 1
fi
if [ -z "${TEST_RECIPIENT:-}" ]; then
    fail "TEST_RECIPIENT not set in config/devnet/.env"
    exit 1
fi

wait_for_rpc "$L2_RPC_URL" "xlayer-node L2 RPC"

# ── Safe-lag pre-flight ───────────────────────────────────────────────────────
# If the batcher has a backlog, TXs sent now will not become SAFE until it clears.
# The backlog estimate: ~2s per L2 block at current batcher throughput.
_SYNC=$(curl -sf -X POST "$L2_ROLLUP_RPC_URL" \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' 2>/dev/null || echo "{}")
_SAFE_N=$(echo "$_SYNC"   | jq -r '.result.safe_l2.number   // "0x0"' | xargs printf '%d\n' 2>/dev/null || echo 0)
_UNSAFE_N=$(echo "$_SYNC" | jq -r '.result.unsafe_l2.number // "0x0"' | xargs printf '%d\n' 2>/dev/null || echo 0)
_SAFE_LAG=$(( _UNSAFE_N - _SAFE_N ))
if [ "$_SAFE_LAG" -gt 100 ] 2>/dev/null; then
    warn "Safe lag is $_SAFE_LAG blocks — op-batcher is catching up."
    warn "TXs sent now may not reach SAFE within ${PHASE_TIMEOUT}s."
    warn "Wait for lag to drop, or pass --timeout N (estimate: ~$((_SAFE_LAG * 2 + PHASE_TIMEOUT))s)."
    echo "  Monitor: ./scripts/devnet/health-check.sh --once"
    echo ""
fi
unset _SYNC _SAFE_N _UNSAFE_N _SAFE_LAG

SENDER_ADDR=$(cast wallet address "$TEST_SENDER_KEY")
SENDER_BALANCE=$(cast balance --rpc-url "$L2_RPC_URL" "$SENDER_ADDR" --ether 2>/dev/null || echo "unknown")

info "Sender:    $SENDER_ADDR ($SENDER_BALANCE ETH)"
info "Recipient: $TEST_RECIPIENT"
info "Value:     $TX_VALUE per TX, $TX_COUNT TX(s)"
echo ""

# ── Helpers ───────────────────────────────────────────────────────────────────
phase_label() {
    case "$1" in
        0) echo "PENDING"   ;;
        1) echo "UNSAFE"    ;;
        2) echo "SAFE"      ;;
        3) echo "FINALIZED" ;;
    esac
}

# hex_to_dec: convert hex (0x...) or decimal string to decimal integer
# printf %d handles both: "0x832321" → 8594209, "8594913" → 8594913
hex_to_dec() {
    printf '%d\n' "$1" 2>/dev/null || echo "0"
}

# get_sync_status: fetch optimism_syncStatus, return raw JSON
get_sync_status() {
    curl -sf -X POST "$L2_ROLLUP_RPC_URL" \
        -H "Content-Type: application/json" \
        -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' 2>/dev/null || echo "{}"
}

# tx_phase: check which OP Stack phase a TX has reached
# Returns: 0=pending, 1=unsafe, 2=safe, 3=finalized
tx_phase() {
    local tx_hash="$1"

    local block_hex
    block_hex=$(cast receipt --rpc-url "$L2_RPC_URL" "$tx_hash" 2>/dev/null \
        | grep -E "^blockNumber" | awk '{print $2}') || true
    if [ -z "$block_hex" ]; then echo "0"; return; fi

    local tx_block
    tx_block=$(hex_to_dec "$block_hex")
    if [ -z "$tx_block" ] || [ "$tx_block" -eq 0 ] 2>/dev/null; then echo "0"; return; fi

    local status
    status=$(get_sync_status)

    if [ -z "$status" ] || [ "$status" = "{}" ]; then
        echo "1"; return  # rollup RPC not up yet — TX is at least unsafe
    fi

    local finalized safe unsafe
    finalized=$(echo "$status" | jq -r '.result.finalized_l2.number // "0"' | xargs -I{} sh -c 'printf "%d\n" "{}" 2>/dev/null || echo 0')
    safe=$(echo "$status"      | jq -r '.result.safe_l2.number // "0"'      | xargs -I{} sh -c 'printf "%d\n" "{}" 2>/dev/null || echo 0')
    unsafe=$(echo "$status"    | jq -r '.result.unsafe_l2.number // "0"'    | xargs -I{} sh -c 'printf "%d\n" "{}" 2>/dev/null || echo 0')

    if   [ "$tx_block" -le "$finalized" ] 2>/dev/null; then echo "3"
    elif [ "$tx_block" -le "$safe" ] 2>/dev/null;      then echo "2"
    elif [ "$tx_block" -le "$unsafe" ] 2>/dev/null;    then echo "1"
    else echo "0"
    fi
}

# ── Send all TXs ──────────────────────────────────────────────────────────────
PASS=0; FAIL=0
declare -a TX_HASHES=()
declare -a TX_SUBMIT_TS=()   # unix timestamp (seconds) when each TX was submitted

for i in $(seq 1 "$TX_COUNT"); do
    step "TX $i/$TX_COUNT: sending $TX_VALUE to $TEST_RECIPIENT..."

    TS_BEFORE=$(date +%s)
    TX_HASH=$(cast send \
        --rpc-url "$L2_RPC_URL" \
        --private-key "$TEST_SENDER_KEY" \
        "$TEST_RECIPIENT" \
        --value "$TX_VALUE" \
        --json 2>/dev/null | jq -r '.transactionHash') || {
        fail "TX $i: send failed"
        FAIL=$((FAIL + 1))
        continue
    }

    ok "TX $i sent: $TX_HASH"
    TX_HASHES+=("$TX_HASH")
    TX_SUBMIT_TS+=("$TS_BEFORE")
done

echo ""
step "Tracking phases (timeout: ${PHASE_TIMEOUT}s per phase)..."
echo ""

# ── Track each TX through all phases ─────────────────────────────────────────
TX_IDX=0
for TX_HASH in "${TX_HASHES[@]}"; do
    SUBMIT_TS="${TX_SUBMIT_TS[$TX_IDX]}"
    TX_IDX=$((TX_IDX + 1))

    # Per-TX verbose state
    TX_BLOCK=""
    L1_AT_SAFE=0
    PHASE1_ELAPSED=0
    PHASE2_ELAPSED=0
    PHASE3_ELAPSED=0
    TX_TRACK_START=$SECONDS

    echo -e "${BOLD}$TX_HASH${RESET}"

    for TARGET_PHASE in 1 2 3; do
        LABEL=$(phase_label "$TARGET_PHASE")
        printf "  %-12s  " "$LABEL"
        START_TIME=$SECONDS
        REACHED=false

        while true; do
            PHASE=$(tx_phase "$TX_HASH")
            if [ "$PHASE" -ge "$TARGET_PHASE" ] 2>/dev/null; then
                ELAPSED=$((SECONDS - START_TIME))
                echo -e "${GREEN}✅ +${ELAPSED}s${RESET}"

                # Store per-phase elapsed
                case "$TARGET_PHASE" in
                    1) PHASE1_ELAPSED=$ELAPSED ;;
                    2) PHASE2_ELAPSED=$ELAPSED ;;
                    3) PHASE3_ELAPSED=$ELAPSED ;;
                esac
                REACHED=true
                break
            fi

            ELAPSED=$((SECONDS - START_TIME))
            if [ "$ELAPSED" -ge "$PHASE_TIMEOUT" ]; then
                echo -e "${RED}❌ timeout (${PHASE_TIMEOUT}s)${RESET}"
                break
            fi

            printf "."
            sleep 2
        done

        if [ "$REACHED" = false ]; then
            FAIL=$((FAIL + 1))
            break
        fi

        # ── Verbose: per-phase detail ─────────────────────────────────────
        if [ "$VERBOSE" = true ]; then
            case "$TARGET_PHASE" in
                1) # UNSAFE — get L2 block and inclusion delay
                    BLOCK_HEX=$(cast receipt --rpc-url "$L2_RPC_URL" "$TX_HASH" 2>/dev/null \
                        | grep -E "^blockNumber" | awk '{print $2}' || echo "")
                    if [ -n "$BLOCK_HEX" ]; then
                        TX_BLOCK=$(hex_to_dec "$BLOCK_HEX")
                        L2_BLOCK_TS=$(cast block --rpc-url "$L2_RPC_URL" "$TX_BLOCK" \
                            --field timestamp 2>/dev/null | xargs printf '%d' 2>/dev/null || echo "0")
                        INCLUSION_DELAY=$((L2_BLOCK_TS - SUBMIT_TS))
                        echo "    └─ L2 block $TX_BLOCK | TX→block sealed: ~${INCLUSION_DELAY}s  [reth sequencer]"
                    fi
                    ;;
                2) # SAFE — note L1 block kona has reached
                    STATUS=$(get_sync_status)
                    L1_AT_SAFE=$(echo "$STATUS" | jq -r '.result.current_l1.number // "0x0"' \
                        | xargs -I{} sh -c 'printf "%d\n" "{}" 2>/dev/null || echo 0')
                    echo "    └─ L1 current block ~$L1_AT_SAFE | op-batcher submitted batch, kona derived  [batcher + kona]"
                    ;;
                3) # FINALIZED — L1 blocks elapsed since SAFE
                    STATUS=$(get_sync_status)
                    L1_AT_FINAL=$(echo "$STATUS" | jq -r '.result.current_l1.number // "0x0"' \
                        | xargs -I{} sh -c 'printf "%d\n" "{}" 2>/dev/null || echo 0')
                    L1_DELTA=$((L1_AT_FINAL - L1_AT_SAFE))
                    echo "    └─ L1 current block ~$L1_AT_FINAL | $L1_DELTA L1 blocks for PoS finality  [L1 consensus]"
                    ;;
            esac
        fi

        if [ "$TARGET_PHASE" -eq 3 ]; then
            PASS=$((PASS + 1))
        fi
    done

    # ── Verbose: total breakdown ──────────────────────────────────────────
    if [ "$VERBOSE" = true ] && [ "$REACHED" = true ]; then
        TOTAL_ELAPSED=$((SECONDS - TX_TRACK_START))
        echo ""
        echo "  ┌─ Component breakdown ──────────────────────────────────────"
        printf "  │  TX submit → UNSAFE     (reth seals block):    ~%ds\n"  "$PHASE1_ELAPSED"
        printf "  │  UNSAFE    → SAFE       (op-batcher + kona):   ~%ds\n"  "$PHASE2_ELAPSED"
        printf "  │  SAFE      → FINALIZED  (L1 PoS finality):     ~%ds\n"  "$PHASE3_ELAPSED"
        echo "  └────────────────────────────────────────────────────────────"
        printf "     Total TX submit → FINALIZED:                   ~%ds\n" "$TOTAL_ELAPSED"
    fi

    echo ""
done

# ── Summary ───────────────────────────────────────────────────────────────────
echo "────────────────────────────────────"
if [ "$FAIL" -eq 0 ]; then
    ok "All $PASS TX(s) reached FINALIZED"
    exit 0
else
    fail "$PASS passed, $FAIL failed"
    exit 1
fi
