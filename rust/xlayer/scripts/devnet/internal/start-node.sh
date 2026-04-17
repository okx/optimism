#!/bin/bash
# internal/start-node.sh — Build and start xlayer-node in the foreground (debug mode)
#
# Runs xlayer-node attached to the terminal so you see logs live.
# Open a second terminal for test-tx.sh or health-check.sh.
# For background start (normal devnet use), use start-all.sh instead.
#
# Usage:
#   ./scripts/devnet/internal/start-node.sh            # build + run
#   ./scripts/devnet/internal/start-node.sh --no-build # skip build (use existing binary)

set -e
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
check_deps cargo curl jq

NO_BUILD=false
for arg in "$@"; do
    [[ "$arg" == "--no-build" ]] && NO_BUILD=true
done

# Always run from project root so relative config paths work
cd "$XLAYER_ROOT"

# ── JWT secret ────────────────────────────────────────────────────────────────
ensure_jwt

# ── Build ─────────────────────────────────────────────────────────────────────
if [ "$NO_BUILD" = false ]; then
    step "Building xlayer-node (release)..."
    cargo build --release --package xlayer-node
    ok "Build complete: $XLAYER_BINARY"
else
    if [ ! -f "$XLAYER_BINARY" ]; then
        fail "Binary not found: $XLAYER_BINARY — run without --no-build first"
        exit 1
    fi
    info "Skipping build — using existing binary"
fi

# ── Pre-flight checks ─────────────────────────────────────────────────────────
step "Checking L1 connectivity..."
if ! cast bn --rpc-url "$L1_RPC_URL" &>/dev/null; then
    fail "L1 RPC not reachable at $L1_RPC_URL"
    echo "  Run: ./scripts/devnet/internal/start-l1.sh"
    exit 1
fi
ok "L1 RPC reachable (block $(cast bn --rpc-url "$L1_RPC_URL"))"

# ── Data directory ────────────────────────────────────────────────────────────
mkdir -p "$XLAYER_DATA_DIR"
mkdir -p logs

# ── Launch xlayer-node ────────────────────────────────────────────────────────
step "Starting xlayer-node..."
info "  Config:  $XLAYER_CONFIG"
info "  Chain:   $XLAYER_CHAIN_CONFIG"
info "  Datadir: $XLAYER_DATA_DIR"
info "  L2 RPC:  $L2_RPC_URL"
info "  Rollup:  $L2_ROLLUP_RPC_URL"
info "  Logs:    logs/xlayer-node.log  (file: logs/reth/<chain_id>/reth.log)"

# XLAYER_LOG_LEVEL defaults to "info,engine_bridge=debug" so load-test.sh can
# parse ENGINE BRIDGE latencies from logs/reth/<chain_id>/reth.log.
# Override: XLAYER_LOG_LEVEL=info ./scripts/devnet/internal/start-node.sh
XLAYER_LOG_LEVEL="${XLAYER_LOG_LEVEL:-info,engine_bridge=debug}"

# Unset OTEL env vars — reth rejects values like "http/json" that other tools set.
env -u OTEL_EXPORTER_OTLP_PROTOCOL \
    -u OTEL_METRICS_EXPORTER \
    -u OTEL_LOGS_EXPORTER \
"$XLAYER_BINARY" node \
    --xlayer-config "$XLAYER_CONFIG" \
    --chain "$XLAYER_CHAIN_CONFIG" \
    --datadir "$XLAYER_DATA_DIR" \
    --http \
    --http.addr 0.0.0.0 \
    --http.port 8123 \
    --http.api "$XLAYER_HTTP_API" \
    --ws \
    --ws.addr 0.0.0.0 \
    --ws.port 7546 \
    --authrpc.addr 127.0.0.1 \
    --authrpc.port 8552 \
    --authrpc.jwtsecret "$JWT_SECRET_FILE" \
    --log.stdout.filter "$XLAYER_LOG_LEVEL" \
    --log.file.directory logs/reth \
    2>&1 | tee logs/xlayer-node.log
