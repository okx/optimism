#!/bin/bash
# reset-l2.sh — Wipe L2 chain data and logs (L1 is NOT touched)
#
# By default removes: chain data (MDBX), logs, xlayer-node process.
# Does NOT remove: L1 containers, config files, .env, binary.
#
# Levels:
#   ./scripts/devnet/maintenance/reset-l2.sh             # chain data + logs (safe to re-run)
#   ./scripts/devnet/maintenance/reset-l2.sh --secrets   # also remove jwt.txt (re-generated on next start)
#   ./scripts/devnet/maintenance/reset-l2.sh --nuke      # DESTRUCTIVE: also removes .env — requires typing
#                                            # "yes-delete-my-config" at the prompt
#
# After reset, start fresh L2 with: ./scripts/devnet/start-all.sh --no-build

set -e
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

CLEAN_SECRETS=false
CLEAN_ENV=false

for arg in "$@"; do
    case "$arg" in
        --secrets) CLEAN_SECRETS=true ;;
        --nuke)    CLEAN_SECRETS=true; CLEAN_ENV=true ;;
        --help|-h)
            echo "Usage: $0 [--secrets] [--nuke]"
            echo "  (no flags)   Remove chain data + logs"
            echo "  --secrets    Also remove jwt.txt (re-generated on next start)"
            echo "  --nuke       DESTRUCTIVE: also remove .env (prompted for confirmation)"
            exit 0
            ;;
    esac
done

# ── Guard: --nuke requires two explicit confirmations ────────────────────────
if [ "$CLEAN_ENV" = true ]; then
    warn "WARNING: --nuke will delete config/devnet/.env"
    warn "You will need to reconfigure from .env.example before the next start."
    echo ""
    printf "Type  yes-delete-my-config  to confirm: "
    read -r CONFIRM1
    if [ "$CONFIRM1" != "yes-delete-my-config" ]; then
        fail "Aborted — first confirmation did not match."
        exit 1
    fi
    printf "Type it again to be sure: "
    read -r CONFIRM2
    if [ "$CONFIRM2" != "yes-delete-my-config" ]; then
        fail "Aborted — second confirmation did not match."
        exit 1
    fi
fi

cd "$XLAYER_ROOT"

step "Cleaning xlayer-node runtime state..."

# ── Kill xlayer-node if running ───────────────────────────────────────────────
if pgrep -x xlayer-node &>/dev/null; then
    step "Stopping xlayer-node..."
    pkill -x xlayer-node || true
    sleep 1
    ok "xlayer-node stopped"
fi

# ── Chain data (MDBX) ─────────────────────────────────────────────────────────
DATA_DIR="${XLAYER_DATA_DIR:-/tmp/xlayer-data}"
if [ -d "$DATA_DIR" ]; then
    step "Removing chain data: $DATA_DIR"
    rm -rf "$DATA_DIR"
    ok "Chain data removed"
else
    info "Chain data not found at $DATA_DIR — nothing to remove"
fi

# ── Logs ─────────────────────────────────────────────────────────────────────
step "Removing logs..."
rm -f  logs/xlayer-node.log
rm -rf logs/reth/
rm -rf logs/kona/
# Restore .gitkeep so the logs/ directory stays tracked
touch logs/.gitkeep
ok "Logs cleared"

# ── JWT secret (optional) ────────────────────────────────────────────────────
if [ "$CLEAN_SECRETS" = true ]; then
    if [ -f "$JWT_SECRET_FILE" ]; then
        step "Removing JWT secret: $JWT_SECRET_FILE"
        rm -f "$JWT_SECRET_FILE"
        ok "JWT secret removed (will be regenerated on next start)"
    fi
fi

# ── .env (optional) ──────────────────────────────────────────────────────────
if [ "$CLEAN_ENV" = true ]; then
    if [ -f "$ENV_FILE" ]; then
        step "Removing .env: $ENV_FILE"
        rm -f "$ENV_FILE"
        ok ".env removed — copy config/devnet/.env.example to config/devnet/.env to reconfigure"
    fi
fi

echo ""
ok "Clean complete."
info "What was kept:"
echo "  config/devnet/*.json          (chain config — committed)"
echo "  config/devnet/xlayer-node.toml (node config — committed)"
[ "$CLEAN_SECRETS" = false ] && echo "  config/devnet/jwt.txt         (JWT secret)"
[ "$CLEAN_ENV" = false ]     && echo "  config/devnet/.env            (your env config)"
echo "  target/                        (Rust build cache)"
echo ""
info "Start fresh with: ./scripts/devnet/start-all.sh --no-build"
