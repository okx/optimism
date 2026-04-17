#!/bin/bash
# health-check.sh — Comprehensive devnet health monitor
#
# Shows status of ALL components: L1 (geth + beacon), xlayer-node, op-batcher.
# Runs continuously (refresh every N seconds) or once with --once.
#
# Usage:
#   ./scripts/devnet/health-check.sh           # continuous, refresh every 5s
#   ./scripts/devnet/health-check.sh --once    # single snapshot, then exit
#   ./scripts/devnet/health-check.sh --watch 2 # refresh every 2s

set -e
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
check_deps cast jq curl docker

ONCE=false
INTERVAL=5

while [[ $# -gt 0 ]]; do
    case "$1" in
        --once)  ONCE=true; shift ;;
        --watch) INTERVAL="$2"; shift 2 ;;
        *) echo "Unknown flag: $1"; exit 1 ;;
    esac
done

# ── helpers ───────────────────────────────────────────────────────────────────

hex_to_dec() {
    local h="${1:-0x0}"
    printf '%d\n' "$h" 2>/dev/null || echo "0"
}

# Colour-coded status tag
status_ok()   { echo -e "${GREEN}✅ ${*}${RESET}"; }
status_warn() { echo -e "${YELLOW}⚠️  ${*}${RESET}"; }
status_err()  { echo -e "${RED}❌ ${*}${RESET}"; }

# Lag badge: green <warn, yellow <err, red >=err
lag_badge() {
    local val="$1" warn_thresh="$2" err_thresh="$3"
    if   [ "$val" -ge "$err_thresh"  ] 2>/dev/null; then echo -e "${RED}${val}${RESET}"
    elif [ "$val" -ge "$warn_thresh" ] 2>/dev/null; then echo -e "${YELLOW}${val}${RESET}"
    else                                                  echo -e "${GREEN}${val}${RESET}"
    fi
}

# ── snapshot ──────────────────────────────────────────────────────────────────
snapshot() {
    local ts; ts=$(date '+%H:%M:%S')
    local any_error=false

    printf "${BOLD}[%s] Devnet Health${RESET}\n" "$ts"
    printf "══════════════════════════════════════════\n"

    # ─────────────────────────────────────────────────────────────────────────
    # L1 SERVICES
    # ─────────────────────────────────────────────────────────────────────────
    printf "${BOLD}L1 (Ethereum)${RESET}\n"

    # l1-geth
    local l1_block="?" l1_line
    if docker ps --format '{{.Names}}' 2>/dev/null | grep -q '^l1-geth$'; then
        if l1_block=$(cast bn --rpc-url "$L1_RPC_URL" 2>/dev/null); then
            l1_line=$(status_ok "running  block ${CYAN}${l1_block}${RESET}")
        else
            l1_line=$(status_warn "container up but RPC not responding")
            any_error=true
        fi
    else
        l1_line=$(status_err "not running")
        any_error=true
    fi
    printf "  %-14s %s\n" "l1-geth:" "$l1_line"

    # l1-beacon-chain
    local beacon_line
    if docker ps --format '{{.Names}}' 2>/dev/null | grep -q '^l1-beacon-chain$'; then
        local bsync; bsync=$(curl -sf "$L1_BEACON_URL/eth/v1/node/syncing" 2>/dev/null || echo "{}")
        local bslot bdist boffline
        bslot=$(echo "$bsync"   | jq -r '.data.head_slot    // "?"')
        bdist=$(echo "$bsync"   | jq -r '.data.sync_distance // "?"')
        boffline=$(echo "$bsync" | jq -r '.data.el_offline   // false')
        if [ "$boffline" = "true" ]; then
            beacon_line=$(status_warn "running  slot ${bslot}  dist=${bdist}  ${RED}EL offline${RESET}")
            any_error=true
        elif [ "${bdist:-999}" -gt 10 ] 2>/dev/null; then
            beacon_line=$(status_warn "running  slot ${bslot}  dist=${YELLOW}${bdist}${RESET} (catching up)")
        else
            beacon_line=$(status_ok "running  slot ${CYAN}${bslot}${RESET}  dist=${bdist}")
        fi
    else
        beacon_line=$(status_err "not running")
        any_error=true
    fi
    printf "  %-14s %s\n" "l1-beacon:" "$beacon_line"

    # l1-validator
    local validator_line
    if docker ps --format '{{.Names}}' 2>/dev/null | grep -q '^l1-validator$'; then
        validator_line=$(status_ok "running")
    else
        validator_line=$(status_warn "not running (L1 will not produce new blocks)")
        any_error=true
    fi
    printf "  %-14s %s\n" "l1-validator:" "$validator_line"

    echo ""

    # ─────────────────────────────────────────────────────────────────────────
    # L2 NODE
    # ─────────────────────────────────────────────────────────────────────────
    printf "${BOLD}xlayer-node (L2)${RESET}\n"

    # Process
    local node_pid; node_pid=$(pgrep -x xlayer-node 2>/dev/null | head -1 || echo "")
    local node_line
    if [ -n "$node_pid" ]; then
        node_line=$(status_ok "running  pid ${CYAN}${node_pid}${RESET}")
    else
        node_line=$(status_err "not running")
        any_error=true
    fi
    printf "  %-14s %s\n" "process:" "$node_line"

    # L2 sync status
    if [ -n "$node_pid" ]; then
        local status_json
        status_json=$(curl -sf -X POST "$L2_ROLLUP_RPC_URL" \
            -H "Content-Type: application/json" \
            -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' 2>/dev/null || echo "{}")

        local unsafe safe finalized
        unsafe=$(echo "$status_json"    | jq -r '.result.unsafe_l2.number    // "0x0"')
        safe=$(echo "$status_json"      | jq -r '.result.safe_l2.number      // "0x0"')
        finalized=$(echo "$status_json" | jq -r '.result.finalized_l2.number // "0x0"')

        local unsafe_n safe_n final_n
        unsafe_n=$(hex_to_dec "$unsafe")
        safe_n=$(hex_to_dec "$safe")
        final_n=$(hex_to_dec "$finalized")

        local safe_lag final_lag
        safe_lag=$((unsafe_n - safe_n))
        final_lag=$((unsafe_n - final_n))

        # Block time (last 5 blocks)
        local blk_time="n/a"
        if [ "$unsafe_n" -gt 5 ] 2>/dev/null; then
            local ba_raw bb_raw ts_a ts_b
            ba_raw=$(cast block "$((unsafe_n - 5))" --rpc-url "$L2_RPC_URL" --json 2>/dev/null || echo "{}")
            bb_raw=$(cast block "$unsafe_n"         --rpc-url "$L2_RPC_URL" --json 2>/dev/null || echo "{}")
            ts_a=$(echo "$ba_raw" | jq -r '.timestamp // "0"' | xargs printf '%d\n' 2>/dev/null || echo "0")
            ts_b=$(echo "$bb_raw" | jq -r '.timestamp // "0"' | xargs printf '%d\n' 2>/dev/null || echo "0")
            if [ "$ts_a" -gt 0 ] && [ "$ts_b" -gt "$ts_a" ] 2>/dev/null; then
                local span=$((ts_b - ts_a))
                blk_time=$(awk "BEGIN {printf \"%.1f\", $span / 5}")s
            fi
        fi

        # L1 origin
        local l1_origin
        l1_origin=$(echo "$status_json" | jq -r '.result.unsafe_l2.l1origin.number // "?"')
        [ "$l1_origin" != "?" ] && l1_origin=$(hex_to_dec "$l1_origin")

        printf "  %-14s %s  (%s/block)\n" "unsafe head:" "${CYAN}${unsafe_n}${RESET}" "$blk_time"
        printf "  %-14s %s  (lag: %s blocks)\n" "safe head:" "${CYAN}${safe_n}${RESET}" "$(lag_badge "$safe_lag" 10 50)"
        printf "  %-14s %s  (lag: %s blocks)\n" "finalized:" "${CYAN}${final_n}${RESET}" "$(lag_badge "$final_lag" 50 200)"
        printf "  %-14s %s\n" "L1 origin:" "$l1_origin"

        if [ "$safe_lag" -gt 50 ] 2>/dev/null; then
            warn "High safe lag ($safe_lag blocks) — check op-batcher"
            any_error=true
        fi
        if [ "$unsafe_n" -eq 0 ] && [ "$safe_n" -eq 0 ]; then
            warn "Rollup RPC not responding ($L2_ROLLUP_RPC_URL)"
            any_error=true
        fi
    fi

    echo ""

    # ─────────────────────────────────────────────────────────────────────────
    # OP-BATCHER
    # ─────────────────────────────────────────────────────────────────────────
    printf "${BOLD}op-batcher${RESET}\n"

    local batcher_line batcher_running="no"
    if docker ps --format '{{.Names}}' 2>/dev/null | grep -q '^op-batcher$'; then
        batcher_running="yes"
        batcher_line=$(status_ok "running")
    else
        batcher_line=$(status_warn "not running — batches not submitted, safe head will lag")
        any_error=true
    fi
    printf "  %-14s %s\n" "container:" "$batcher_line"

    # Admin RPC
    local batcher_admin="down"
    if curl -sfS --max-time 2 http://127.0.0.1:8548/ >/dev/null 2>&1; then
        batcher_admin="up"
    fi
    printf "  %-14s %s\n" "admin RPC:" "$( [ "$batcher_admin" = "up" ] && echo -e "${GREEN}ok${RESET}" || echo -e "${YELLOW}no-response${RESET}" )"

    # L1 nonce + balance
    if [ -n "${OP_BATCHER_PRIVATE_KEY:-}" ]; then
        local baddr; baddr=$(cast wallet address "$OP_BATCHER_PRIVATE_KEY" 2>/dev/null || echo "")
        if [ -n "$baddr" ]; then
            local bnonce bbal_wei bbal_eth
            bnonce=$(cast nonce   --rpc-url "$L1_RPC_URL" "$baddr" 2>/dev/null || echo "?")
            bbal_wei=$(cast balance --rpc-url "$L1_RPC_URL" "$baddr" 2>/dev/null || echo "0")
            bbal_eth=$(awk "BEGIN {printf \"%.2f\", $bbal_wei / 1e18}" 2>/dev/null || echo "?")
            printf "  %-14s %s\n" "L1 nonce:" "$bnonce"
            printf "  %-14s %s ETH\n" "L1 balance:" "$bbal_eth"
        fi
    fi

    # ─────────────────────────────────────────────────────────────────────────
    # SUMMARY
    # ─────────────────────────────────────────────────────────────────────────
    printf "══════════════════════════════════════════\n"
    if [ "$any_error" = true ]; then
        warn "One or more components need attention (see above)"
    else
        ok "All systems healthy"
    fi
    echo ""
}

# ── Main ──────────────────────────────────────────────────────────────────────
if [ "$ONCE" = true ]; then
    snapshot
else
    info "Monitoring devnet (Ctrl+C to stop, refresh every ${INTERVAL}s)"
    echo ""
    while true; do
        clear 2>/dev/null || true
        snapshot
        sleep "$INTERVAL"
    done
fi
