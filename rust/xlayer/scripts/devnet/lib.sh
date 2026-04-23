#!/bin/bash
# lib.sh — shared helpers for xlayer devnet scripts
# Source this file: source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
#
# After sourcing, the following are available:
#
# Variables:
#   XLAYER_ROOT          — absolute path to project root
#   XLAYER_BINARY        — path to compiled xlayer-node binary
#   XLAYER_CONFIG        — path to config/devnet/xlayer-node.toml
#   XLAYER_CHAIN_CONFIG  — path to config/devnet/genesis.json
#   ENV_FILE             — path to config/devnet/.env
#   L1_RPC_URL           — L1 execution RPC (default: http://localhost:8545)
#   L2_RPC_URL           — L2 execution RPC (default: http://localhost:8123)
#   L2_ROLLUP_RPC_URL    — rollup/consensus RPC, served by kona (default: http://localhost:9545)
#   L1_BEACON_URL        — L1 beacon chain RPC (default: http://localhost:3500)
#   XLAYER_DATA_DIR      — chain data directory (default: /tmp/xlayer-data)
#   JWT_SECRET_FILE      — path to JWT secret for Engine API auth
#
# Functions:
#   ok/fail/warn/info/step  — coloured log helpers
#   check_deps <cmd...>     — exit with message if any tool is missing
#   wait_for_rpc <url> [label] [timeout]  — poll until JSON-RPC endpoint responds
#   ensure_jwt              — generate JWT secret if not present
#   xlayer_node_running     — returns true if L2 RPC is responding
#   l2_block_number         — current L2 block as decimal
#   l2_sync_status          — {unsafe, safe, finalized} heads as JSON

# Resolve project root (two levels up from scripts/devnet/)
XLAYER_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ENV_FILE="$XLAYER_ROOT/config/devnet/.env"

# Load .env if present
if [ -f "$ENV_FILE" ]; then
    set -a
    # shellcheck source=/dev/null
    source "$ENV_FILE"
    set +a
else
    echo "[lib] WARNING: $ENV_FILE not found — copy config/devnet/.env.example to config/devnet/.env"
fi

# Defaults (override in .env)
L1_RPC_URL="${L1_RPC_URL:-http://localhost:8545}"
L2_RPC_URL="${L2_RPC_URL:-http://localhost:8123}"
L2_ROLLUP_RPC_URL="${L2_ROLLUP_RPC_URL:-http://localhost:9545}"
L1_BEACON_URL="${L1_BEACON_URL:-http://localhost:3500}"
XLAYER_DATA_DIR="${XLAYER_DATA_DIR:-/tmp/xlayer-data}"
XLAYER_LOG_LEVEL="${XLAYER_LOG_LEVEL:-info}"
XLAYER_HTTP_API="${XLAYER_HTTP_API:-eth,net,web3,miner,debug}"
JWT_SECRET_FILE="${JWT_SECRET_FILE:-$XLAYER_ROOT/config/devnet/jwt.txt}"
BATCHER_MAX_CHANNEL_DURATION="${BATCHER_MAX_CHANNEL_DURATION:-10}"
BATCHER_NUM_CONFIRMATIONS="${BATCHER_NUM_CONFIRMATIONS:-1}"

XLAYER_BINARY="$XLAYER_ROOT/target/release/xlayer-node"
XLAYER_CONFIG="$XLAYER_ROOT/config/devnet/xlayer-node.toml"
XLAYER_CHAIN_CONFIG="$XLAYER_ROOT/config/devnet/genesis.json"
XLAYER_DEVNET_COMPOSE="$XLAYER_ROOT/docker/docker-compose.devnet.yml"

# Port numbers (used by docker-compose.devnet.yml for host.docker.internal URLs)
L1_HTTP_PORT="${L1_HTTP_PORT:-8545}"
L2_HTTP_PORT="${L2_HTTP_PORT:-8123}"
L2_ROLLUP_PORT="${L2_ROLLUP_PORT:-9545}"

# Convenience wrapper: docker compose with our compose file + env file
# Usage: devnet_compose up -d op-batcher
devnet_compose() {
    docker compose \
        -f "$XLAYER_DEVNET_COMPOSE" \
        --env-file "$ENV_FILE" \
        "$@"
}

# ── colour helpers ────────────────────────────────────────────────────────────
RED=$'\033[0;31m'; GREEN=$'\033[0;32m'; YELLOW=$'\033[1;33m'
CYAN=$'\033[0;36m'; BOLD=$'\033[1m'; RESET=$'\033[0m'

ok()   { echo -e "${GREEN}✅ $*${RESET}"; }
fail() { echo -e "${RED}❌ $*${RESET}"; }
warn() { echo -e "${YELLOW}⚠️  $*${RESET}"; }
info() { echo -e "${CYAN}ℹ  $*${RESET}"; }
step() { echo -e "${BOLD}── $*${RESET}"; }

# ── dependency checks ─────────────────────────────────────────────────────────
check_deps() {
    local missing=()
    for cmd in "$@"; do
        if ! command -v "$cmd" &>/dev/null; then
            missing+=("$cmd")
        fi
    done
    if [ ${#missing[@]} -gt 0 ]; then
        fail "Missing required tools: ${missing[*]}"
        echo "  cast:  https://book.getfoundry.sh/getting-started/installation"
        echo "  jq:    brew install jq"
        echo "  curl:  brew install curl"
        exit 1
    fi
}

# ── RPC wait helpers ──────────────────────────────────────────────────────────

# Wait until an eth JSON-RPC endpoint responds to eth_blockNumber.
# Usage: wait_for_rpc <url> [label] [timeout_seconds]
wait_for_rpc() {
    local url="$1"
    local label="${2:-$url}"
    local timeout="${3:-120}"
    local elapsed=0
    step "Waiting for $label to be ready..."
    until curl -sf -X POST -H "Content-Type: application/json" \
        -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
        "$url" | grep -q '"result"'; do
        if [ $elapsed -ge $timeout ]; then
            fail "Timeout waiting for $label after ${timeout}s"
            exit 1
        fi
        sleep 1
        elapsed=$((elapsed + 1))
        echo -n "."
    done
    echo ""
    ok "$label is ready (${elapsed}s)"
}

# ── block/sync helpers ────────────────────────────────────────────────────────

# Returns current block number as decimal
l2_block_number() {
    cast bn --rpc-url "$L2_RPC_URL" 2>/dev/null || echo "0"
}

# Returns sync status as JSON: {unsafe, safe, finalized}
l2_sync_status() {
    curl -sf -X POST "$L2_ROLLUP_RPC_URL" \
        -H "Content-Type: application/json" \
        -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' \
        | jq '{unsafe: .result.unsafe_l2.number, safe: .result.safe_l2.number, finalized: .result.finalized_l2.number}'
}

# Returns the L2 block number a TX was included in (0 = not yet included)
tx_block_number() {
    local tx_hash="$1"
    cast receipt --rpc-url "$L2_RPC_URL" "$tx_hash" 2>/dev/null \
        | grep blockNumber | awk '{print $2}' || echo "0"
}

# ── JWT helper ────────────────────────────────────────────────────────────────

# Generate a JWT secret if one doesn't exist
ensure_jwt() {
    if [ ! -f "$JWT_SECRET_FILE" ]; then
        step "Generating JWT secret at $JWT_SECRET_FILE"
        openssl rand -hex 32 > "$JWT_SECRET_FILE"
        ok "JWT secret generated"
    fi
}

# ── process helpers ───────────────────────────────────────────────────────────

# Check if xlayer-node is running (by listening on L2_RPC_URL port)
xlayer_node_running() {
    curl -sf -X POST -H "Content-Type: application/json" \
        -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
        "$L2_RPC_URL" | grep -q '"result"' 2>/dev/null
}
