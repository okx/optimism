#!/bin/bash
# internal/start-l1.sh — Initialise and start L1 devnet services
#
# Services: l1-geth + l1-beacon-chain + l1-validator
# Static configs:  docker/l1/execution/genesis-raw.json, docker/l1/consensus/config.yml
# Runtime data:    docker/l1/execution/geth/, docker/l1/consensus/beacondata/
#
# First start  → runs init containers to generate L1 chain data, then starts services.
# Resume       → skips init containers, simply resumes stopped containers (data preserved).
#
# After starting L1 for the first time, this script funds operator accounts
# (batcher, proposer, challenger) on L1 so they can submit transactions.
#
# Usage:
#   ./scripts/devnet/internal/start-l1.sh

set -e
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
check_deps cast curl jq docker

cd "$XLAYER_ROOT"

# ── Generate L1 JWT secret if not present ─────────────────────────────────────
L1_JWT="$XLAYER_ROOT/docker/l1/execution/jwtsecret"
if [ ! -f "$L1_JWT" ]; then
    step "Generating L1 JWT secret..."
    mkdir -p "$(dirname "$L1_JWT")"
    openssl rand -hex 32 > "$L1_JWT"
    ok "L1 JWT secret generated"
fi

# ── If already running, nothing to do ─────────────────────────────────────────
if cast bn --rpc-url "$L1_RPC_URL" &>/dev/null; then
    ok "L1 geth already running at $L1_RPC_URL (block $(cast bn --rpc-url "$L1_RPC_URL")) — skipping start"
    exit 0
fi

# ── Detect first start vs resume ──────────────────────────────────────────────
# There are three cases:
#   exited   → containers exist in our compose project but stopped — use "start"
#   absent   + data exists → data was migrated/copied in — use "up --no-deps"
#             (skip init containers which would WIPE the existing data)
#   absent   + no data     → true first start — use "up" (runs init containers)
L1_STATE=$(devnet_compose ps --format json l1-geth 2>/dev/null \
    | jq -r 'if type == "array" then .[0].State else .State end' 2>/dev/null || echo "absent")
GETH_DATA="$XLAYER_ROOT/docker/l1/execution/geth"

if [ "$L1_STATE" = "exited" ]; then
    step "Resuming L1 services (containers stopped, data preserved)..."
    devnet_compose start l1-geth l1-beacon-chain l1-validator
    FIRST_START=false
elif [ -d "$GETH_DATA" ]; then
    step "Starting L1 services (data exists, skipping chain init)..."
    # --no-deps skips init container dependencies — data is already in place
    devnet_compose up -d --no-deps l1-geth l1-beacon-chain l1-validator
    FIRST_START=false
else
    step "First start — initialising L1 chain data..."
    info "Init containers will: generate genesis.json, patch fork times, init geth db"
    devnet_compose up -d l1-geth l1-beacon-chain l1-validator
    FIRST_START=true
fi

# ── Wait for L1 geth ──────────────────────────────────────────────────────────
step "Waiting for L1 geth to be ready..."
until cast rpc eth_syncing --rpc-url "$L1_RPC_URL" 2>/dev/null | grep -q "false"; do
    echo -n "."
    sleep 1
done
echo ""
ok "L1 geth ready at $L1_RPC_URL (block $(cast bn --rpc-url "$L1_RPC_URL"))"

# ── Wait for L1 beacon chain ──────────────────────────────────────────────────
step "Waiting for L1 beacon chain..."
until curl -sf "$L1_BEACON_URL/eth/v1/node/syncing" 2>/dev/null \
      | jq -e '.data.is_syncing == false' > /dev/null 2>&1; do
    echo -n "."
    sleep 1
done
echo ""
ok "L1 beacon ready at $L1_BEACON_URL"

# ── Auto-align geth to beacon's EL head on resume ────────────────────────────
# After a stop/restart, Prysm may reload from a stale hot-block snapshot,
# leaving geth's canonical head AHEAD of beacon's EL head.
# Symptom: geth logs "Ignoring beacon update to old head" and L1 stalls.
#
# Fix: roll geth back to beacon's EL head via debug_setHead.
# In geth v1.16.7 archive mode, debug_setHead DELETES blocks above the target.
# Those orphan EL blocks (produced by geth but not embedded in any finalized
# beacon block) have no value and will be re-produced by normal L1 operation.
if [ "$FIRST_START" = false ]; then
    _GETH_HEAD=$(cast bn --rpc-url "$L1_RPC_URL" 2>/dev/null || echo "0")
    _BEACON_EL=$(curl -sf "$L1_BEACON_URL/eth/v2/beacon/blocks/head" \
        | jq -r '.data.message.body.execution_payload.block_hash // empty' 2>/dev/null || true)

    if [ -n "$_BEACON_EL" ] && [ "$_BEACON_EL" != "0x0000000000000000000000000000000000000000000000000000000000000000" ]; then
        _BEACON_EL_NUM=$(cast block "$_BEACON_EL" --rpc-url "$L1_RPC_URL" --json 2>/dev/null \
            | jq -r '.number // "0x0"' | xargs printf '%d\n' 2>/dev/null || echo "0")

        if [ "${_BEACON_EL_NUM:-0}" -gt 0 ] && [ "$_GETH_HEAD" -gt "$_BEACON_EL_NUM" ] 2>/dev/null; then
            warn "Beacon-geth EL desync detected: geth=$_GETH_HEAD, beacon EL head=$_BEACON_EL_NUM"
            warn "Rolling geth back to block $_BEACON_EL_NUM to realign with beacon..."
            cast rpc debug_setHead "$(cast --to-hex "$_BEACON_EL_NUM")" \
                --rpc-url "$L1_RPC_URL" > /dev/null
            ok "Geth realigned to block $_BEACON_EL_NUM — L1 will resume from there"
        else
            info "EL heads aligned (geth=$_GETH_HEAD)"
        fi
    fi
    unset _GETH_HEAD _BEACON_EL _BEACON_EL_NUM
fi

# ── Fund operator accounts (first start only) ─────────────────────────────────
if [ "$FIRST_START" = true ]; then
    if [ -z "${RICH_L1_PRIVATE_KEY:-}" ]; then
        warn "RICH_L1_PRIVATE_KEY not set — skipping operator account funding"
        warn "Fund batcher/proposer/challenger manually before running start-all.sh"
    else
        step "Funding operator accounts on L1..."
        for key_var in OP_BATCHER_PRIVATE_KEY OP_PROPOSER_PRIVATE_KEY OP_CHALLENGER_PRIVATE_KEY; do
            key="${!key_var:-}"
            if [ -z "$key" ]; then
                info "  $key_var not set — skipping"
                continue
            fi
            addr=$(cast wallet address "$key")
            cast send \
                --private-key "$RICH_L1_PRIVATE_KEY" \
                --value 100ether \
                "$addr" \
                --legacy \
                --rpc-url "$L1_RPC_URL" \
                --json > /dev/null
            ok "  Funded $addr ($key_var) with 100 ETH"
        done
    fi
fi

echo ""
ok "L1 devnet is up and ready"
info "L1 block: $(cast bn --rpc-url "$L1_RPC_URL")"
