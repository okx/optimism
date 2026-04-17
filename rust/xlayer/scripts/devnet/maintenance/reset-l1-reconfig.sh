#!/bin/bash
# reset-l1-reconfig.sh — Patch rollup.json with the reth genesis hash
#
# ╔══════════════════════════════════════════════════════════════════════════╗
# ║  ⚠️  NOT PART OF NORMAL SETUP — DO NOT RUN THIS AS A DEVELOPER          ║
# ║                                                                          ║
# ║  This script is for chain infrastructure maintenance ONLY.               ║
# ║  Run it ONLY after you have wiped the entire L1 chain data AND           ║
# ║  redeployed OP contracts from scratch (an exceptional, destructive op).  ║
# ║                                                                          ║
# ║  For normal first-time setup:  ./scripts/devnet/0-all.sh                ║
# ║  For L2 reset only:            ./scripts/devnet/maintenance/reset-l2.sh             ║
# ╚══════════════════════════════════════════════════════════════════════════╝
#
# When to run (both conditions must be true):
#   1. You wiped docker/l1/ and re-ran internal/start-l1.sh (new L1 genesis)
#   2. You redeployed OP contracts and have a new genesis.json + rollup.json
#
# What it does:
#   1. Computes the reth genesis hash (differs from op-geth due to number field)
#   2. Patches rollup.json genesis.l2.hash with the reth hash
#   3. Warns if stale L2 data exists
#
# After running: commit the updated config files and reset L2 data.
#
# Usage:
#   ./scripts/devnet/maintenance/reset-l1-reconfig.sh

set -e
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
check_deps jq

cd "$XLAYER_ROOT"

# ── Validate config files exist ───────────────────────────────────────────────
if [ ! -f "config/devnet/genesis.json" ]; then
    fail "config/devnet/genesis.json not found"
    echo "  This file should be committed to the repo."
    echo "  If you've just redeployed OP contracts, copy the new genesis-reth.json here:"
    echo "    cp /path/to/new/genesis-reth.json config/devnet/genesis.json"
    exit 1
fi

if [ ! -f "config/devnet/rollup.json" ]; then
    fail "config/devnet/rollup.json not found"
    echo "  This file should be committed to the repo."
    echo "  If you've just redeployed OP contracts, copy the new rollup.json here:"
    echo "    cp /path/to/new/rollup.json config/devnet/rollup.json"
    exit 1
fi

# ── Compute reth genesis hash ─────────────────────────────────────────────────
# reth computes a different genesis hash than op-geth because genesis.json
# sets "number" to FORK_BLOCK+1 rather than "0x0". kona will reject
# engine_forkchoiceUpdated if rollup.json has the wrong hash.
step "Computing reth genesis hash..."

HASH_TMPDIR=$(mktemp -d)
RETH_GENESIS_HASH=$(env -u OTEL_EXPORTER_OTLP_PROTOCOL \
    -u OTEL_METRICS_EXPORTER \
    -u OTEL_LOGS_EXPORTER \
    "$XLAYER_BINARY" init \
        --chain config/devnet/genesis.json \
        --datadir "$HASH_TMPDIR" 2>&1 \
    | grep "Genesis block written" | grep -oE '0x[0-9a-f]{64}' | head -1)
rm -rf "$HASH_TMPDIR"

if [ -z "$RETH_GENESIS_HASH" ]; then
    fail "Could not compute reth genesis hash — is $XLAYER_BINARY built?"
    echo "  Run: cargo build --release --package xlayer-node"
    exit 1
fi
ok "Reth genesis hash: $RETH_GENESIS_HASH"

# ── Patch rollup.json ─────────────────────────────────────────────────────────
step "Patching rollup.json genesis.l2.hash..."
CURRENT_HASH=$(jq -r '.genesis.l2.hash' config/devnet/rollup.json)
if [ "$CURRENT_HASH" = "$RETH_GENESIS_HASH" ]; then
    ok "rollup.json already has correct hash — nothing to patch"
else
    jq --arg h "$RETH_GENESIS_HASH" '.genesis.l2.hash = $h' \
        config/devnet/rollup.json > /tmp/rollup-patched.json
    mv /tmp/rollup-patched.json config/devnet/rollup.json
    ok "Patched: $CURRENT_HASH → $RETH_GENESIS_HASH"
    echo ""
    info "Commit the updated config files:"
    echo "  git add config/devnet/genesis.json config/devnet/rollup.json config/devnet/l1-genesis.json"
    echo "  git commit -m 'chore: update chain config for new L1 deployment'"
fi

# ── Warn if stale L2 data exists ─────────────────────────────────────────────
if [ -d "${XLAYER_DATA_DIR}" ] && [ -n "$(ls -A "${XLAYER_DATA_DIR}" 2>/dev/null)" ]; then
    echo ""
    warn "Stale L2 chain data found at: $XLAYER_DATA_DIR"
    warn "This data was built against a different genesis — it must be cleared."
    echo "  Run: ./scripts/devnet/maintenance/reset-l2.sh"
    echo "  Then: ./scripts/devnet/start-all.sh --no-build"
else
    echo ""
    ok "Setup complete. Start the devnet with:"
    echo "  ./scripts/devnet/start-all.sh --no-build"
fi
