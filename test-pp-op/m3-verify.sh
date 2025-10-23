#!/bin/bash
set -e
set -x

IMAGE_NAME="op-geth-migrate:latest"
CONTAINER_NAME="op-verify-container"
RAMDISK_PATH="${RAMDISK_PATH:-/mnt/ramdisk_op}"
ERIGON_DATA_DIR="${ERIGON_DATA_DIR:-/data/erigon-data}"
OP_GETH_DATA_DIR="${RAMDISK_PATH}/test-pp-op/data/op-geth-seq"

echo "Verifying migration in ramdisk..."
echo "  Erigon: ${ERIGON_DATA_DIR}/chaindata"
echo "  OP-Geth: ${OP_GETH_DATA_DIR}"

# Basic checks
[ -d "${ERIGON_DATA_DIR}/chaindata" ] || { echo "❌ Erigon chaindata not found at ${ERIGON_DATA_DIR}/chaindata"; exit 1; }
[ -d "${OP_GETH_DATA_DIR}" ] || { echo "❌ OP-Geth data not found at ${OP_GETH_DATA_DIR}"; exit 1; }

# Cleanup old container
docker rm -f ${CONTAINER_NAME} 2>/dev/null || true

# Start container
docker run -d --name ${CONTAINER_NAME} \
    -v ${ERIGON_DATA_DIR}:/data/erigon-data \
    -v ${RAMDISK_PATH}:${RAMDISK_PATH} \
    ${IMAGE_NAME} sleep infinity

sleep 2

# Run verification
echo "Running verification..."
docker exec -it ${CONTAINER_NAME} geth verifyMigrate \
    --chaindata=/data/erigon-data/chaindata \
    --datadir=${OP_GETH_DATA_DIR} \
    --standalone-smt=true

RESULT=$?

# Cleanup
docker rm -f ${CONTAINER_NAME} 2>/dev/null || true

exit $RESULT

