#!/bin/bash
# stop-all.sh — Gracefully stop all devnet services (batcher + xlayer-node + L1)
#
# Stops in reverse order: op-batcher → xlayer-node → L1 services.
# Chain data, logs, and config are NOT touched — run start-all.sh to resume.
#
# Usage:
#   ./scripts/devnet/stop-all.sh            # stop everything
#   ./scripts/devnet/stop-all.sh --node     # stop xlayer-node + batcher only (keep L1 running)
#   ./scripts/devnet/stop-all.sh --l1       # stop L1 services only (xlayer-toolkit or your L1)

set -e
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

STOP_NODE=true
STOP_L1=true

# ── Parse flags ───────────────────────────────────────────────────────────────
for arg in "$@"; do
    case "$arg" in
        --node) STOP_L1=false  ;;
        --l1)   STOP_NODE=false ;;
        --help|-h)
            echo "Usage: $0 [--node | --l1]"
            echo "  (no flags)  Stop everything: op-batcher + xlayer-node + L1"
            echo "  --node      Stop xlayer-node + batcher (keep L1 running)"
            echo "  --l1        Stop L1 services only"
            exit 0
            ;;
    esac
done

cd "$XLAYER_ROOT"

# ── Step 1: Stop op-batcher ───────────────────────────────────────────────────
if [ "$STOP_NODE" = true ]; then
    step "Stopping op-batcher..."
    devnet_compose stop op-batcher 2>/dev/null || true
    ok "op-batcher stopped"
fi

# ── Step 2: Stop xlayer-node ─────────────────────────────────────────────────
if [ "$STOP_NODE" = true ]; then
    if pgrep -x xlayer-node &>/dev/null; then
        step "Stopping xlayer-node..."
        pkill -TERM -x xlayer-node || true
        # Give it up to 10s to exit cleanly before forcing
        for i in $(seq 1 10); do
            pgrep -x xlayer-node &>/dev/null || break
            sleep 1
        done
        if pgrep -x xlayer-node &>/dev/null; then
            warn "xlayer-node did not exit cleanly — sending SIGKILL"
            pkill -KILL -x xlayer-node || true
        fi
        ok "xlayer-node stopped"
    else
        info "xlayer-node is not running"
    fi
fi

# ── Step 3: Stop L1 services ─────────────────────────────────────────────────
# Stop order: validator first (no new attestations/proposals), then beacon
# (flushes hot-block database on SIGTERM), then geth.
#
# On restart, Prysm re-processes its persisted hot blocks and sends
# engine_forkchoiceUpdated with the correct head hash — geth accepts and
# L1 resumes from where it left off.
#
# NOTE: Do NOT use debug_setHead before stopping geth. In geth v1.16.7
# archive mode, debug_setHead DELETES blocks above the target from the DB.
# Beacon's hot blocks then reference hashes geth no longer has → L1 stalls
# permanently on restart.
if [ "$STOP_L1" = true ]; then
    L1_RUNNING=$(docker ps --format "{{.Names}}" | grep -E "^(l1-geth|l1-beacon-chain|l1-validator)$" || true)
    if [ -n "$L1_RUNNING" ]; then
        step "Stopping L1 services (validator → geth → beacon)..."

        # 3a: Stop validator — no new proposals or attestations from this point.
        docker stop l1-validator 2>/dev/null || true

        # 3b: Stop geth BEFORE beacon — ensures beacon's last successful FCU
        #     (which is its last known EL head) matches geth's canonical head
        #     when Prysm flushes its hot-block DB in step 3c.
        #     Any EL blocks geth built after beacon's last flush are orphaned
        #     (no finalized beacon block references them). internal/start-l1.sh will
        #     auto-detect and fix any remaining EL desync via debug_setHead.
        docker stop l1-geth 2>/dev/null || true

        # 3c: Stop beacon with extended timeout — Prysm needs time to flush its
        #     hot-block DB on SIGTERM. Default 10s is often not enough; 60s
        #     ensures in-flight slot processing completes and blocks persist.
        docker stop --time 60 l1-beacon-chain 2>/dev/null || true

        ok "L1 services stopped (data volumes preserved)"
    else
        info "L1 services are not running"
    fi
fi

echo ""
ok "All services stopped."
info "Chain data and config are intact."
info "Resume with: ./scripts/devnet/start-all.sh --no-build"
