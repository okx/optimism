#!/bin/bash
# =============================================================================
# Step20: Stop Erigon and extract fork info
# Function: Stop Erigon services, extract FORK_BLOCK and PARENT_HASH
# Output: Update FORK_BLOCK and PARENT_HASH in .env file
# =============================================================================
set -e
set -x

# Change to test directory (parent of steps/)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$(dirname "$SCRIPT_DIR")"

source .env
source tools.sh
source utils.sh

echo "=========================================="
echo "Step20: Stop Erigon and extract fork info"
echo "=========================================="

PWD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PWD_DIR=$(dirname "$PWD_DIR")
cd $PWD_DIR

# Stopping X Layer services
echo " StopErigonservice..."
echo "DOCKER_COMPOSE_CMD: ${DOCKER_COMPOSE_CMD}"

${DOCKER_COMPOSE_CMD} stop xlayer-seq || true
${DOCKER_COMPOSE_CMD} stop xlayer-rpc || true
${DOCKER_COMPOSE_CMD} stop xlayer-bridge-service || true
${DOCKER_COMPOSE_CMD} stop xlayer-bridge-ui || true
${DOCKER_COMPOSE_CMD} stop xlayer-agg-sender || true

echo "SUCCESS: ErigonserviceStop"
echo ""

# Extractfork blockInfo
echo " Extractfork blockInfo..."

# 1: RPCGet
if [ -n "${L2_RPC_URL:-}" ]; then
    echo "   RPCGet..."
    FORK_BLOCK=$(cast block-number --rpc-url $L2_RPC_URL 2>/dev/null || echo "")

    if [ -n "$FORK_BLOCK" ] && [ "$FORK_BLOCK" != "0" ]; then
        echo "   SUCCESS: RPCGetfork block: $FORK_BLOCK"

        # Getblockhashparent hash
        BLOCK_INFO=$(cast block $FORK_BLOCK --rpc-url $L2_RPC_URL --json 2>/dev/null || echo "")
        if [ -n "$BLOCK_INFO" ]; then
            PARENT_HASH=$(echo "$BLOCK_INFO" | jq -r '.hash // empty')
            echo "   SUCCESS: Getparent hash: $PARENT_HASH"
        fi
    fi
fi

# 2: DockerlogExtract
if [ -z "$FORK_BLOCK" ] || [ -z "$PARENT_HASH" ]; then
    echo "   RPCfailedlogExtract..."

    LOG_OUTPUT=$(docker logs xlayer-seq 2>&1 || echo "")

    if [ -n "$LOG_OUTPUT" ]; then
        # Extractfork block
        FORK_BLOCK=$(echo "$LOG_OUTPUT" | grep -E "(Finish block|Finished block)" | tail -1 | grep -oP 'block\s+\K\d+' || echo "")

        if [ -z "$FORK_BLOCK" ]; then
            FORK_BLOCK=$(echo "$LOG_OUTPUT" | grep "Finish block" | tail -1 | sed -n 's/.*Finish block \([0-9]*\).*/\1/p')
        fi

        # Extractparent hash
        PARENT_HASH=$(echo "$LOG_OUTPUT" | grep -E "(RPC Daemon notified|new headers|block hash)" | tail -1 | grep -oP '(hash=|hash:)\K0x[0-9a-fA-F]{64}' || echo "")

        if [ -z "$PARENT_HASH" ]; then
            PARENT_HASH=$(echo "$LOG_OUTPUT" | grep "RPC Daemon notified of new headers" | tail -1 | sed -n 's/.*hash=\([0-9a-fx]*\) .*/\1/p')
        fi

        if [ -n "$FORK_BLOCK" ]; then
            echo "   SUCCESS: logExtractfork block: $FORK_BLOCK"
        fi
        if [ -n "$PARENT_HASH" ]; then
            echo "   SUCCESS: logExtractparent hash: $PARENT_HASH"
        fi
    fi
fi

# ValidatingExtractResult
if [ -z "$FORK_BLOCK" ]; then
    echo " FORK_BLOCK"
    exit 1
fi

if ! [[ "$FORK_BLOCK" =~ ^[0-9]+$ ]]; then
    echo " FORK_BLOCKformat: $FORK_BLOCK"
    exit 1
fi

if [ -z "$PARENT_HASH" ]; then
    echo "  PARENT_HASHUsing0x00...00"
    PARENT_HASH="0x0000000000000000000000000000000000000000000000000000000000000000"
fi

if [[ ! "$PARENT_HASH" =~ ^0x[0-9a-fA-F]{64}$ ]]; then
    echo " PARENT_HASHformat: $PARENT_HASH"
    exit 1
fi

# fork block+1
ACTUAL_FORK_BLOCK=$((FORK_BLOCK + 1))

echo ""
echo " ForkInfo:"
echo "   Erigonblock:    $FORK_BLOCK"
echo "   OP Forkblock:       $ACTUAL_FORK_BLOCK"
echo "   Parent Hash:       $PARENT_HASH"
echo ""

# Updating.envfile
echo " Updating.envfile..."
sed_inplace "s/FORK_BLOCK=.*/FORK_BLOCK=$ACTUAL_FORK_BLOCK/" .env
sed_inplace "s/PARENT_HASH=.*/PARENT_HASH=$PARENT_HASH/" .env

# ValidatingUpdating
source .env
if [ "$FORK_BLOCK" = "$ACTUAL_FORK_BLOCK" ] && [ "$PARENT_HASH" != "" ]; then
    echo "SUCCESS: .envfileUpdating"
else
    echo " .envfileUpdatingfailed"
    exit 1
fi

echo ""
echo "SUCCESS: Step20completed: ForkInfoExtractSaving"
echo "   FORK_BLOCK=$FORK_BLOCK"
echo "   PARENT_HASH=$PARENT_HASH"

