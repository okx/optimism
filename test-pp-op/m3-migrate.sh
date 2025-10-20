#!/bin/bash
set -e
set -x

CONTAINER_NAME="${CONTAINER_NAME:-op-migrate-container}"
IMAGE_NAME="op-migrate:latest"
RAMDISK_PATH="/mnt/ramdisk_op"
DATA_DIR="/data"
ERIGON_DATA_DIR="/data/erigon-data"

echo "=============================================="
echo "Step 1: Pre-flight checks"
echo "=============================================="

# Check if ramdisk is mounted
if ! mountpoint -q ${RAMDISK_PATH}; then
    echo "❌ Error: Ramdisk not mounted at ${RAMDISK_PATH}"
    echo "Please run 2.2-download-image.sh first to setup ramdisk"
    exit 1
fi

# Check if Docker image exists
if ! docker image inspect ${IMAGE_NAME} >/dev/null 2>&1; then
    echo "❌ Error: Docker image ${IMAGE_NAME} not found"
    echo "Please run 2.2-download-image.sh first to load the image"
    exit 1
fi

# Check if erigon data directory exists
if [ ! -d "${ERIGON_DATA_DIR}" ]; then
    echo "❌ Error: Erigon data directory ${ERIGON_DATA_DIR} not found"
    exit 1
fi

echo "✅ All pre-flight checks passed"

echo ""
echo "=============================================="
echo "Step 2: Start Docker container"
echo "=============================================="

# Check if container already exists
if docker ps -a --format '{{.Names}}' | grep -q "^${CONTAINER_NAME}$"; then
    echo "⚠️  Container ${CONTAINER_NAME} already exists"
    read -p "Do you want to remove and recreate it? (y/N): " RECREATE
    if [[ "$RECREATE" =~ ^[Yy]$ ]]; then
        echo "Stopping and removing existing container..."
        docker stop ${CONTAINER_NAME} 2>/dev/null || true
        docker rm ${CONTAINER_NAME} 2>/dev/null || true
    else
        echo "Using existing container"
    fi
fi

# Start container if not running
if ! docker ps --format '{{.Names}}' | grep -q "^${CONTAINER_NAME}$"; then
    echo "Starting container ${CONTAINER_NAME}..."
    docker run \
        --name ${CONTAINER_NAME} \
        -v /var/run/docker.sock:/var/run/docker.sock \
        -v ${ERIGON_DATA_DIR}:${ERIGON_DATA_DIR} \
        -v ${RAMDISK_PATH}:${RAMDISK_PATH} \
        -v ${RAMDISK_PATH}/test-pp-op/data/op-geth-seq:/app/op-geth/test-pp-op/data/op-geth-seq \
        -e DOCKER_HOST=unix:///var/run/docker.sock \
        -d ${IMAGE_NAME} sleep infinity

    echo "✅ Container started successfully"

    # Wait a moment for container to be ready
    sleep 2
else
    echo "✅ Container ${CONTAINER_NAME} is already running"
fi

echo ""
echo "=============================================="
echo "Step 3: Execute migration inside container"
echo "=============================================="

echo "Executing ./4-migrate-op.sh inside container..."
echo ""

# Execute migration script inside container
# Use docker exec to run the command and capture output
if docker exec -i ${CONTAINER_NAME} bash -c "cd /app/op-geth/test-pp-op && ./4-migrate-op.sh"; then
    echo ""
    echo "✅ Migration completed successfully inside container"
else
    MIGRATION_EXIT_CODE=$?
    echo ""
    echo "❌ Migration failed with exit code: ${MIGRATION_EXIT_CODE}"
    echo ""
    read -p "Do you want to keep the container for debugging? (y/N): " KEEP_CONTAINER
    if [[ ! "$KEEP_CONTAINER" =~ ^[Yy]$ ]]; then
        echo "Stopping and removing container..."
        docker stop ${CONTAINER_NAME} 2>/dev/null || true
        docker rm ${CONTAINER_NAME} 2>/dev/null || true
    else
        echo "Container ${CONTAINER_NAME} kept for debugging"
        echo "To enter the container: docker exec -it ${CONTAINER_NAME} bash"
    fi
    exit ${MIGRATION_EXIT_CODE}
fi

echo ""
echo "=============================================="
echo "Step 4: Copy results to disk"
echo "=============================================="

echo "Copying important files from ramdisk to disk..."

# Create backup directory with timestamp
BACKUP_DIR="${DATA_DIR}/migration-backup-$(date +%Y%m%d-%H%M%S)"
mkdir -p ${BACKUP_DIR}

# Copy files to backup directory
echo "Backup directory: ${BACKUP_DIR}"

FILES_TO_COPY=(
    ".env"
    "config-op/rollup.json"
    "merged.genesis.json"
    "data/op-geth-seq"
)

cd ${RAMDISK_PATH}/test-pp-op

for file in "${FILES_TO_COPY[@]}"; do
    if [ -e "$file" ]; then
        echo "Copying $file..."
        cp -r "$file" "${BACKUP_DIR}/"
    else
        echo "⚠️  Warning: $file not found, skipping"
    fi
done

# Also copy to /data for easy access
echo ""
echo "Copying files to ${DATA_DIR} for easy access..."
for file in "${FILES_TO_COPY[@]}"; do
    if [ -e "$file" ]; then
        cp -r "$file" "${DATA_DIR}/" || true
    fi
done

echo "✅ Files copied successfully"

echo ""
echo "=============================================="
echo "Step 5: Cleanup"
echo "=============================================="

read -p "Do you want to stop and remove the container? (Y/n): " CLEANUP
if [[ ! "$CLEANUP" =~ ^[Nn]$ ]]; then
    echo "Stopping and removing container..."
    docker stop ${CONTAINER_NAME} 2>/dev/null || true
    docker rm ${CONTAINER_NAME} 2>/dev/null || true
    echo "✅ Container cleaned up"
else
    echo "Container ${CONTAINER_NAME} kept running"
    echo "To stop it later: docker stop ${CONTAINER_NAME} && docker rm ${CONTAINER_NAME}"
fi

echo ""
echo "=============================================="
echo "✅ Migration process completed successfully!"
echo "=============================================="
echo "Backup directory: ${BACKUP_DIR}"
echo "Files also copied to: ${DATA_DIR}"


echo "=============================================="
