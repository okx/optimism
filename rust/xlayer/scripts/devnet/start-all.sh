#!/bin/bash
# start-all.sh — Bring up the full devnet in one command
#
# If L1 is not running, starts it automatically (same as internal/start-l1.sh).
#
# Order:
#   1. Ensure L1 is up (start if needed via internal/start-l1.sh)
#   2. Build xlayer-node (skipped with --no-build)
#   3. Start xlayer-node in background
#   4. Wait for L2 RPC ready
#   5. Start op-batcher via docker/docker-compose.devnet.yml
#
# Logs:
#   logs/xlayer-node.log  — xlayer-node stdout
#
# Usage:
#   ./scripts/devnet/start-all.sh             # full start (builds + starts everything)
#   ./scripts/devnet/start-all.sh --no-build  # skip Rust build

set -e
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
check_deps cast curl jq docker cargo

NO_BUILD=false
for arg in "$@"; do [[ "$arg" == "--no-build" ]] && NO_BUILD=true; done

cd "$XLAYER_ROOT"
mkdir -p logs

# ── Step 1: Ensure L1 is up (start if not running) ────────────────────────────
step "Step 1: Checking L1..."
if cast bn --rpc-url "$L1_RPC_URL" &>/dev/null; then
    ok "L1 already running at $L1_RPC_URL (block $(cast bn --rpc-url "$L1_RPC_URL"))"
else
    info "L1 not running — starting L1..."
    bash "$(dirname "${BASH_SOURCE[0]}")/internal/start-l1.sh"
fi

# ── Step 2: Build ─────────────────────────────────────────────────────────────
ensure_jwt

if [ "$NO_BUILD" = false ]; then
    step "Step 2: Building xlayer-node..."
    cargo build --release --package xlayer-node
    ok "Build complete"
else
    info "Step 2: Skipping build (--no-build)"
fi

# ── Step 3: Start xlayer-node in background ───────────────────────────────────
step "Step 3: Starting xlayer-node (background)..."

if xlayer_node_running; then
    warn "xlayer-node already responding at $L2_RPC_URL — skipping start"
else
    # XLAYER_LOG_LEVEL defaults to include engine_bridge=debug so load-test.sh
    # can parse ENGINE BRIDGE latencies from logs/reth/<chain_id>/reth.log.
    XLAYER_LOG_LEVEL="${XLAYER_LOG_LEVEL:-info,engine_bridge=debug}"

    # Unset OTEL env vars — reth rejects values like "http/json" that other tools set.
    env -u OTEL_EXPORTER_OTLP_PROTOCOL \
        -u OTEL_METRICS_EXPORTER \
        -u OTEL_LOGS_EXPORTER \
    "$XLAYER_BINARY" node \
        --xlayer-config "$XLAYER_CONFIG" \
        --chain "$XLAYER_CHAIN_CONFIG" \
        --datadir "$XLAYER_DATA_DIR" \
        --http --http.addr 0.0.0.0 --http.port 8123 \
        --http.api "$XLAYER_HTTP_API" \
        --ws --ws.addr 0.0.0.0 --ws.port 7546 \
        --authrpc.addr 127.0.0.1 --authrpc.port 8552 \
        --authrpc.jwtsecret "$JWT_SECRET_FILE" \
        --log.stdout.filter "$XLAYER_LOG_LEVEL" \
        --log.file.directory logs/reth \
        >> logs/xlayer-node.log 2>&1 &

    XLAYER_PID=$!
    info "xlayer-node started (pid $XLAYER_PID) — tail -f logs/xlayer-node.log"
fi

# ── Step 4: Wait for L2 RPC ───────────────────────────────────────────────────
wait_for_rpc "$L2_RPC_URL" "xlayer-node L2 RPC" 120

# ── Step 5: Start op-batcher ──────────────────────────────────────────────────
step "Step 5: Starting op-batcher..."

# Kill any stale batcher containers (two batchers sharing a private key corrupt L1 nonces).
STALE=$(docker ps --format '{{.Names}}' | grep -i batcher | grep -v '^op-batcher$' || true)
if [ -n "$STALE" ]; then
    warn "Stale batcher container(s) found: $STALE — removing before start"
    echo "$STALE" | xargs docker stop 2>/dev/null || true
    echo "$STALE" | xargs docker rm   2>/dev/null || true
fi

devnet_compose up -d op-batcher
ok "op-batcher started (max-channel-duration=${BATCHER_MAX_CHANNEL_DURATION}, num-confirmations=${BATCHER_NUM_CONFIRMATIONS})"

echo ""
ok "Devnet is up."

# ── Batcher lag status ────────────────────────────────────────────────────────
_SYNC=$(curl -sf -X POST "$L2_ROLLUP_RPC_URL" \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' 2>/dev/null || echo "{}")
_SAFE=$(echo "$_SYNC"   | jq -r '.result.safe_l2.number   // "0x0"' | xargs printf '%d\n' 2>/dev/null || echo 0)
_UNSAFE=$(echo "$_SYNC" | jq -r '.result.unsafe_l2.number // "0x0"' | xargs printf '%d\n' 2>/dev/null || echo 0)
_LAG=$(( _UNSAFE - _SAFE ))
if [ "$_LAG" -gt 50 ] 2>/dev/null; then
    warn "Safe lag: $_LAG blocks. op-batcher is catching up — run test-tx.sh after lag drops."
else
    info "Safe lag: $_LAG blocks (healthy)"
fi
unset _SYNC _SAFE _UNSAFE _LAG

info "Verify with:"
echo "  ./scripts/devnet/health-check.sh --once      # snapshot: all components"
echo "  ./scripts/devnet/test-tx.sh --verbose        # TX through unsafe→safe→finalized"
echo "  ./scripts/devnet/perf-baseline.sh --count 50 # TPS baseline"
echo "  tail -f logs/xlayer-node.log                   # node logs"
