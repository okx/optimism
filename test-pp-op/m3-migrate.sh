#!/bin/bash
set -e
set -x

IMAGE_NAME="op-geth-migrate:latest"
CONTAINER_NAME="op-migrate-container"
RAMDISK_PATH="/mnt/ramdisk_op"
DATA_DIR="/data"
ERIGON_DATA_DIR="/data/erigon-data"
BACKUP_DIR="${DATA_DIR}/migration-backup-$(date +%Y%m%d-%H%M%S)"

mkdir -p ${BACKUP_DIR}

echo "=============================================="
echo "Step 1: Pre-flight checks"
echo "=============================================="

# Check if ramdisk is mounted
if ! mountpoint -q ${RAMDISK_PATH}; then
    echo "❌ Error: Ramdisk not mounted at ${RAMDISK_PATH}"
    echo "Please run m2-download-image.sh first to setup ramdisk"
    exit 1
fi

# Check if Docker image exists
if ! docker image inspect ${IMAGE_NAME} >/dev/null 2>&1; then
    echo "❌ Error: Docker image ${IMAGE_NAME} not found"
    echo "Please run m2-download-image.sh first to load the image"
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
        -v ${BACKUP_DIR}:${BACKUP_DIR} \ # For writing out config files.
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
echo "Step 3: Update Fork Configuration"
echo "=============================================="

# Prompt user for fork block number
read -p "Enter FORK_BLOCK number (the block to fork from): " FORK_BLOCK_INPUT

if [ -z "$FORK_BLOCK_INPUT" ]; then
    echo "❌ Error: FORK_BLOCK cannot be empty"
    exit 1
fi

echo "Fetching block information from L2 RPC..."

# Fetch block information inside container (where curl/jq are available)
docker exec -i ${CONTAINER_NAME} bash -c "
cd /app/test-pp-op

# Fetch block data
FORK_BLOCK_HEX=\$(printf '0x%x' $FORK_BLOCK_INPUT)
BLOCK_DATA=\$(curl -s -X POST http://rpcapi.xlayer.tech/sequencer \
  -H 'Content-Type: application/json' \
  -d '{\"jsonrpc\":\"2.0\",\"method\":\"eth_getBlockByNumber\",\"params\":[\"'\$FORK_BLOCK_HEX'\",true],\"id\":1}')

# Extract parent hash and timestamp
PARENT_HASH=\$(echo \$BLOCK_DATA | jq -r '.result.parentHash')
TIMESTAMP=\$(echo \$BLOCK_DATA | jq -r '.result.timestamp')

echo \"Block #$FORK_BLOCK_INPUT:\"
echo \"  Parent Hash: \$PARENT_HASH\"
echo \"  Timestamp: \$TIMESTAMP\"

# Validate data
if [ \"\$PARENT_HASH\" = \"null\" ] || [ -z \"\$PARENT_HASH\" ]; then
    echo \"❌ Error: Failed to fetch parent hash for block $FORK_BLOCK_INPUT\"
    exit 1
fi

if [ \"\$TIMESTAMP\" = \"null\" ] || [ -z \"\$TIMESTAMP\" ]; then
    echo \"❌ Error: Failed to fetch timestamp for block $FORK_BLOCK_INPUT\"
    exit 1
fi

# Update .env file
echo \"Updating .env with fork configuration...\"
sed -i \"s/^FORK_BLOCK=.*/FORK_BLOCK=$FORK_BLOCK_INPUT/\" .env
sed -i \"s|^PARENT_HASH=.*|PARENT_HASH=\$PARENT_HASH|\" .env

# Update genesis.json timestamp
echo \"Updating config-op/genesis.json with timestamp...\"
if [ -f config-op/genesis.json ]; then
    # Keep timestamp in hex string format (e.g., \"0x68F5EA9C\")
    jq \".timestamp = \\\"\$TIMESTAMP\\\"\" config-op/genesis.json > config-op/genesis.json.tmp
    mv config-op/genesis.json.tmp config-op/genesis.json
    echo \"✅ Genesis timestamp updated: \$TIMESTAMP\"
fi

echo \"\"
echo \"✅ Fork configuration updated successfully\"
echo \"   FORK_BLOCK: $FORK_BLOCK_INPUT\"
echo \"   PARENT_HASH: \$PARENT_HASH\"
echo \"   TIMESTAMP: \$TIMESTAMP\"
"

if [ $? -ne 0 ]; then
    echo "❌ Failed to update fork configuration"
    exit 1
fi

echo ""
echo "=============================================="
echo "Step 4: Configuration Verification"
echo "=============================================="
echo "Please review the configuration files before migration"
echo "Press ENTER to continue, any other key to abort"
echo ""

# Function to wait for Enter key
wait_for_enter() {
    local prompt="$1"
    echo "---"
    read -n 1 -s -r -p "$prompt" key
    echo ""
    if [ "$key" != "" ]; then
        echo "❌ Aborted by user"
        exit 1
    fi
}

# 1. Check .env file
echo "=============================================="
echo "1. Checking .env file"
echo "=============================================="
docker exec ${CONTAINER_NAME} bash -c "cd /app/test-pp-op && cat .env"
wait_for_enter "Press ENTER to continue to next check..."

# 2. Check genesis.json timestamp
echo ""
echo "=============================================="
echo "2. Checking config-op/genesis.json timestamp"
echo "=============================================="
docker exec ${CONTAINER_NAME} bash -c "cd /app/test-pp-op && jq '.timestamp' config-op/genesis.json"
echo ""
echo "Full genesis.json preview (first 50 lines):"
docker exec ${CONTAINER_NAME} bash -c "cd /app/test-pp-op && cat config-op/genesis.json | head -50"
wait_for_enter "Press ENTER to continue to next check..."

# 3. Check intent.toml
echo ""
echo "=============================================="
echo "3. Checking config-op/intent.toml"
echo "=============================================="
docker exec ${CONTAINER_NAME} bash -c "cd /app/test-pp-op && cat config-op/intent.toml"
wait_for_enter "Press ENTER to continue to next check..."

# 4. Check rollup.json
echo ""
echo "=============================================="
echo "4. Checking config-op/rollup.json"
echo "=============================================="
docker exec ${CONTAINER_NAME} bash -c "cd /app/test-pp-op && cat config-op/rollup.json"
wait_for_enter "Press ENTER to start migration..."

echo ""
echo "✅ Configuration verification completed"
echo ""
echo "=============================================="
echo "Step 5: Execute Migration"
echo "=============================================="
echo "Executing ./4-migrate-op.sh inside container..."
echo ""

# Execute migration script inside container
# Use docker exec to run the command and capture output
if docker exec -i ${CONTAINER_NAME} bash -c "
  cd /app/test-pp-op
  ./4-migrate-op.sh
  cp {.env,merged.genesis.json} ${BACKUP_DIR}
  cp -rf config-op ${BACKUP_DIR}/config-op
  "; then
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
echo "Step 6: Copy results to disk"
echo "=============================================="

echo "Backup directory: ${BACKUP_DIR}"
cp -rfv $RAMDISK_PATH/test-pp-op/data/op-geth-seq $BACKUP_DIR
echo "✅ Files copied successfully"

echo ""
echo "=============================================="
echo "Step 7: Cleanup"
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

echo "=============================================="
