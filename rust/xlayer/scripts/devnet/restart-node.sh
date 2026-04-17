#!/bin/bash
# restart-node.sh — Restart xlayer-node and op-batcher (L1 is left running)
#
# Stops op-batcher + xlayer-node, then starts them again. L1 is never touched.
#
# WHY NOT L1: stopping L1 docker containers and bringing them back up re-runs
# the init containers (l1-geth-remove-db etc.) which WIPE L1 chain data.
# L1 is persistent infrastructure — treat it like a database.
#
# For a fresh L2 (wiped chain data), run ./scripts/devnet/maintenance/reset-l2.sh first:
#   ./scripts/devnet/maintenance/reset-l2.sh && ./scripts/devnet/restart-node.sh --no-build
#
# To also restart L1 (rare — only if L1 is broken): stop-all.sh + start-all.sh manually.
#
# Usage:
#   ./scripts/devnet/restart-node.sh             # stop + rebuild + start
#   ./scripts/devnet/restart-node.sh --no-build  # stop + start (skip Rust build)

set -e
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

"$XLAYER_ROOT/scripts/devnet/stop-all.sh" --node
echo ""
"$XLAYER_ROOT/scripts/devnet/start-all.sh" "$@"
