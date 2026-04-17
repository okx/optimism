#!/bin/bash
# 0-all.sh — Full devnet start in one command (first-run safe)
#
# Handles everything: prerequisite checks, first-run setup, build, start.
# Safe to run repeatedly — skips steps that are already done.
#
# Usage:
#   ./scripts/devnet/0-all.sh             # first time or any time
#   ./scripts/devnet/0-all.sh --no-build  # skip Rust build (binary already built)
#
# What this does on first run:
#   1. Checks required tools (cargo, cast, docker, jq, curl, openssl)
#   2. Auto-creates config/devnet/.env from .env.example if missing
#   3. Auto-generates config/devnet/jwt.txt if missing
#   4. Builds xlayer-node (unless --no-build)
#   5. Starts L1, xlayer-node, and op-batcher
#
# Prerequisites (one-time installs):
#   Rust:    curl https://sh.rustup.rs -sSf | sh
#   Foundry: curl -L https://foundry.paradigm.xyz | bash && foundryup
#   Docker:  https://docs.docker.com/get-docker/
#   jq:      brew install jq   (macOS)  or  apt install jq  (Linux)

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
XLAYER_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# ── Parse flags ───────────────────────────────────────────────────────────────
NO_BUILD=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-build) NO_BUILD=true; shift ;;
        --help|-h)
            sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \?//'
            exit 0
            ;;
        *) echo "Unknown flag: $1"; exit 1 ;;
    esac
done

source "$SCRIPT_DIR/lib.sh"

echo ""
echo -e "${BOLD}xlayer-node Devnet${RESET}"
echo "══════════════════════════════════════════"
echo ""

# ── Step 1: Check prerequisites ───────────────────────────────────────────────
step "Step 1: Checking prerequisites..."

tool_hint() {
    case "$1" in
        cargo)   echo "curl https://sh.rustup.rs -sSf | sh" ;;
        cast)    echo "curl -L https://foundry.paradigm.xyz | bash && foundryup" ;;
        docker)  echo "https://docs.docker.com/get-docker/" ;;
        jq)      echo "brew install jq  (macOS) / apt install jq  (Linux)" ;;
        curl)    echo "pre-installed on most systems" ;;
        openssl) echo "brew install openssl  (macOS) / apt install openssl  (Linux)" ;;
        *)       echo "see project docs" ;;
    esac
}

MISSING_TOOLS=()
for cmd in cargo cast docker jq curl openssl; do
    command -v "$cmd" &>/dev/null || MISSING_TOOLS+=("$cmd")
done

if [ ${#MISSING_TOOLS[@]} -gt 0 ]; then
    fail "Missing required tools:"
    for t in "${MISSING_TOOLS[@]}"; do
        printf "  ✗ %-10s  install: %s\n" "$t" "$(tool_hint "$t")"
    done
    echo ""
    exit 1
fi
ok "All prerequisites installed"

# ── Step 2: First-run setup ───────────────────────────────────────────────────
step "Step 2: Checking config files..."

ENV_FILE="$XLAYER_ROOT/config/devnet/.env"
ENV_EXAMPLE="$XLAYER_ROOT/config/devnet/.env.example"
JWT_FILE="$XLAYER_ROOT/config/devnet/jwt.txt"

SETUP_DONE=false

# Auto-create .env from example
if [ ! -f "$ENV_FILE" ]; then
    if [ ! -f "$ENV_EXAMPLE" ]; then
        fail "config/devnet/.env.example not found — is this a fresh clone? Check git status."
        exit 1
    fi
    cp "$ENV_EXAMPLE" "$ENV_FILE"
    ok "Created config/devnet/.env from .env.example (devnet defaults work out of the box)"
    SETUP_DONE=true
fi

# Auto-generate JWT secret
if [ ! -f "$JWT_FILE" ]; then
    mkdir -p "$(dirname "$JWT_FILE")"
    openssl rand -hex 32 > "$JWT_FILE"
    ok "Generated config/devnet/jwt.txt"
    SETUP_DONE=true
fi

# Validate committed config files (should never be missing post-clone)
MISSING_CONFIGS=()
[ ! -f "$XLAYER_ROOT/config/devnet/genesis.json" ] && MISSING_CONFIGS+=("config/devnet/genesis.json")
[ ! -f "$XLAYER_ROOT/config/devnet/rollup.json"  ] && MISSING_CONFIGS+=("config/devnet/rollup.json")
[ ! -f "$XLAYER_ROOT/config/devnet/xlayer-node.toml" ] && MISSING_CONFIGS+=("config/devnet/xlayer-node.toml")

if [ ${#MISSING_CONFIGS[@]} -gt 0 ]; then
    fail "Missing committed config files (should be in git):"
    for f in "${MISSING_CONFIGS[@]}"; do
        echo "  ✗ $f"
    done
    echo ""
    info "Run: git status  to check if these files exist in the repo"
    exit 1
fi

if [ "$SETUP_DONE" = false ]; then
    ok "Config files present"
fi

# ── Step 3: Build ─────────────────────────────────────────────────────────────
if [ "$NO_BUILD" = false ]; then
    step "Step 3: Building xlayer-node..."
    cargo build --release --package xlayer-node
    ok "Build complete"
else
    XLAYER_BINARY="$XLAYER_ROOT/target/release/xlayer-node"
    if [ ! -f "$XLAYER_BINARY" ]; then
        fail "Binary not found at $XLAYER_BINARY"
        info "Run without --no-build to compile it first"
        exit 1
    fi
    info "Step 3: Skipping build (using existing binary at target/release/xlayer-node)"
fi

# ── Step 4: Start devnet ──────────────────────────────────────────────────────
step "Step 4: Starting devnet..."
echo ""
"$SCRIPT_DIR/start-all.sh" --no-build
