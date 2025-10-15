#!/bin/bash
# =============================================================================
# Step31: Build op-program prestate
# Function: Build cannon prestate files for fraud proof
# =============================================================================
set -e
set -x

# Change to test directory (parent of steps/)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$(dirname "$SCRIPT_DIR")"

source .env
source tools.sh

echo "=========================================="
echo "Step31: Build op-program prestate"
echo "=========================================="

PWD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PWD_DIR=$(dirname "$PWD_DIR")
cd $PWD_DIR

if [ -z "$CHAIN_ID" ]; then
  echo " ERROR: CHAIN_ID is not set"
  exit 1
fi

# Detect architecture
detect_arch() {
    case "$OSTYPE" in
        *darwin*)
            [[ $(uname -m) == "arm64" ]] && echo "linux/arm64" || echo "linux/amd64"
            ;;
        *linux*)
            [[ $(uname -m) =~ (aarch64|arm64) ]] && echo "linux/arm64" || echo "linux/amd64"
            ;;
        *)
            echo "linux/amd64"
            ;;
    esac
}

DOCKER_PLATFORM=$(detect_arch)
echo "  Platform: $DOCKER_PLATFORM"

# genesis.json
echo "  genesis.json..."
gzip -c config-op/genesis.json > config-op/genesis.json.gz

# Preparingdirectory
EXPORT_DIR="$PWD_DIR/data/cannon-data"
rm -rf $EXPORT_DIR
mkdir -p $EXPORT_DIR

# rootless docker
ROOTLESS_DOCKER=$(docker info -f "{{println .SecurityOptions}}" | grep rootless || true)

echo " Build op-program prestate..."

if [ -n "$ROOTLESS_DOCKER" ]; then
    docker run --rm --privileged \
        --platform $DOCKER_PLATFORM \
        -v "$(pwd)/scripts:/scripts" \
        -v "$(pwd)/config-op/rollup.json:/app/op-program/chainconfig/configs/${CHAIN_ID}-rollup.json" \
        -v "$(pwd)/config-op/genesis.json.gz:/app/op-program/chainconfig/configs/${CHAIN_ID}-genesis-l2.json" \
        -v "$(pwd)/l1-geth/execution/genesis.json:/app/op-program/chainconfig/configs/1337-genesis-l1.json" \
        -v "$EXPORT_DIR:/app/op-program/bin" \
        "${OP_STACK_IMAGE_TAG}" \
        bash -c "/scripts/docker-install-start.sh && make -C op-program reproducible-prestate"
else
    docker run --rm \
        --platform $DOCKER_PLATFORM \
        -v /var/run/docker.sock:/var/run/docker.sock \
        -v "$(pwd)/config-op/rollup.json:/app/op-program/chainconfig/configs/${CHAIN_ID}-rollup.json" \
        -v "$(pwd)/config-op/genesis.json.gz:/app/op-program/chainconfig/configs/${CHAIN_ID}-genesis-l2.json" \
        -v "$(pwd)/l1-geth/execution/genesis.json:/app/op-program/chainconfig/configs/1337-genesis-l1.json" \
        -v "$EXPORT_DIR:/app/op-program/bin" \
        -e DOCKER_HOST=unix:///var/run/docker.sock \
        "${OP_STACK_IMAGE_TAG}" \
        bash -c "apt-get update && apt-get install docker.io -y && make -C op-program reproducible-prestate"
fi

# Validating
if [ ! -f "$EXPORT_DIR/prestate-proof-mt64.json" ]; then
    echo " prestatefileGenerate"
    exit 1
fi

PRESTATE=$(jq -r '.pre' "$EXPORT_DIR/prestate-proof-mt64.json")
echo ""
echo "SUCCESS: Step31completed: PrestateBuilding"
echo "   Prestate Hash: $PRESTATE"
echo "   Output Dir: $EXPORT_DIR"

