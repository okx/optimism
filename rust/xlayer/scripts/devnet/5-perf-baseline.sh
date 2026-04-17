#!/bin/bash
# 5-perf-baseline.sh — Enhanced load test with Engine API latency measurement
#
# Measures:
#   - TX submission rate (client → L2 RPC)
#   - Time-to-unsafe / time-to-safe / time-to-finalized (wall clock)
#   - Engine API call latencies: FCU, new_payload, get_payload (from logs)
#   - Block production rate and gaps
#   - Effective TPS across all stages
#
# Designed for apples-to-apples comparison with reth+op-node (2-process) setup.
#
# Usage:
#   ./scripts/devnet/5-perf-baseline.sh                           # 50 TXs serial (default)
#   ./scripts/devnet/5-perf-baseline.sh --count 100               # 100 TXs serial
#   ./scripts/devnet/5-perf-baseline.sh --parallel --count 200    # 200 TXs parallel (fast)
#   ./scripts/devnet/5-perf-baseline.sh --duration 60             # Send TXs for 60s continuously
#
# Output:
#   - Console: summary table
#   - CSV: results/perf-baseline-<timestamp>.csv (for charting)
#   - JSON: results/perf-baseline-<timestamp>.json (for automation)

set -e
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
check_deps cast jq curl bc python3

# ── Configuration ─────────────────────────────────────────────────────────────
TX_COUNT=50
DURATION=0               # if > 0, send TXs continuously for N seconds instead of --count
PARALLEL=false
WAIT_UNSAFE_TIMEOUT=60
WAIT_SAFE_TIMEOUT=300
WAIT_FINALIZED_TIMEOUT=600
RESULTS_DIR="$XLAYER_ROOT/results"
TIMESTAMP=$(date '+%Y%m%d-%H%M%S')

while [[ $# -gt 0 ]]; do
    case "$1" in
        --count)      TX_COUNT="$2"; shift 2 ;;
        --duration)   DURATION="$2"; shift 2 ;;
        --parallel)   PARALLEL=true; shift ;;
        --results-dir) RESULTS_DIR="$2"; shift 2 ;;
        *) echo "Unknown flag: $1"; exit 1 ;;
    esac
done

mkdir -p "$RESULTS_DIR"
CSV_FILE="$RESULTS_DIR/perf-baseline-$TIMESTAMP.csv"
JSON_FILE="$RESULTS_DIR/perf-baseline-$TIMESTAMP.json"

if [ -z "${TEST_SENDER_KEY:-}" ] || [ -z "${TEST_RECIPIENT:-}" ]; then
    fail "TEST_SENDER_KEY and TEST_RECIPIENT must be set in config/devnet/.env"
    exit 1
fi

# ── Multi-sender setup (for parallel mode) ───────────────────────────────────
declare -a SENDER_KEYS=()
declare -a SENDER_ADDRS=()

if [ "$PARALLEL" = true ]; then
    info "Parallel mode: using multiple sender accounts to avoid nonce races"

    CANDIDATE_KEYS=(
        "${TEST_SENDER_KEY}"
        "${OP_PROPOSER_PRIVATE_KEY:-}"
        "${OP_CHALLENGER_PRIVATE_KEY:-}"
    )

    SEEN_ADDRS=""

    for KEY in "${CANDIDATE_KEYS[@]}"; do
        [ -z "$KEY" ] && continue
        ADDR=$(cast wallet address "$KEY" 2>/dev/null || echo "")
        [ -z "$ADDR" ] && continue

        if echo "$SEEN_ADDRS" | grep -q "$ADDR"; then
            continue
        fi
        SEEN_ADDRS="$SEEN_ADDRS $ADDR"

        BAL=$(cast balance --rpc-url "$L2_RPC_URL" "$ADDR" 2>/dev/null || echo "0")
        MIN_BAL="100000000000000000"  # 0.1 ETH

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
        echo "  See: docs/setup/parallel-mode-setup.md"
        exit 1
    fi

    info "Using ${#SENDER_KEYS[@]} sender accounts for parallel submission"
else
    SENDER_KEYS=("$TEST_SENDER_KEY")
    SENDER_ADDRS=("$(cast wallet address "$TEST_SENDER_KEY")")
fi

# ══════════════════════════════════════════════════════════════════════════════
# ── COMPREHENSIVE PRE-FLIGHT HEALTH CHECKS ────────────────────────────────────
# ══════════════════════════════════════════════════════════════════════════════
step "Pre-flight health checks (validating all nodes)..."
echo ""

HEALTH_FAILED=false

# Check 1: L1 geth
printf "  [1/6] L1 geth (execution)...           "
L1_BLOCK=$(cast bn --rpc-url "$L1_RPC_URL" 2>/dev/null || echo "FAILED")
if [ "$L1_BLOCK" = "FAILED" ] || ! [[ "$L1_BLOCK" =~ ^[0-9]+$ ]]; then
    echo -e "${RED}DOWN${RESET}"
    echo "        RPC: $L1_RPC_URL"
    echo "        Fix: ./scripts/devnet/1-start-l1.sh"
    HEALTH_FAILED=true
else
    echo -e "${GREEN}OK${RESET} (block: $L1_BLOCK)"
fi

# Check 2: L1 Beacon
printf "  [2/6] L1 beacon-chain (consensus)...   "
BEACON_STATUS=$(curl -sf --max-time 3 "$L1_BEACON_URL/eth/v1/node/syncing" 2>/dev/null || echo "FAILED")
if [ "$BEACON_STATUS" = "FAILED" ]; then
    echo -e "${RED}DOWN${RESET}"
    echo "        RPC: $L1_BEACON_URL"
    HEALTH_FAILED=true
else
    BEACON_SYNCING=$(echo "$BEACON_STATUS" | jq -r '.data.is_syncing // "unknown"' 2>/dev/null || echo "unknown")
    if [ "$BEACON_SYNCING" = "false" ]; then
        echo -e "${GREEN}OK${RESET} (synced)"
    else
        echo -e "${YELLOW}SYNCING${RESET}"
    fi
fi

# Check 3: L1 Validator
printf "  [3/6] L1 validator...                  "
VALIDATOR_RUNNING=$(docker ps --format '{{.Names}}' --filter "name=l1-validator" 2>/dev/null || echo "")
if [ -z "$VALIDATOR_RUNNING" ]; then
    echo -e "${RED}DOWN${RESET}"
    HEALTH_FAILED=true
else
    echo -e "${GREEN}OK${RESET}"
fi

# Check 4: xlayer-node L2 execution
printf "  [4/6] xlayer-node (L2 execution)...    "
L2_BLOCK_1=$(cast bn --rpc-url "$L2_RPC_URL" 2>/dev/null || echo "FAILED")
if [ "$L2_BLOCK_1" = "FAILED" ] || ! [[ "$L2_BLOCK_1" =~ ^[0-9]+$ ]]; then
    echo -e "${RED}DOWN${RESET}"
    echo "        RPC: $L2_RPC_URL"
    echo "        Fix: ./scripts/devnet/2-start-node.sh"
    HEALTH_FAILED=true
else
    # Verify blocks are being produced (wait 3s and check again)
    sleep 3
    L2_BLOCK_2=$(cast bn --rpc-url "$L2_RPC_URL" 2>/dev/null || echo "FAILED")
    if [ "$L2_BLOCK_2" = "FAILED" ] || [ "$L2_BLOCK_2" = "$L2_BLOCK_1" ]; then
        echo -e "${RED}STALLED${RESET} (block: $L2_BLOCK_1, not progressing)"
        echo "        Node responding but NOT producing blocks"
        echo "        Fix: Check xlayer-node logs: tail -50 logs/xlayer-node.log"
        HEALTH_FAILED=true
    else
        echo -e "${GREEN}OK${RESET} (block: $L2_BLOCK_2, producing)"
    fi
fi

# Check 5: kona-node rollup RPC
printf "  [5/6] kona-node (L2 rollup/consensus)... "
ROLLUP_STATUS=$(curl -sf --max-time 3 -X POST "$L2_ROLLUP_RPC_URL" \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' 2>/dev/null || echo "FAILED")

if [ "$ROLLUP_STATUS" = "FAILED" ]; then
    echo -e "${RED}DOWN${RESET}"
    echo "        RPC: $L2_ROLLUP_RPC_URL"
    HEALTH_FAILED=true
else
    UNSAFE=$(echo "$ROLLUP_STATUS" | jq -r '.result.unsafe_l2.number // "0x0"' 2>/dev/null | xargs printf '%d\n' 2>/dev/null || echo "0")
    SAFE=$(echo "$ROLLUP_STATUS" | jq -r '.result.safe_l2.number // "0x0"' 2>/dev/null | xargs printf '%d\n' 2>/dev/null || echo "0")
    echo -e "${GREEN}OK${RESET} (unsafe: $UNSAFE, safe: $SAFE)"
fi

# Check 6: op-batcher
printf "  [6/6] op-batcher...                    "
BATCHER_RUNNING=$(docker ps --format '{{.Names}}' --filter "name=op-batcher" 2>/dev/null || echo "")
if [ -z "$BATCHER_RUNNING" ]; then
    echo -e "${YELLOW}DOWN${RESET}"
    echo "        Impact: Safe/finalized heads will NOT advance"
    echo "        Fix: docker compose -f docker/docker-compose.devnet.yml up -d op-batcher"
    echo ""
    echo "        ${YELLOW}WARNING:${RESET} Only unsafe metrics will be measured"
    sleep 2
else
    echo -e "${GREEN}OK${RESET}"
fi

echo ""

if [ "$HEALTH_FAILED" = true ]; then
    fail "Pre-flight checks FAILED — cannot proceed"
    echo ""
    echo "  ${BOLD}Quick fix (start everything):${RESET}"
    echo "    ./scripts/devnet/0-all.sh"
    exit 1
fi

ok "All nodes healthy — starting load test"
echo ""

# ── Utility functions ─────────────────────────────────────────────────────────
ms_now() {
    python3 -c "import time; print(int(time.time() * 1000))" 2>/dev/null \
        || echo "$(($(date +%s) * 1000))"
}


if [ "$PARALLEL" = false ]; then
    SENDER="${SENDER_ADDRS[0]}"
    BALANCE=$(cast balance --rpc-url "$L2_RPC_URL" "$SENDER" --ether 2>/dev/null || echo "?")
    info "Sender: $SENDER ($BALANCE ETH)"
fi

# ── Capture baseline sync status ─────────────────────────────────────────────
step "Capturing baseline state..."
BASELINE_STATUS=$(curl -sf -X POST "$L2_ROLLUP_RPC_URL" \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' 2>/dev/null || echo "{}")

BASELINE_UNSAFE=$(echo "$BASELINE_STATUS" | jq -r '.result.unsafe_l2.number // "0x0"' | xargs printf '%d\n' 2>/dev/null || echo "0")
BASELINE_SAFE=$(echo "$BASELINE_STATUS" | jq -r '.result.safe_l2.number // "0x0"' | xargs printf '%d\n' 2>/dev/null || echo "0")
BASELINE_FINALIZED=$(echo "$BASELINE_STATUS" | jq -r '.result.finalized_l2.number // "0x0"' | xargs printf '%d\n' 2>/dev/null || echo "0")

info "Baseline: unsafe=$BASELINE_UNSAFE, safe=$BASELINE_SAFE, finalized=$BASELINE_FINALIZED"
echo ""

# ── Mark log position for Engine API metrics extraction ──────────────────────
LOG_FILE="${LOG_FILE:-$XLAYER_ROOT/logs/xlayer-node.log}"
if [ -f "$LOG_FILE" ]; then
    LOG_MARK_LINE=$(wc -l < "$LOG_FILE" 2>/dev/null || echo "0")
else
    LOG_MARK_LINE=0
    warn "Log file not found at $LOG_FILE — Engine API metrics will be unavailable"
fi

# ── Phase 1: Send TXs ─────────────────────────────────────────────────────────
if [ "$DURATION" -gt 0 ]; then
    info "Sending TXs continuously for ${DURATION}s..."
else
    info "Sending $TX_COUNT transactions..."
fi
echo ""

declare -a TX_HASHES=()
declare -a SEND_TIMES=()

SEND_START=$SECONDS
SEND_START_MS=$(ms_now)

if [ "$DURATION" -gt 0 ]; then
    # Duration mode: send TXs continuously until time expires
    END_TIME=$((SECONDS + DURATION))
    TX_NUM=0

    if [ "$PARALLEL" = true ]; then
        # Parallel continuous mode
        declare -a PIDS=()
        while [ $SECONDS -lt $END_TIME ]; do
            for SENDER_IDX in "${!SENDER_KEYS[@]}"; do
                [ $SECONDS -ge $END_TIME ] && break

                KEY="${SENDER_KEYS[$SENDER_IDX]}"
                T=$(ms_now)
                TX_NUM=$((TX_NUM + 1))

                (
                    HASH=$(cast send --rpc-url "$L2_RPC_URL" --private-key "$KEY" \
                        "$TEST_RECIPIENT" --value "0.001ether" --json 2>/dev/null \
                        | jq -r '.transactionHash') || echo "FAILED"
                    if [ "$HASH" != "FAILED" ] && [ -n "$HASH" ]; then
                        echo "$HASH|$T" >> "$RESULTS_DIR/.txs-$TIMESTAMP.tmp"
                    fi
                ) &
                PIDS+=($!)

                # Keep background job count reasonable
                if [ ${#PIDS[@]} -ge 10 ]; then
                    wait "${PIDS[0]}" 2>/dev/null || true
                    PIDS=("${PIDS[@]:1}")
                fi
            done
        done

        # Wait for remaining jobs
        for pid in "${PIDS[@]}"; do
            wait "$pid" 2>/dev/null || true
        done

        # Collect results
        if [ -f "$RESULTS_DIR/.txs-$TIMESTAMP.tmp" ]; then
            while IFS='|' read -r HASH T; do
                TX_HASHES+=("$HASH")
                SEND_TIMES+=("$T")
            done < "$RESULTS_DIR/.txs-$TIMESTAMP.tmp"
            rm -f "$RESULTS_DIR/.txs-$TIMESTAMP.tmp"
        fi
    else
        # Serial continuous mode
        while [ $SECONDS -lt $END_TIME ]; do
            T=$(ms_now)
            TX_NUM=$((TX_NUM + 1))
            HASH=$(cast send --rpc-url "$L2_RPC_URL" --private-key "$TEST_SENDER_KEY" \
                "$TEST_RECIPIENT" --value "0.001ether" --json 2>/dev/null \
                | jq -r '.transactionHash') || continue
            TX_HASHES+=("$HASH")
            SEND_TIMES+=("$T")
            [ $((TX_NUM % 10)) -eq 0 ] && printf "  sent %d TXs...\n" "$TX_NUM"
        done
    fi

    TX_COUNT=$TX_NUM
else
    # Count mode: send exactly TX_COUNT transactions
    if [ "$PARALLEL" = true ]; then
        declare -a PIDS=()
        declare -a TMPFILES=()

        for i in $(seq 1 "$TX_COUNT"); do
            SENDER_IDX=$(( (i - 1) % ${#SENDER_KEYS[@]} ))
            KEY="${SENDER_KEYS[$SENDER_IDX]}"

            T=$(ms_now)
            TMPFILE=$(mktemp)
            TMPFILES+=("$TMPFILE")

            (
                HASH=$(cast send --rpc-url "$L2_RPC_URL" --private-key "$KEY" \
                    "$TEST_RECIPIENT" --value "0.001ether" --json 2>/dev/null \
                    | jq -r '.transactionHash') || echo "FAILED"
                echo "$HASH|$T" > "$TMPFILE"
            ) &
            PIDS+=($!)
        done

        for pid in "${PIDS[@]}"; do
            wait "$pid" 2>/dev/null || true
        done

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
                    warn "TX $((i+1)) send failed"
                fi
            fi
        done
    else
        for i in $(seq 1 "$TX_COUNT"); do
            T=$(ms_now)
            HASH=$(cast send --rpc-url "$L2_RPC_URL" --private-key "$TEST_SENDER_KEY" \
                "$TEST_RECIPIENT" --value "0.001ether" --json 2>/dev/null \
                | jq -r '.transactionHash') || {
                warn "TX $i send failed"
                continue
            }
            TX_HASHES+=("$HASH")
            SEND_TIMES+=("$T")
            printf "  sent %d/%d  %s\n" "$i" "$TX_COUNT" "$HASH"
        done
    fi
fi

SEND_END=$SECONDS
SEND_END_MS=$(ms_now)
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
        warn "Unsafe timeout after ${WAIT_UNSAFE_TIMEOUT}s — only $CONFIRMED/$SENT_COUNT confirmed"
        break
    fi

    for i in "${!TX_HASHES[@]}"; do
        HASH="${TX_HASHES[$i]}"
        [ -n "${UNSAFE_TIMES[$i]:-}" ] && continue

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

UNSAFE_END=$SECONDS
echo ""

if [ $CONFIRMED -eq 0 ]; then
    fail "No transactions confirmed after ${WAIT_UNSAFE_TIMEOUT}s"
    echo ""
    echo "  Possible causes:"
    echo "    1. L2 node stopped producing blocks"
    echo "    2. Transactions rejected (insufficient gas, nonce issues)"
    echo "    3. RPC connection issue"
    echo ""
    echo "  Debug:"
    echo "    Check current block: cast bn --rpc-url $L2_RPC_URL"
    echo "    Check TX status:     cast receipt --rpc-url $L2_RPC_URL <tx_hash>"
    echo "    Check node logs:     tail -50 logs/xlayer-node.log"
    exit 1
elif [ $CONFIRMED -lt $((SENT_COUNT / 2)) ]; then
    warn "Only $CONFIRMED/$SENT_COUNT confirmed — many TXs failed or timed out"
fi

ok "Unsafe: $CONFIRMED/$SENT_COUNT confirmed in $((UNSAFE_END - POLL_START))s"
echo ""

# ── Phase 3: Wait for safe head ──────────────────────────────────────────────
if [ $CONFIRMED -gt 0 ]; then
    step "Waiting for safe head to advance..."

    MAX_UNSAFE_BLOCK=0
    for i in "${!UNSAFE_BLOCKS[@]}"; do
        B="${UNSAFE_BLOCKS[$i]}"
        [[ "$B" =~ ^[0-9]+$ ]] && [ "$B" -gt "$MAX_UNSAFE_BLOCK" ] && MAX_UNSAFE_BLOCK="$B"
    done

    declare -a SAFE_TIMES=()
    SAFE_START=$SECONDS
    SAFE_REACHED=false

    while true; do
        STATUS=$(curl -sf -X POST "$L2_ROLLUP_RPC_URL" \
            -H "Content-Type: application/json" \
            -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' 2>/dev/null || echo "{}")
        SAFE_HEX=$(echo "$STATUS" | jq -r '.result.safe_l2.number // "0x0"')
        SAFE_N=$(printf '%d\n' "$SAFE_HEX" 2>/dev/null || echo "0")

        # Record safe time for each TX
        T=$(ms_now)
        for i in "${!UNSAFE_BLOCKS[@]}"; do
            [ -n "${SAFE_TIMES[$i]:-}" ] && continue
            B="${UNSAFE_BLOCKS[$i]}"
            [[ "$B" =~ ^[0-9]+$ ]] && [ "$SAFE_N" -ge "$B" ] && SAFE_TIMES[$i]="$T"
        done

        if [ "$SAFE_N" -ge "$MAX_UNSAFE_BLOCK" ] 2>/dev/null; then
            SAFE_ELAPSED=$((SECONDS - SAFE_START))
            ok "Safe head reached block $SAFE_N in +${SAFE_ELAPSED}s"
            SAFE_REACHED=true
            break
        fi

        if [ $((SECONDS - SAFE_START)) -ge $WAIT_SAFE_TIMEOUT ]; then
            warn "Safe head timeout — safe at $SAFE_N, need $MAX_UNSAFE_BLOCK"
            break
        fi
        sleep 2
    done
fi

SAFE_END=$SECONDS
echo ""

# ── Phase 4: Wait for finalized head ──────────────────────────────────────────
if [ "$SAFE_REACHED" = true ]; then
    step "Waiting for finalized head to advance..."

    declare -a FINALIZED_TIMES=()
    FINALIZED_START=$SECONDS
    FINALIZED_REACHED=false

    while true; do
        STATUS=$(curl -sf -X POST "$L2_ROLLUP_RPC_URL" \
            -H "Content-Type: application/json" \
            -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' 2>/dev/null || echo "{}")
        FINAL_HEX=$(echo "$STATUS" | jq -r '.result.finalized_l2.number // "0x0"')
        FINAL_N=$(printf '%d\n' "$FINAL_HEX" 2>/dev/null || echo "0")

        T=$(ms_now)
        for i in "${!UNSAFE_BLOCKS[@]}"; do
            [ -n "${FINALIZED_TIMES[$i]:-}" ] && continue
            B="${UNSAFE_BLOCKS[$i]}"
            [[ "$B" =~ ^[0-9]+$ ]] && [ "$FINAL_N" -ge "$B" ] && FINALIZED_TIMES[$i]="$T"
        done

        if [ "$FINAL_N" -ge "$MAX_UNSAFE_BLOCK" ] 2>/dev/null; then
            FINALIZED_ELAPSED=$((SECONDS - FINALIZED_START))
            ok "Finalized head reached block $FINAL_N in +${FINALIZED_ELAPSED}s"
            FINALIZED_REACHED=true
            break
        fi

        if [ $((SECONDS - FINALIZED_START)) -ge $WAIT_FINALIZED_TIMEOUT ]; then
            warn "Finalized head timeout — finalized at $FINAL_N, need $MAX_UNSAFE_BLOCK"
            break
        fi
        sleep 3
    done
fi

FINALIZED_END=$SECONDS
echo ""

# ── Phase 5: Extract Engine API metrics from logs ────────────────────────────
step "Extracting Engine API latencies from logs..."

declare -a FCU_LATENCIES=()
declare -a NEW_PAYLOAD_LATENCIES=()
declare -a GET_PAYLOAD_LATENCIES=()

if [ -f "$LOG_FILE" ] && [ "$LOG_MARK_LINE" -gt 0 ]; then
    # Extract lines added since test started
    tail -n +$((LOG_MARK_LINE + 1)) "$LOG_FILE" > "$RESULTS_DIR/.log-extract-$TIMESTAMP.tmp" 2>/dev/null || true

    # Parse FCU latencies: look for "FCU ok" with elapsed
    grep -E 'engine_bridge.*FCU ok.*elapsed' "$RESULTS_DIR/.log-extract-$TIMESTAMP.tmp" 2>/dev/null | \
        grep -oE 'elapsed=[0-9.]+[a-zµ]+' | \
        sed 's/elapsed=//' > "$RESULTS_DIR/.fcu-$TIMESTAMP.tmp" || true

    # Parse new_payload latencies
    grep -E 'engine_bridge.*new_payload ok.*elapsed' "$RESULTS_DIR/.log-extract-$TIMESTAMP.tmp" 2>/dev/null | \
        grep -oE 'elapsed=[0-9.]+[a-zµ]+' | \
        sed 's/elapsed=//' > "$RESULTS_DIR/.newpayload-$TIMESTAMP.tmp" || true

    # Parse get_payload latencies
    grep -E 'engine_bridge.*get_payload ok.*elapsed' "$RESULTS_DIR/.log-extract-$TIMESTAMP.tmp" 2>/dev/null | \
        grep -oE 'elapsed=[0-9.]+[a-zµ]+' | \
        sed 's/elapsed=//' > "$RESULTS_DIR/.getpayload-$TIMESTAMP.tmp" || true

    # Convert to arrays
    if [ -f "$RESULTS_DIR/.fcu-$TIMESTAMP.tmp" ]; then
        while IFS= read -r line; do
            FCU_LATENCIES+=("$line")
        done < "$RESULTS_DIR/.fcu-$TIMESTAMP.tmp"
    fi

    if [ -f "$RESULTS_DIR/.newpayload-$TIMESTAMP.tmp" ]; then
        while IFS= read -r line; do
            NEW_PAYLOAD_LATENCIES+=("$line")
        done < "$RESULTS_DIR/.newpayload-$TIMESTAMP.tmp"
    fi

    if [ -f "$RESULTS_DIR/.getpayload-$TIMESTAMP.tmp" ]; then
        while IFS= read -r line; do
            GET_PAYLOAD_LATENCIES+=("$line")
        done < "$RESULTS_DIR/.getpayload-$TIMESTAMP.tmp"
    fi

    rm -f "$RESULTS_DIR/.log-extract-$TIMESTAMP.tmp" \
          "$RESULTS_DIR/.fcu-$TIMESTAMP.tmp" \
          "$RESULTS_DIR/.newpayload-$TIMESTAMP.tmp" \
          "$RESULTS_DIR/.getpayload-$TIMESTAMP.tmp"

    info "Captured: ${#FCU_LATENCIES[@]} FCU, ${#NEW_PAYLOAD_LATENCIES[@]} new_payload, ${#GET_PAYLOAD_LATENCIES[@]} get_payload calls"
else
    warn "Cannot extract Engine API metrics — log file not available"
fi

echo ""

# ── Phase 6: Compute statistics ──────────────────────────────────────────────
step "Computing statistics..."

# Helper: convert duration string (e.g., "1.2ms", "345µs") to microseconds
# ══════════════════════════════════════════════════════════════════════════════
# ── COMPREHENSIVE PRE-FLIGHT HEALTH CHECKS ────────────────────────────────────
# ══════════════════════════════════════════════════════════════════════════════
# Validate ALL critical infrastructure before starting the load test.
# If any component is down, FAIL FAST with clear instructions.

step "Pre-flight health checks (all nodes must be running)..."
echo ""

HEALTH_FAILED=false

# ── Check 1: L1 Execution (geth) ──────────────────────────────────────────────
printf "  [1/6] L1 geth (execution)...           "
L1_BLOCK=$(cast bn --rpc-url "$L1_RPC_URL" 2>/dev/null || echo "FAILED")
if [ "$L1_BLOCK" = "FAILED" ] || ! [[ "$L1_BLOCK" =~ ^[0-9]+$ ]]; then
    echo -e "${RED}DOWN${RESET}"
    echo "        RPC: $L1_RPC_URL"
    echo "        Fix: docker compose -f docker/docker-compose.devnet.yml ps l1-geth"
    echo "             docker compose -f docker/docker-compose.devnet.yml start l1-geth"
    HEALTH_FAILED=true
else
    echo -e "${GREEN}OK${RESET} (block: $L1_BLOCK)"
fi

# ── Check 2: L1 Beacon Chain ──────────────────────────────────────────────────
printf "  [2/6] L1 beacon-chain (consensus)...   "
BEACON_STATUS=$(curl -sf --max-time 3 "$L1_BEACON_URL/eth/v1/node/syncing" 2>/dev/null || echo "FAILED")
if [ "$BEACON_STATUS" = "FAILED" ]; then
    echo -e "${RED}DOWN${RESET}"
    echo "        RPC: $L1_BEACON_URL"
    echo "        Fix: docker compose -f docker/docker-compose.devnet.yml ps l1-beacon-chain"
    echo "             docker compose -f docker/docker-compose.devnet.yml start l1-beacon-chain"
    HEALTH_FAILED=true
else
    BEACON_SYNCING=$(echo "$BEACON_STATUS" | jq -r '.data.is_syncing // "unknown"' 2>/dev/null || echo "unknown")
    if [ "$BEACON_SYNCING" = "false" ]; then
        echo -e "${GREEN}OK${RESET} (synced)"
    elif [ "$BEACON_SYNCING" = "true" ]; then
        echo -e "${YELLOW}SYNCING${RESET}"
        echo "        Beacon is syncing — safe/finalized metrics may lag"
    else
        echo -e "${YELLOW}UNKNOWN${RESET}"
    fi
fi

# ── Check 3: L1 Validator ─────────────────────────────────────────────────────
printf "  [3/6] L1 validator...                  "
VALIDATOR_RUNNING=$(docker ps --format '{{.Names}}' --filter "name=l1-validator" 2>/dev/null || echo "")
if [ -z "$VALIDATOR_RUNNING" ]; then
    echo -e "${RED}DOWN${RESET}"
    echo "        Container: l1-validator"
    echo "        Fix: docker compose -f docker/docker-compose.devnet.yml start l1-validator"
    HEALTH_FAILED=true
else
    VALIDATOR_STATUS=$(docker inspect -f '{{.State.Status}}' l1-validator 2>/dev/null || echo "unknown")
    if [ "$VALIDATOR_STATUS" = "running" ]; then
        echo -e "${GREEN}OK${RESET}"
    else
        echo -e "${RED}$VALIDATOR_STATUS${RESET}"
        HEALTH_FAILED=true
    fi
fi

# ── Check 4: xlayer-node (L2 execution RPC) ───────────────────────────────────
printf "  [4/6] xlayer-node (L2 execution)...    "
L2_BLOCK=$(cast bn --rpc-url "$L2_RPC_URL" 2>/dev/null || echo "FAILED")
if [ "$L2_BLOCK" = "FAILED" ] || ! [[ "$L2_BLOCK" =~ ^[0-9]+$ ]]; then
    echo -e "${RED}DOWN${RESET}"
    echo "        RPC: $L2_RPC_URL"
    echo "        Fix: Check if xlayer-node process is running:"
    echo "             ps aux | grep xlayer-node"
    echo "             ./scripts/devnet/2-start-node.sh"
    HEALTH_FAILED=true
else
    echo -e "${GREEN}OK${RESET} (block: $L2_BLOCK)"
fi

# ── Check 5: kona-node (L2 rollup RPC inside xlayer-node) ────────────────────
printf "  [5/6] kona-node (L2 rollup/consensus)... "
ROLLUP_STATUS=$(curl -sf --max-time 3 -X POST "$L2_ROLLUP_RPC_URL" \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' 2>/dev/null || echo "FAILED")

if [ "$ROLLUP_STATUS" = "FAILED" ]; then
    echo -e "${RED}DOWN${RESET}"
    echo "        RPC: $L2_ROLLUP_RPC_URL"
    echo "        Note: Rollup RPC is served by kona inside xlayer-node"
    echo "        Fix: Ensure xlayer-node is running with rollup RPC enabled"
    HEALTH_FAILED=true
else
    UNSAFE=$(echo "$ROLLUP_STATUS" | jq -r '.result.unsafe_l2.number // "0x0"' 2>/dev/null | xargs printf '%d\n' 2>/dev/null || echo "0")
    SAFE=$(echo "$ROLLUP_STATUS" | jq -r '.result.safe_l2.number // "0x0"' 2>/dev/null | xargs printf '%d\n' 2>/dev/null || echo "0")
    echo -e "${GREEN}OK${RESET} (unsafe: $UNSAFE, safe: $SAFE)"
fi

# ── Check 6: op-batcher ───────────────────────────────────────────────────────
printf "  [6/6] op-batcher...                    "
BATCHER_RUNNING=$(docker ps --format '{{.Names}}' --filter "name=op-batcher" 2>/dev/null || echo "")
if [ -z "$BATCHER_RUNNING" ]; then
    echo -e "${YELLOW}DOWN${RESET}"
    echo "        Container: op-batcher"
    echo "        Impact: Safe/finalized heads will NOT advance"
    echo "        Fix: docker compose -f docker/docker-compose.devnet.yml up -d op-batcher"
    echo ""
    echo "        ${YELLOW}WARNING:${RESET} Continuing without batcher — only unsafe metrics will be measured"
    sleep 3
else
    BATCHER_STATUS=$(docker inspect -f '{{.State.Status}}' op-batcher 2>/dev/null || echo "unknown")
    if [ "$BATCHER_STATUS" = "running" ]; then
        # Check admin RPC (quick probe)
        if curl -sf --max-time 1 http://127.0.0.1:8548/ >/dev/null 2>&1; then
            echo -e "${GREEN}OK${RESET} (admin RPC responding)"
        else
            echo -e "${YELLOW}OK${RESET} (running, admin RPC not responding)"
        fi
    else
        echo -e "${RED}$BATCHER_STATUS${RESET}"
        echo "        Impact: Safe/finalized heads will NOT advance"
        HEALTH_FAILED=true
    fi
fi

echo ""

# ── FAIL FAST if any critical component is down ───────────────────────────────
if [ "$HEALTH_FAILED" = true ]; then
    fail "Pre-flight checks FAILED — cannot proceed with load test"
    echo ""
    echo "  ${BOLD}Quick fix (start everything):${RESET}"
    echo "    ./scripts/devnet/0-all.sh"
    echo ""
    echo "  Or start individual components as shown above."
    exit 1
fi

ok "All nodes healthy — ready to start load test"
echo ""

duration_to_us() {
    local val="$1"
    if [[ "$val" =~ ^([0-9.]+)([a-zµ]+)$ ]]; then
        local num="${BASH_REMATCH[1]}"
        local unit="${BASH_REMATCH[2]}"
        case "$unit" in
            s)   echo "$num * 1000000" | bc -l | cut -d. -f1 ;;
            ms)  echo "$num * 1000" | bc -l | cut -d. -f1 ;;
            µs|us) echo "$num" | cut -d. -f1 ;;
            ns)  echo "$num / 1000" | bc -l | cut -d. -f1 ;;
            *)   echo "0" ;;
        esac
    else
        echo "0"
    fi
}

# Compute stats for an array of durations
compute_stats() {
    local arr_name="$1[@]"
    local arr=("${!arr_name}")

    if [ ${#arr[@]} -eq 0 ]; then
        echo "0|0|0|0|0"  # count|min|max|avg|p95
        return
    fi

    local -a values_us=()
    for v in "${arr[@]}"; do
        values_us+=("$(duration_to_us "$v")")
    done

    # Sort
    IFS=$'\n' sorted=($(sort -n <<<"${values_us[*]}"))
    unset IFS

    local count=${#sorted[@]}
    local min=${sorted[0]}
    local max=${sorted[$((count - 1))]}

    local sum=0
    for v in "${sorted[@]}"; do
        sum=$((sum + v))
    done
    local avg=$((sum / count))

    local p95_idx=$(( count * 95 / 100 ))
    [ "$p95_idx" -ge "$count" ] && p95_idx=$((count - 1))
    local p95=${sorted[$p95_idx]}

    echo "$count|$min|$max|$avg|$p95"
}

FCU_STATS=$(compute_stats FCU_LATENCIES)
NP_STATS=$(compute_stats NEW_PAYLOAD_LATENCIES)
GP_STATS=$(compute_stats GET_PAYLOAD_LATENCIES)

# TX latencies
TOTAL_UNSAFE_MS=0
TOTAL_SAFE_MS=0
TOTAL_FINALIZED_MS=0
COUNT_UNSAFE=0
COUNT_SAFE=0
COUNT_FINALIZED=0

for i in "${!TX_HASHES[@]}"; do
    if [ -n "${UNSAFE_TIMES[$i]:-}" ]; then
        LATENCY_MS=$((UNSAFE_TIMES[$i] - SEND_TIMES[$i]))
        TOTAL_UNSAFE_MS=$((TOTAL_UNSAFE_MS + LATENCY_MS))
        COUNT_UNSAFE=$((COUNT_UNSAFE + 1))
    fi

    if [ -n "${SAFE_TIMES[$i]:-}" ]; then
        LATENCY_MS=$((SAFE_TIMES[$i] - SEND_TIMES[$i]))
        TOTAL_SAFE_MS=$((TOTAL_SAFE_MS + LATENCY_MS))
        COUNT_SAFE=$((COUNT_SAFE + 1))
    fi

    if [ -n "${FINALIZED_TIMES[$i]:-}" ]; then
        LATENCY_MS=$((FINALIZED_TIMES[$i] - SEND_TIMES[$i]))
        TOTAL_FINALIZED_MS=$((TOTAL_FINALIZED_MS + LATENCY_MS))
        COUNT_FINALIZED=$((COUNT_FINALIZED + 1))
    fi
done

AVG_UNSAFE_MS=0
AVG_SAFE_MS=0
AVG_FINALIZED_MS=0

[ "$COUNT_UNSAFE" -gt 0 ] && AVG_UNSAFE_MS=$(echo "scale=0; $TOTAL_UNSAFE_MS / $COUNT_UNSAFE" | bc)
[ "$COUNT_SAFE" -gt 0 ] && AVG_SAFE_MS=$(echo "scale=0; $TOTAL_SAFE_MS / $COUNT_SAFE" | bc)
[ "$COUNT_FINALIZED" -gt 0 ] && AVG_FINALIZED_MS=$(echo "scale=0; $TOTAL_FINALIZED_MS / $COUNT_FINALIZED" | bc)

WALL_CLOCK=$((SECONDS - SEND_START))
[ "$WALL_CLOCK" -eq 0 ] && WALL_CLOCK=1

TPS_UNSAFE=$(echo "scale=2; $COUNT_UNSAFE / $WALL_CLOCK" | bc)
TPS_SAFE=$(echo "scale=2; $COUNT_SAFE / $((SAFE_END - SEND_START))" | bc 2>/dev/null || echo "0")
TPS_FINALIZED=$(echo "scale=2; $COUNT_FINALIZED / $((FINALIZED_END - SEND_START))" | bc 2>/dev/null || echo "0")

# ── Phase 7: Print results ────────────────────────────────────────────────────
echo ""
step "Results"
echo ""

printf "${BOLD}Transaction Throughput${RESET}\n"
printf "  %-30s %s\n" "TXs sent:"              "$SENT_COUNT"
printf "  %-30s %s\n" "TXs confirmed (unsafe):" "$COUNT_UNSAFE"
printf "  %-30s %s\n" "TXs confirmed (safe):"   "$COUNT_SAFE"
printf "  %-30s %s\n" "TXs confirmed (finalized):" "$COUNT_FINALIZED"
printf "  %-30s %s\n" "Submission rate:"       "$(echo "scale=1; $SENT_COUNT / $SEND_ELAPSED" | bc) TX/s"
printf "  %-30s %s\n" "Effective TPS (unsafe):" "$TPS_UNSAFE"
printf "  %-30s %s\n" "Effective TPS (safe):"   "$TPS_SAFE"
printf "  %-30s %s\n" "Effective TPS (finalized):" "$TPS_FINALIZED"
echo ""

printf "${BOLD}Transaction Latencies (avg)${RESET}\n"
printf "  %-30s %s\n" "Time to unsafe:"    "${AVG_UNSAFE_MS}ms"
printf "  %-30s %s\n" "Time to safe:"      "${AVG_SAFE_MS}ms"
printf "  %-30s %s\n" "Time to finalized:" "${AVG_FINALIZED_MS}ms"
echo ""

printf "${BOLD}Engine API Call Latencies${RESET}\n"
IFS='|' read -r fcu_count fcu_min fcu_max fcu_avg fcu_p95 <<< "$FCU_STATS"
IFS='|' read -r np_count np_min np_max np_avg np_p95 <<< "$NP_STATS"
IFS='|' read -r gp_count gp_min gp_max gp_avg gp_p95 <<< "$GP_STATS"

printf "  ${CYAN}fork_choice_updated:${RESET}\n"
printf "    Count: %s  |  Avg: %s µs  |  P95: %s µs  |  Min: %s µs  |  Max: %s µs\n" \
    "$fcu_count" "$fcu_avg" "$fcu_p95" "$fcu_min" "$fcu_max"

printf "  ${CYAN}new_payload:${RESET}\n"
printf "    Count: %s  |  Avg: %s µs  |  P95: %s µs  |  Min: %s µs  |  Max: %s µs\n" \
    "$np_count" "$np_avg" "$np_p95" "$np_min" "$np_max"

printf "  ${CYAN}get_payload:${RESET}\n"
printf "    Count: %s  |  Avg: %s µs  |  P95: %s µs  |  Min: %s µs  |  Max: %s µs\n" \
    "$gp_count" "$gp_avg" "$gp_p95" "$gp_min" "$gp_max"

echo ""
printf "  %-30s %s\n" "Total elapsed:" "${WALL_CLOCK}s"
echo ""

# ── Phase 8: Export results ───────────────────────────────────────────────────
# CSV
{
    echo "metric,value,unit"
    echo "txs_sent,$SENT_COUNT,count"
    echo "txs_unsafe,$COUNT_UNSAFE,count"
    echo "txs_safe,$COUNT_SAFE,count"
    echo "txs_finalized,$COUNT_FINALIZED,count"
    echo "submission_rate,$(echo "scale=2; $SENT_COUNT / $SEND_ELAPSED" | bc),tx_per_sec"
    echo "tps_unsafe,$TPS_UNSAFE,tx_per_sec"
    echo "tps_safe,$TPS_SAFE,tx_per_sec"
    echo "tps_finalized,$TPS_FINALIZED,tx_per_sec"
    echo "latency_unsafe_avg,$AVG_UNSAFE_MS,ms"
    echo "latency_safe_avg,$AVG_SAFE_MS,ms"
    echo "latency_finalized_avg,$AVG_FINALIZED_MS,ms"
    echo "fcu_count,$fcu_count,count"
    echo "fcu_avg,$fcu_avg,us"
    echo "fcu_p95,$fcu_p95,us"
    echo "fcu_min,$fcu_min,us"
    echo "fcu_max,$fcu_max,us"
    echo "new_payload_count,$np_count,count"
    echo "new_payload_avg,$np_avg,us"
    echo "new_payload_p95,$np_p95,us"
    echo "new_payload_min,$np_min,us"
    echo "new_payload_max,$np_max,us"
    echo "get_payload_count,$gp_count,count"
    echo "get_payload_avg,$gp_avg,us"
    echo "get_payload_p95,$gp_p95,us"
    echo "get_payload_min,$gp_min,us"
    echo "get_payload_max,$gp_max,us"
    echo "wall_clock_total,$WALL_CLOCK,s"
} > "$CSV_FILE"

# JSON
cat > "$JSON_FILE" <<EOF
{
  "timestamp": "$TIMESTAMP",
  "config": {
    "tx_count": $TX_COUNT,
    "duration": $DURATION,
    "parallel": $PARALLEL,
    "sender_accounts": ${#SENDER_KEYS[@]}
  },
  "throughput": {
    "txs_sent": $SENT_COUNT,
    "txs_unsafe": $COUNT_UNSAFE,
    "txs_safe": $COUNT_SAFE,
    "txs_finalized": $COUNT_FINALIZED,
    "submission_rate_tx_per_sec": $(echo "scale=2; $SENT_COUNT / $SEND_ELAPSED" | bc),
    "tps_unsafe": $TPS_UNSAFE,
    "tps_safe": $TPS_SAFE,
    "tps_finalized": $TPS_FINALIZED
  },
  "latency_ms": {
    "unsafe_avg": $AVG_UNSAFE_MS,
    "safe_avg": $AVG_SAFE_MS,
    "finalized_avg": $AVG_FINALIZED_MS
  },
  "engine_api_us": {
    "fork_choice_updated": {
      "count": $fcu_count,
      "avg": $fcu_avg,
      "p95": $fcu_p95,
      "min": $fcu_min,
      "max": $fcu_max
    },
    "new_payload": {
      "count": $np_count,
      "avg": $np_avg,
      "p95": $np_p95,
      "min": $np_min,
      "max": $np_max
    },
    "get_payload": {
      "count": $gp_count,
      "avg": $gp_avg,
      "p95": $gp_p95,
      "min": $gp_min,
      "max": $gp_max
    }
  },
  "timing": {
    "wall_clock_total_s": $WALL_CLOCK,
    "send_elapsed_s": $SEND_ELAPSED
  }
}
EOF

ok "Results saved:"
echo "  CSV:  $CSV_FILE"
echo "  JSON: $JSON_FILE"
echo ""
ok "Load test complete"

