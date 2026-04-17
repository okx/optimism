#!/bin/bash
# redeploy-op-contracts.sh — Deploy OP contracts on a fresh L1 and regenerate
#                             config/devnet/genesis.json + config/devnet/rollup.json
#
# ╔══════════════════════════════════════════════════════════════════════════════╗
# ║  Run this ONLY after a full L1 wipe (docker/l1/ data deleted and            ║
# ║  internal/start-l1.sh re-run). Deployer key must be funded on the new L1.  ║
# ╚══════════════════════════════════════════════════════════════════════════════╝
#
# What it does:
#   1. Deploys owner Transactor contract
#   2. Deploys superchain + implementation contracts via op-deployer
#   3. Deploys OP chain contracts and generates genesis.json + rollup.json
#   4. Copies genesis.json → config/devnet/genesis.json
#   5. Copies rollup.json  → config/devnet/rollup.json
#   6. Patches rollup.json with the correct reth genesis hash
#   7. Resets L2 data (stale after L1 change)
#
# After this script: start fresh with ./scripts/devnet/start-all.sh --no-build
#
# Usage:
#   ./scripts/devnet/maintenance/redeploy-op-contracts.sh

set -e
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
check_deps jq docker cast

cd "$XLAYER_ROOT"

# ── Config ────────────────────────────────────────────────────────────────────
OP_CONTRACTS_IMAGE="${OP_CONTRACTS_IMAGE_TAG:-op-contracts:latest}"
OP_STACK_IMAGE="${OP_STACK_IMAGE_TAG:-op-stack:latest}"
DOCKER_NETWORK="${DOCKER_NETWORK:-xlayer-devnet}"
L1_RPC_IN_DOCKER="http://l1-geth:8545"
CHAIN_ID="${L2_CHAIN_ID:-195}"

# Deployer / owner accounts (Hardhat default accounts — devnet only)
DEPLOYER_PRIVATE_KEY="${DEPLOYER_PRIVATE_KEY:-0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80}"
OP_CHALLENGER_PRIVATE_KEY="${OP_CHALLENGER_PRIVATE_KEY:-0x8b3a350cf5c34c9194ca9aa3f146b2b9afed22cd83d3c5f6a3f2f243ce220c01}"
ADMIN_OWNER_ADDRESS="${ADMIN_OWNER_ADDRESS:-0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266}"

# Timing parameters (permissive for devnet)
CHALLENGE_PERIOD_SECONDS=10
WITHDRAWAL_DELAY_SECONDS=20
DISPUTE_GAME_FINALITY_DELAY_SECONDS=5
TEMP_CLOCK_EXTENSION=5
TEMP_MAX_CLOCK_DURATION=20

# Working directories
CONTRACTS_DIR="$XLAYER_ROOT/docker/op-contracts"
DEPLOY_WORKDIR="$CONTRACTS_DIR/deploy-state"

# ── Pre-flight ────────────────────────────────────────────────────────────────
step "Pre-flight checks..."

if ! cast bn --rpc-url "$L1_RPC_URL" &>/dev/null; then
    fail "L1 RPC not reachable at $L1_RPC_URL"
    info "Run: ./scripts/devnet/internal/start-l1.sh"
    exit 1
fi
ok "L1 reachable (block $(cast bn --rpc-url "$L1_RPC_URL"))"

# Check deployer is funded on L1
DEPLOYER_ADDR=$(cast wallet address "$DEPLOYER_PRIVATE_KEY")
DEPLOYER_BAL=$(cast balance --rpc-url "$L1_RPC_URL" "$DEPLOYER_ADDR" --ether 2>/dev/null || echo "0")
info "Deployer: $DEPLOYER_ADDR ($DEPLOYER_BAL ETH)"

# Fund deployer if not funded (use RICH_L1_PRIVATE_KEY from .env)
DEPLOYER_BAL_WEI=$(cast balance --rpc-url "$L1_RPC_URL" "$DEPLOYER_ADDR" 2>/dev/null || echo "0")
if python3 -c "exit(0 if int('$DEPLOYER_BAL_WEI') >= 10**18 else 1)" 2>/dev/null; then
    ok "Deployer has enough ETH"
else
    if [ -n "${RICH_L1_PRIVATE_KEY:-}" ]; then
        step "Funding deployer with 100 ETH..."
        cast send \
            --private-key "$RICH_L1_PRIVATE_KEY" \
            --value 100ether \
            "$DEPLOYER_ADDR" \
            --legacy \
            --rpc-url "$L1_RPC_URL" > /dev/null
        ok "Deployer funded"
    else
        fail "Deployer has insufficient ETH and RICH_L1_PRIVATE_KEY is not set"
        exit 1
    fi
fi

# ── Prepare working directory ─────────────────────────────────────────────────
step "Preparing deployment workdir: $DEPLOY_WORKDIR"
rm -rf "$DEPLOY_WORKDIR"
mkdir -p "$DEPLOY_WORKDIR"
cp "$CONTRACTS_DIR/intent.toml.bak" "$DEPLOY_WORKDIR/intent.toml"
cp "$CONTRACTS_DIR/state.json.bak"  "$DEPLOY_WORKDIR/state.json"
ok "Workdir ready"

# ── Step 1: Deploy Transactor ─────────────────────────────────────────────────
step "Step 1: Deploying Transactor..."

TRANSACTOR_OUTPUT=$(docker run --rm \
    --network "$DOCKER_NETWORK" \
    -v "$DEPLOY_WORKDIR:/deployments" \
    -w /app/packages/contracts-bedrock \
    "$OP_CONTRACTS_IMAGE" \
    forge create --json --broadcast --legacy \
      --rpc-url "$L1_RPC_IN_DOCKER" \
      --private-key "$DEPLOYER_PRIVATE_KEY" \
      "src/periphery/Transactor.sol:Transactor.0.8.30" \
      --constructor-args "$ADMIN_OWNER_ADDRESS" 2>&1)

TRANSACTOR_ADDRESS=$(echo "$TRANSACTOR_OUTPUT" | grep -v '^Warning' | jq -r '.deployedTo // empty' 2>/dev/null)
if [ -z "$TRANSACTOR_ADDRESS" ] || [ "$TRANSACTOR_ADDRESS" = "null" ]; then
    fail "Failed to deploy Transactor"
    echo "$TRANSACTOR_OUTPUT" | tail -20
    exit 1
fi
ok "Transactor deployed at: $TRANSACTOR_ADDRESS"

# ── Step 2: Bootstrap superchain ──────────────────────────────────────────────
step "Step 2: Bootstrapping superchain..."

docker run --rm \
    --network "$DOCKER_NETWORK" \
    -v "$DEPLOY_WORKDIR:/deployments" \
    "$OP_CONTRACTS_IMAGE" \
    bash -c "
        set -e
        /app/op-deployer/bin/op-deployer bootstrap superchain \
          --l1-rpc-url $L1_RPC_IN_DOCKER \
          --private-key $DEPLOYER_PRIVATE_KEY \
          --artifacts-locator file:///app/packages/contracts-bedrock/forge-artifacts \
          --superchain-proxy-admin-owner $TRANSACTOR_ADDRESS \
          --protocol-versions-owner $ADMIN_OWNER_ADDRESS \
          --guardian $ADMIN_OWNER_ADDRESS \
          --outfile /deployments/superchain.json
    "
ok "Superchain bootstrapped"

PROTOCOL_VERSIONS_PROXY=$(jq -r '.protocolVersionsProxyAddress' "$DEPLOY_WORKDIR/superchain.json")
SUPERCHAIN_CONFIG_PROXY=$(jq -r '.superchainConfigProxyAddress' "$DEPLOY_WORKDIR/superchain.json")
PROXY_ADMIN_SC=$(jq -r '.proxyAdminAddress' "$DEPLOY_WORKDIR/superchain.json")

# ── Step 3: Bootstrap implementations ────────────────────────────────────────
step "Step 3: Bootstrapping implementations..."

CHALLENGER_ADDR=$(cast wallet address "$OP_CHALLENGER_PRIVATE_KEY")

docker run --rm \
    --network "$DOCKER_NETWORK" \
    -v "$DEPLOY_WORKDIR:/deployments" \
    "$OP_CONTRACTS_IMAGE" \
    bash -c "
        set -e
        /app/op-deployer/bin/op-deployer bootstrap implementations \
          --artifacts-locator file:///app/packages/contracts-bedrock/forge-artifacts \
          --l1-rpc-url $L1_RPC_IN_DOCKER \
          --outfile /deployments/implementations.json \
          --mips-version 8 \
          --private-key $DEPLOYER_PRIVATE_KEY \
          --protocol-versions-proxy $PROTOCOL_VERSIONS_PROXY \
          --superchain-config-proxy $SUPERCHAIN_CONFIG_PROXY \
          --superchain-proxy-admin $PROXY_ADMIN_SC \
          --upgrade-controller $ADMIN_OWNER_ADDRESS \
          --challenger $CHALLENGER_ADDR \
          --challenge-period-seconds $CHALLENGE_PERIOD_SECONDS \
          --withdrawal-delay-seconds $WITHDRAWAL_DELAY_SECONDS \
          --proof-maturity-delay-seconds $WITHDRAWAL_DELAY_SECONDS \
          --dispute-game-finality-delay-seconds $DISPUTE_GAME_FINALITY_DELAY_SECONDS \
          --dispute-clock-extension $TEMP_CLOCK_EXTENSION \
          --dispute-max-clock-duration $TEMP_MAX_CLOCK_DURATION \
          --dev-feature-bitmap 0x0000000000000000000000000000000000000000000000000000000000001000
    "
ok "Implementations bootstrapped"

OPCM_ADDRESS=$(jq -r '.opcmAddress' "$DEPLOY_WORKDIR/implementations.json")

# ── Step 4: Update intent.toml with new values ────────────────────────────────
step "Step 4: Updating intent.toml..."

CHAIN_ID_UINT256=$(cast to-uint256 "$CHAIN_ID")
sed -i '' 's/id = .*/id = "'"$CHAIN_ID_UINT256"'"/' "$DEPLOY_WORKDIR/intent.toml" 2>/dev/null || \
    sed -i 's/id = .*/id = "'"$CHAIN_ID_UINT256"'"/' "$DEPLOY_WORKDIR/intent.toml"
sed -i '' "s/l1ProxyAdminOwner = .*/l1ProxyAdminOwner = \"$TRANSACTOR_ADDRESS\"/" "$DEPLOY_WORKDIR/intent.toml" 2>/dev/null || \
    sed -i "s/l1ProxyAdminOwner = .*/l1ProxyAdminOwner = \"$TRANSACTOR_ADDRESS\"/" "$DEPLOY_WORKDIR/intent.toml"
sed -i '' "s/faultGameClockExtension = .*/faultGameClockExtension = $TEMP_CLOCK_EXTENSION/" "$DEPLOY_WORKDIR/intent.toml" 2>/dev/null || \
    sed -i "s/faultGameClockExtension = .*/faultGameClockExtension = $TEMP_CLOCK_EXTENSION/" "$DEPLOY_WORKDIR/intent.toml"
sed -i '' "s/faultGameMaxClockDuration = .*/faultGameMaxClockDuration = $TEMP_MAX_CLOCK_DURATION/" "$DEPLOY_WORKDIR/intent.toml" 2>/dev/null || \
    sed -i "s/faultGameMaxClockDuration = .*/faultGameMaxClockDuration = $TEMP_MAX_CLOCK_DURATION/" "$DEPLOY_WORKDIR/intent.toml"
sed -i '' "s/^opcmAddress = \".*\"/opcmAddress = \"$OPCM_ADDRESS\"/" "$DEPLOY_WORKDIR/intent.toml" 2>/dev/null || \
    sed -i "s/^opcmAddress = \".*\"/opcmAddress = \"$OPCM_ADDRESS\"/" "$DEPLOY_WORKDIR/intent.toml"
ok "intent.toml updated"

# ── Step 5: Deploy OP chain + generate genesis + rollup ──────────────────────
step "Step 5: Deploying OP chain and generating genesis.json + rollup.json..."

docker run --rm \
    --network "$DOCKER_NETWORK" \
    -v "$DEPLOY_WORKDIR:/deployments" \
    "$OP_CONTRACTS_IMAGE" \
    bash -c "
        set -e
        echo '🔧 Deploying OP chain contracts...'
        /app/op-deployer/bin/op-deployer apply \
          --workdir /deployments \
          --private-key $DEPLOYER_PRIVATE_KEY \
          --l1-rpc-url $L1_RPC_IN_DOCKER

        echo '📄 Generating genesis.json...'
        /app/op-deployer/bin/op-deployer inspect genesis \
          --workdir /deployments \
          $CHAIN_ID > /deployments/genesis.json

        echo '📄 Generating rollup.json...'
        /app/op-deployer/bin/op-deployer inspect rollup \
          --workdir /deployments \
          $CHAIN_ID > /deployments/rollup.json

        echo '✅ Done'
    "
ok "OP chain deployed, genesis.json and rollup.json generated"

# ── Step 6: Patch genesis.json (add legacyXLayerBlock, parentHash, etc.) ─────
step "Step 6: Patching genesis.json for XLayer..."

FORK_BLOCK=8593920
NEXT_BLOCK=$((FORK_BLOCK + 1))
NEXT_BLOCK_HEX=$(printf "0x%x" "$NEXT_BLOCK")
PARENT_HASH="0x6912fea590fd46ca6a63ec02c6733f6ffb942b84cdf86f7894c21e1757a1f68a"

python3 - <<PYEOF
import json, re

with open('$DEPLOY_WORKDIR/genesis.json') as f:
    g = json.load(f)

# Add legacyXLayerBlock to config
g['config']['legacyXLayerBlock'] = $NEXT_BLOCK

# Set parentHash to the XLayer pre-genesis parent
g['parentHash'] = '$PARENT_HASH'

# Set block number
g['number'] = '$NEXT_BLOCK_HEX'

# Fix EIP-1559 denominator/elasticity canyon params
eip1559_denom = g.get('config', {}).get('optimism', {}).get('eip1559Denominator', 50)
eip1559_denom_canyon = g.get('config', {}).get('optimism', {}).get('eip1559DenominatorCanyon', 250)
g['config']['eip1559DenominatorCanyon'] = eip1559_denom_canyon

# Fund the proposer address (Hardhat account #2) with large balance
proposer_addr = '0x70997970c51812dc3a010c7d01b50e0d17dc79c8'
if proposer_addr in g.get('alloc', {}):
    g['alloc'][proposer_addr]['balance'] = '0x446c3b15f9926687d2c40534fdb564000000000000'

with open('$DEPLOY_WORKDIR/genesis.json', 'w') as f:
    json.dump(g, f, indent=2)
print('genesis.json patched')
PYEOF

# Make genesis-reth.json with hex number (reth needs hex string, not decimal)
python3 -c "
import json
with open('$DEPLOY_WORKDIR/genesis.json') as f:
    g = json.load(f)
# reth expects number as hex string
g['number'] = '$NEXT_BLOCK_HEX'
with open('$DEPLOY_WORKDIR/genesis-reth.json', 'w') as f:
    json.dump(g, f, indent=2)
print('genesis-reth.json created')
"

ok "genesis.json patched"

# ── Step 7: Copy to config/devnet/ ───────────────────────────────────────────
step "Step 7: Copying genesis.json and rollup.json to config/devnet/..."

cp "$DEPLOY_WORKDIR/genesis-reth.json" "config/devnet/genesis.json"
cp "$DEPLOY_WORKDIR/rollup.json"       "config/devnet/rollup.json"

# op-deployer sets genesis.l2.number=0 for a standard genesis. Patch to the actual
# XLayer genesis block number (FORK_BLOCK+1) so kona finds it by number on fresh start.
jq ".genesis.l2.number = $NEXT_BLOCK" config/devnet/rollup.json > /tmp/rollup-number-patch.json
mv /tmp/rollup-number-patch.json config/devnet/rollup.json
ok "Copied genesis.json and rollup.json to config/devnet/ (genesis.l2.number=$NEXT_BLOCK)"

# ── Step 8: Patch rollup.json with reth genesis hash ─────────────────────────
step "Step 8: Patching rollup.json with reth genesis hash..."
./scripts/devnet/maintenance/reset-l1-reconfig.sh

# ── Step 9: Reset stale L2 data ───────────────────────────────────────────────
step "Step 9: Resetting stale L2 chain data..."
./scripts/devnet/maintenance/reset-l2.sh

echo ""
ok "OP contract redeployment complete."
info "Start fresh with: ./scripts/devnet/start-all.sh --no-build"
