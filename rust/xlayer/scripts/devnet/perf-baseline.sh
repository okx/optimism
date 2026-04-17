#!/bin/bash
# perf-baseline.sh — Measure L2 TPS and TX latency baseline
#
# Sends a burst of transactions and measures:
#   - Submission rate (how fast we can send TXs to the RPC)
#   - Inclusion time (blocks until TX appears in unsafe head)
#   - Time-to-unsafe / time-to-safe (wall clock seconds)
#   - Effective TPS (TXs confirmed per second over measurement window)
#
# This is a rough baseline, not a load test. For sustained load testing,
# use the adventure benchmark in the reth pre-warming branch.
#
# Usage:
#   ./scripts/devnet/perf-baseline.sh                    # 20 TXs serial (default)
#   ./scripts/devnet/perf-baseline.sh --count 50         # 50 TXs serial
#   ./scripts/devnet/perf-baseline.sh --parallel         # 20 TXs parallel (multi-sender)
#   ./scripts/devnet/perf-baseline.sh --parallel --count 100  # 100 TXs parallel

set -e
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
check_deps cast jq curl bc

TX_COUNT=20
WAIT_UNSAFE_TIMEOUT=30   # seconds to wait for unsafe inclusion
WAIT_SAFE_TIMEOUT=120    # seconds to wait for safe
PARALLEL=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --count)    TX_COUNT="$2"; shift 2 ;;
        --parallel) PARALLEL=true; shift ;;
        *) echo "Unknown flag: $1"; exit 1 ;;
    esac
done

if [ -z "${TEST_SENDER_KEY:-}" ] || [ -z "${TEST_RECIPIENT:-}" ]; then
    fail "TEST_SENDER_KEY and TEST_RECIPIENT must be set in config/devnet/.env"
    exit 1
fi

# ── Multi-sender setup (for parallel mode) ───────────────────────────────────
# To avoid nonce races, we use multiple funded devnet accounts in round-robin.
# These are Foundry's default test accounts — safe for devnet only.
declare -a SENDER_KEYS=()
declare -a SENDER_ADDRS=()

if [ "$PARALLEL" = true ]; then
    info "Parallel mode: using multiple sender accounts to avoid nonce races"

    # Pool of devnet keys (from .env)
    CANDIDATE_KEYS=(
        "${TEST_SENDER_KEY}"
        "${OP_PROPOSER_PRIVATE_KEY:-}"
        "${OP_CHALLENGER_PRIVATE_KEY:-}"
    )

    # Track seen addresses to avoid duplicates (bash 3.2 compatible)
    SEEN_ADDRS=""

    for KEY in "${CANDIDATE_KEYS[@]}"; do
        [ -z "$KEY" ] && continue
        ADDR=$(cast wallet address "$KEY" 2>/dev/null || echo "")
        [ -z "$ADDR" ] && continue

        # Skip duplicate addresses (same key used multiple times in .env)
        if echo "$SEEN_ADDRS" | grep -q "$ADDR"; then
            continue
        fi
        SEEN_ADDRS="$SEEN_ADDRS $ADDR"

        # Check balance on L2
        BAL=$(cast balance --rpc-url "$L2_RPC_URL" "$ADDR" 2>/dev/null || echo "0")

        # Need at least 0.1 ETH to send TXs
        MIN_BAL="100000000000000000"  # 0.1 ETH in wei

        # Use bc for large number comparison
        if echo "$BAL >= $MIN_BAL" | bc -l | grep -q '^1$'; then
            SENDER_KEYS+=("$KEY")
            SENDER_ADDRS+=("$ADDR")
            BAL_ETH=$(cast balance --rpc-url "$L2_RPC_URL" "$ADDR" --ether 2>/dev/null || echo "?")
            info "  ✓ $ADDR ($BAL_ETH ETH)"
        else
            BAL_ETH=$(cast balance --rpc-url "$L2_RPC_URL" "$ADDR" --ether 2>/dev/null || echo "?")
            warn "Skipping $ADDR (insufficient balance: $BAL_ETH ETH)"
        fi
    done

    if [ ${#SENDER_KEYS[@]} -lt 2 ]; then
        fail "Parallel mode requires at least 2 funded accounts. Found: ${#SENDER_KEYS[@]}"
        echo ""
        echo "  Your .env currently has these accounts:"
        echo "    TEST_SENDER_KEY:        $(cast wallet address "$TEST_SENDER_KEY" 2>/dev/null || echo "invalid")"
        echo "    OP_PROPOSER_PRIVATE_KEY:  $(cast wallet address "${OP_PROPOSER_PRIVATE_KEY:-0x0}" 2>/dev/null || echo "not set")"
        echo "    OP_CHALLENGER_PRIVATE_KEY: $(cast wallet address "${OP_CHALLENGER_PRIVATE_KEY:-0x0}" 2>/dev/null || echo "not set")"
        echo ""
        echo "  To fund accounts on L2, run:"
        echo "    cast send --rpc-url http://localhost:8123 --private-key \$TEST_SENDER_KEY --value 10ether <addr>"
        exit 1
    fi

    info "Using ${#SENDER_KEYS[@]} sender accounts for parallel submission"
else
    # Serial mode: single sender
    SENDER_KEYS=("$TEST_SENDER_KEY")
    SENDER_ADDRS=("$(cast wallet address "$TEST_SENDER_KEY")")
fi

# Portable millisecond timestamp (macOS date lacks %N support)
ms_now() {
    python3 -c "import time; print(int(time.time() * 1000))" 2>/dev/null \
        || echo "$(($(date +%s) * 1000))"
}

wait_for_rpc "$L2_RPC_URL" "xlayer-node"

if [ "$PARALLEL" = false ]; then
    SENDER="${SENDER_ADDRS[0]}"
    BALANCE=$(cast balance --rpc-url "$L2_RPC_URL" "$SENDER" --ether 2>/dev/null || echo "?")
    info "Sender: $SENDER ($BALANCE ETH)"
fi

info "Sending $TX_COUNT transactions..."
echo ""

# ── Phase 1: Send all TXs, record send timestamps ────────────────────────────
declare -a TX_HASHES=()
declare -a SEND_TIMES=()

SEND_START=$SECONDS

if [ "$PARALLEL" = true ]; then
    # Parallel mode: send all TXs concurrently using background jobs
    # Round-robin across sender accounts to avoid nonce races
    declare -a PIDS=()
    declare -a TMPFILES=()

    for i in $(seq 1 "$TX_COUNT"); do
        SENDER_IDX=$(( (i - 1) % ${#SENDER_KEYS[@]} ))
        KEY="${SENDER_KEYS[$SENDER_IDX]}"

        T=$(ms_now)
        TMPFILE=$(mktemp)
        TMPFILES+=("$TMPFILE")

        (
            HASH=$(cast send \
                --rpc-url "$L2_RPC_URL" \
                --private-key "$KEY" \
                "$TEST_RECIPIENT" \
                --value "0.001ether" \
                --json 2>/dev/null | jq -r '.transactionHash') || echo "FAILED"
            echo "$HASH|$T" > "$TMPFILE"
        ) &
        PIDS+=($!)
    done

    # Wait for all background sends to complete
    for pid in "${PIDS[@]}"; do
        wait "$pid" 2>/dev/null || true
    done

    # Collect results from temp files
    for i in "${!TMPFILES[@]}"; do
        TMPFILE="${TMPFILES[$i]}"
        if [ -f "$TMPFILE" ]; then
            RESULT=$(cat "$TMPFILE")
            HASH=$(echo "$RESULT" | cut -d'|' -f1)
            T=$(echo "$RESULT" | cut -d'|' -f2)
            rm -f "$TMPFILE"

            if [ "$HASH" != "FAILED" ] && [ -n "$HASH" ]; then
                TX_HASHES+=("$HASH")
                SEND_TIMES+=("$T")
                printf "  sent %d/%d  %s\n" "$((i+1))" "$TX_COUNT" "$HASH"
            else
                warn "TX $((i+1)) send failed — skipping"
            fi
        fi
    done
else
    # Serial mode: send one at a time (original behavior)
    for i in $(seq 1 "$TX_COUNT"); do
        T=$(ms_now)
        HASH=$(cast send \
            --rpc-url "$L2_RPC_URL" \
            --private-key "$TEST_SENDER_KEY" \
            "$TEST_RECIPIENT" \
            --value "0.001ether" \
            --json 2>/dev/null | jq -r '.transactionHash') || {
            warn "TX $i send failed — skipping"
            continue
        }
        TX_HASHES+=("$HASH")
        SEND_TIMES+=("$T")
        printf "  sent %d/%d  %s\n" "$i" "$TX_COUNT" "$HASH"
    done
fi

SEND_END=$SECONDS
SENT_COUNT=${#TX_HASHES[@]}
SEND_ELAPSED=$((SEND_END - SEND_START))
[ "$SEND_ELAPSED" -eq 0 ] && SEND_ELAPSED=1

echo ""
ok "Sent $SENT_COUNT TXs in ${SEND_ELAPSED}s ($(echo "scale=1; $SENT_COUNT / $SEND_ELAPSED" | bc) TX/s submission rate)"
echo ""

# ── Phase 2: Poll for unsafe inclusion ───────────────────────────────────────
step "Waiting for all TXs to reach unsafe head..."

declare -a UNSAFE_TIMES=()
declare -a UNSAFE_BLOCKS=()

POLL_START=$SECONDS
CONFIRMED=0

while [ $CONFIRMED -lt $SENT_COUNT ]; do
    ELAPSED=$((SECONDS - POLL_START))
    if [ $ELAPSED -ge $WAIT_UNSAFE_TIMEOUT ]; then
        warn "Timeout after ${WAIT_UNSAFE_TIMEOUT}s — only $CONFIRMED/$SENT_COUNT confirmed unsafe"
        break
    fi

    for i in "${!TX_HASHES[@]}"; do
        HASH="${TX_HASHES[$i]}"
        [ -n "${UNSAFE_TIMES[$i]:-}" ] && continue  # already confirmed

        BLOCK_HEX=$(cast receipt --rpc-url "$L2_RPC_URL" "$HASH" 2>/dev/null \
            | grep -E "^blockNumber" | awk '{print $2}') || true
        if [ -n "$BLOCK_HEX" ]; then
            UNSAFE_TIMES[$i]=$(ms_now)
            UNSAFE_BLOCKS[$i]=$(printf '%d\n' "$BLOCK_HEX" 2>/dev/null || echo "?")
            CONFIRMED=$((CONFIRMED + 1))
        fi
    done
    sleep 0.5
done

echo ""

# ── Phase 3: Wait for safe head to cover all included TXs ───────────────────
if [ $CONFIRMED -gt 0 ]; then
    step "Waiting for safe head to advance..."

    MAX_BLOCK=0
    for i in "${!UNSAFE_BLOCKS[@]}"; do
        B="${UNSAFE_BLOCKS[$i]}"
        [[ "$B" =~ ^[0-9]+$ ]] && [ "$B" -gt "$MAX_BLOCK" ] && MAX_BLOCK="$B"
    done

    SAFE_REACHED=false
    SAFE_START=$SECONDS
    while true; do
        STATUS=$(curl -sf -X POST "$L2_ROLLUP_RPC_URL" \
            -H "Content-Type: application/json" \
            -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' 2>/dev/null || echo "{}")
        SAFE_HEX=$(echo "$STATUS" | jq -r '.result.safe_l2.number // "0x0"')
        SAFE_N=$(printf '%d\n' "$SAFE_HEX" 2>/dev/null || echo "0")

        if [ "$SAFE_N" -ge "$MAX_BLOCK" ] 2>/dev/null; then
            SAFE_ELAPSED=$((SECONDS - SAFE_START))
            ok "Safe head reached block $SAFE_N in +${SAFE_ELAPSED}s"
            SAFE_REACHED=true
            break
        fi

        if [ $((SECONDS - SAFE_START)) -ge $WAIT_SAFE_TIMEOUT ]; then
            warn "Safe head timeout — safe at $SAFE_N, need $MAX_BLOCK"
            break
        fi
        sleep 2
    done
fi

# ── Phase 4: Print results table ─────────────────────────────────────────────
echo ""
step "Results"
echo ""
printf "  %-6s %-70s %-12s %-12s\n" "TX#" "Hash" "Block" "Unsafe(ms)"
printf "  %-6s %-70s %-12s %-12s\n" "---" "----" "-----" "----------"

TOTAL_UNSAFE_MS=0
COUNT=0
for i in "${!TX_HASHES[@]}"; do
    HASH="${TX_HASHES[$i]}"
    if [ -n "${UNSAFE_TIMES[$i]:-}" ]; then
        LATENCY_MS=$((UNSAFE_TIMES[$i] - SEND_TIMES[$i]))
        printf "  %-6s %-70s %-12s %-12s\n" \
            "$((i+1))" "$HASH" "${UNSAFE_BLOCKS[$i]:-?}" "${LATENCY_MS}ms"
        TOTAL_UNSAFE_MS=$((TOTAL_UNSAFE_MS + LATENCY_MS))
        COUNT=$((COUNT + 1))
    else
        printf "  %-6s %-70s %-12s %-12s\n" "$((i+1))" "$HASH" "---" "TIMEOUT"
    fi
done

echo ""
if [ $COUNT -gt 0 ]; then
    AVG_UNSAFE_MS=$(echo "scale=0; $TOTAL_UNSAFE_MS / $COUNT" | bc)
    WALL_CLOCK=$((SECONDS - SEND_START))
    [ "$WALL_CLOCK" -eq 0 ] && WALL_CLOCK=1
    TPS_UNSAFE=$(echo "scale=1; $COUNT / $WALL_CLOCK" | bc)

    echo "  ──────────────────────────────────────────────"
    printf "  %-30s %s\n"  "TXs sent:"              "$SENT_COUNT"
    printf "  %-30s %s\n"  "TXs confirmed unsafe:"  "$COUNT"
    printf "  %-30s %s\n"  "Avg time-to-unsafe:"    "${AVG_UNSAFE_MS}ms"
    printf "  %-30s %s\n"  "Effective TPS (unsafe):" "$TPS_UNSAFE"
    printf "  %-30s %s\n"  "Total elapsed:"         "${WALL_CLOCK}s"
    echo ""
    ok "Baseline complete"
else
    fail "No TXs confirmed — check xlayer-node is running and funded"
    exit 1
fi
