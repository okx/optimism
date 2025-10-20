#!/bin/bash
set -e
set -x

CLEAN_ENVIRONMENT=${CLEAN_ENVIRONMENT:-true}
IMAGE_NAME="op-geth-migrate:latest"
CONTAINER_NAME="op-migrate-container"
RAMDISK_PATH="/mnt/ramdisk_op"
DATA_DIR="/data"
ERIGON_DATA_DIR="/data/erigon-data"
BACKUP_DIR="${DATA_DIR}/migration-backup-$(date +%Y%m%d-%H%M%S)"
L2_RPC_URL="${L2_RPC_URL:-http://rpcapi.xlayer.tech/sequencer}"

# Optional: Complete cleanup before migration
# Set CLEAN_ENVIRONMENT=true to force cleanup of container and ramdisk
if [ "${CLEAN_ENVIRONMENT}" = "true" ]; then
    echo "=============================================="
    echo "Environment Cleanup (CLEAN_ENVIRONMENT=true)"
    echo "=============================================="

    # 1. Remove container if exists
    if docker ps -a --format '{{.Names}}' | grep -q "^${CONTAINER_NAME}$"; then
        echo "Removing container ${CONTAINER_NAME}..."
        docker stop ${CONTAINER_NAME} 2>/dev/null || true
        docker rm ${CONTAINER_NAME} 2>/dev/null || true
        echo "✅ Container removed"
    fi

    # 2. Unmount ramdisk if mounted
    if mountpoint -q ${RAMDISK_PATH}; then
        echo "⚠️  Unmounting ramdisk at ${RAMDISK_PATH}..."
        echo "   This will clear all data in ramdisk!"
        if sudo umount ${RAMDISK_PATH} 2>/dev/null; then
            echo "✅ Ramdisk unmounted"
        else
            echo "❌ Error: Failed to unmount ramdisk (may be in use or require root)"
            echo "   Processes using ramdisk:"
            sudo lsof ${RAMDISK_PATH} 2>/dev/null || echo "   Unable to check (requires root)"
            echo "   You can manually unmount: sudo umount ${RAMDISK_PATH}"
            echo "   Or force kill processes: sudo fuser -km ${RAMDISK_PATH}"
            exit 1
        fi
    fi

    echo "✅ Environment cleaned successfully"
    echo "⚠️  Please run m2-download-image.sh to recreate ramdisk before migration"
    exit 0
fi

# Check required tools
for cmd in docker jq curl sed grep; do
    if ! command -v $cmd >/dev/null 2>&1; then
        echo "❌ Error: Required tool '$cmd' is not installed"
        exit 1
    fi
done

# Create and verify backup directory
mkdir -p ${BACKUP_DIR}
if ! touch ${BACKUP_DIR}/.write_test 2>/dev/null; then
    echo "❌ Error: Cannot write to backup directory: ${BACKUP_DIR}"
    exit 1
fi
rm -f ${BACKUP_DIR}/.write_test

# Function to fetch block data from RPC (executed on host)
fetch_block_data() {
    local fork_block=$1

    echo "Fetching block #$fork_block from RPC..."
    echo "RPC URL: ${L2_RPC_URL}"

    # Convert to hex
    local fork_block_hex=$(printf '0x%x' $fork_block)

    # Fetch block data from RPC (on host, not in container)
    local block_data=$(curl -s --max-time 30 -X POST ${L2_RPC_URL} \
      -H 'Content-Type: application/json' \
      -d '{"jsonrpc":"2.0","method":"eth_getBlockByNumber","params":["'$fork_block_hex'",true],"id":1}')

    local curl_exit_code=$?
    if [ $curl_exit_code -ne 0 ]; then
        echo "❌ Error: Failed to fetch block data from RPC (curl exit code: $curl_exit_code)"
        echo "   RPC URL: ${L2_RPC_URL}"
        echo "   This may be due to network issues or RPC timeout."
        echo "   You can set L2_RPC_URL environment variable to customize."
        exit 1
    fi

    # Extract hash and timestamp
    local block_hash=$(echo "$block_data" | jq -r '.result.hash')
    local timestamp=$(echo "$block_data" | jq -r '.result.timestamp')

    echo "Block #$fork_block:"
    echo "  Hash: $block_hash"
    echo "  Timestamp: $timestamp"

    # Validate data
    if [ "$block_hash" = "null" ] || [ -z "$block_hash" ]; then
        echo "❌ Error: Failed to fetch block hash for block $fork_block"
        echo "   RPC Response: $block_data"
        exit 1
    fi

    # Validate hash format (0x + 64 hex characters)
    if ! [[ "$block_hash" =~ ^0x[0-9a-fA-F]{64}$ ]]; then
        echo "❌ Error: Invalid block hash format: $block_hash"
        echo "   Expected: 0x followed by 64 hexadecimal characters"
        exit 1
    fi

    if [ "$timestamp" = "null" ] || [ -z "$timestamp" ]; then
        echo "❌ Error: Failed to fetch timestamp for block $fork_block"
        echo "   RPC Response: $block_data"
        exit 1
    fi

    # Validate timestamp format (0x + hex number)
    if ! [[ "$timestamp" =~ ^0x[0-9a-fA-F]+$ ]]; then
        echo "❌ Error: Invalid timestamp format: $timestamp"
        echo "   Expected: 0x followed by hexadecimal number"
        exit 1
    fi

    # Export for use by other functions
    export FETCHED_BLOCK_HASH="$block_hash"
    export FETCHED_TIMESTAMP="$timestamp"
    export FETCHED_FORK_BLOCK="$fork_block"
}

# Function to update fork configuration (executed in container)
update_fork_configuration() {
    local fork_block=$1
    local block_hash=$2
    local timestamp=$3

    echo ""
    echo "Updating .env with fork configuration..."

    docker exec ${CONTAINER_NAME} bash -c "
        set -e
        cd /app/test-pp-op

        # Calculate FORK_BLOCK + 1
        FORK_BLOCK_PLUS_ONE=\$(($fork_block + 1))

        # Verify .env exists and has required fields
        if [ ! -f .env ]; then
            echo \"❌ Error: .env file not found\"
            exit 1
        fi

        if ! grep -q '^FORK_BLOCK=' .env; then
            echo \"❌ Error: .env does not contain FORK_BLOCK field\"
            exit 1
        fi

        if ! grep -q '^PARENT_HASH=' .env; then
            echo \"❌ Error: .env does not contain PARENT_HASH field\"
            exit 1
        fi

        # Update .env file
        sed -i \"s/^FORK_BLOCK=.*/FORK_BLOCK=\$FORK_BLOCK_PLUS_ONE/\" .env
        sed -i \"s|^PARENT_HASH=.*|PARENT_HASH=$block_hash|\" .env

        # Verify updates succeeded
        ACTUAL_FORK_BLOCK=\$(grep '^FORK_BLOCK=' .env | cut -d'=' -f2)
        ACTUAL_PARENT_HASH=\$(grep '^PARENT_HASH=' .env | cut -d'=' -f2)

        if [ \"\$ACTUAL_FORK_BLOCK\" != \"\$FORK_BLOCK_PLUS_ONE\" ]; then
            echo \"❌ Error: Failed to update FORK_BLOCK\"
            echo \"   Expected: \$FORK_BLOCK_PLUS_ONE, Got: \$ACTUAL_FORK_BLOCK\"
            exit 1
        fi

        if [ \"\$ACTUAL_PARENT_HASH\" != \"$block_hash\" ]; then
            echo \"❌ Error: Failed to update PARENT_HASH\"
            echo \"   Expected: $block_hash, Got: \$ACTUAL_PARENT_HASH\"
            exit 1
        fi

        echo \"✅ .env updated:\"
        echo \"   FORK_BLOCK: \$FORK_BLOCK_PLUS_ONE (input block + 1)\"
        echo \"   PARENT_HASH: $block_hash\"
        echo \"   RPC_TIMESTAMP: $timestamp\"
    "
}

# Function to validate configuration (executed in container)
validate_configuration() {
    local rpc_timestamp=$1

    echo ""
    echo "============================================="
    echo "Validating Configuration Files"
    echo "============================================="

    docker exec ${CONTAINER_NAME} bash -c "
        set -e
        cd /app/test-pp-op

        # 1. Validate .env
        echo \"\"
        echo \"[1/4] Validating .env...\"
        ENV_CHAIN_ID=\$(grep '^CHAIN_ID=' .env | cut -d'=' -f2)
        if [ -z \"\$ENV_CHAIN_ID\" ]; then
            echo \"  ❌ Error: CHAIN_ID not found in .env\"
            echo \"   Please ensure .env contains 'CHAIN_ID=196'\"
            exit 1
        fi
        echo \"  CHAIN_ID: \$ENV_CHAIN_ID\"
        if [ \"\$ENV_CHAIN_ID\" != \"196\" ]; then
            echo \"  ❌ Error: .env CHAIN_ID must be 196, but got \$ENV_CHAIN_ID\"
            exit 1
        fi
        echo \"  ✅ .env validation passed\"

        # 2. Validate genesis.json
        echo \"\"
        echo \"[2/4] Validating config-op/genesis.json...\"
        if [ ! -f config-op/genesis.json ]; then
            echo \"  ❌ Error: config-op/genesis.json not found\"
            exit 1
        fi

        CHAIN_ID=\$(jq -r '.config.chainId' config-op/genesis.json)
        if [ -z \"\$CHAIN_ID\" ] || [ \"\$CHAIN_ID\" = \"null\" ]; then
            echo \"  ❌ Error: Failed to read .config.chainId from genesis.json\"
            echo \"   Response: \$CHAIN_ID\"
            exit 1
        fi
        echo \"  config.chainId: \$CHAIN_ID\"
        if [ \"\$CHAIN_ID\" != \"196\" ]; then
            echo \"  ❌ Error: Chain ID must be 196, but got \$CHAIN_ID\"
            exit 1
        fi

        # Validate timestamp: RPC timestamp must be < genesis.json timestamp
        EXISTING_TIMESTAMP=\$(jq -r '.timestamp' config-op/genesis.json)
        if [ -z \"\$EXISTING_TIMESTAMP\" ] || [ \"\$EXISTING_TIMESTAMP\" = \"null\" ]; then
            echo \"  ❌ Error: Failed to read .timestamp from genesis.json\"
            exit 1
        fi
        RPC_TS_DEC=\$(($rpc_timestamp))
        GENESIS_TS_DEC=\$((EXISTING_TIMESTAMP))
        echo \"  genesis.json timestamp: \$EXISTING_TIMESTAMP (decimal: \$GENESIS_TS_DEC)\"
        echo \"  RPC timestamp: $rpc_timestamp (decimal: \$RPC_TS_DEC)\"

        if [ \$RPC_TS_DEC -ge \$GENESIS_TS_DEC ]; then
            echo \"  ❌ Error: RPC timestamp (\$RPC_TS_DEC) must be < genesis.json timestamp (\$GENESIS_TS_DEC)\"
            echo \"   This indicates the fork block is at or after the genesis block.\"
            echo \"   Please specify an earlier fork block number.\"
            exit 1
        fi
        echo \"  ✅ genesis.json timestamp validation passed (RPC < genesis)\"

        # 3. Validate intent.toml
        echo \"\"
        echo \"[3/4] Validating config-op/intent.toml...\"
        if [ ! -f config-op/intent.toml ]; then
            echo \"  ❌ Error: config-op/intent.toml not found\"
            exit 1
        fi

        L1_CHAIN_ID=\$(grep '^l1ChainID' config-op/intent.toml | head -1 | sed 's/.*=[[:space:]]*\\([0-9]*\\).*/\\1/')
        if [ -z \"\$L1_CHAIN_ID\" ]; then
            echo \"  ❌ Error: Failed to extract l1ChainID from intent.toml\"
            exit 1
        fi
        echo \"  l1ChainID: \$L1_CHAIN_ID\"
        if [ \"\$L1_CHAIN_ID\" != \"1\" ]; then
            echo \"  ❌ Error: l1ChainID must be 1, but got \$L1_CHAIN_ID\"
            exit 1
        fi

        CHAIN_ID_HEX=\$(grep '^[[:space:]]*id[[:space:]]*=' config-op/intent.toml | head -1 | sed 's/.*\"\\(0x[0-9a-fA-F]*\\)\".*/\\1/')
        if [ -z \"\$CHAIN_ID_HEX\" ]; then
            echo \"  ❌ Error: Failed to extract chains[0].id from intent.toml\"
            exit 1
        fi
        CHAIN_ID_DEC=\$((\$CHAIN_ID_HEX))
        echo \"  chains[0].id: \$CHAIN_ID_HEX (decimal: \$CHAIN_ID_DEC)\"
        if [ \"\$CHAIN_ID_DEC\" != \"196\" ]; then
            echo \"  ❌ Error: chains[0].id must be 196, but got \$CHAIN_ID_DEC\"
            exit 1
        fi

        echo \"  ✅ intent.toml validation passed\"

        # 4. Validate rollup.json
        echo \"\"
        echo \"[4/4] Validating config-op/rollup.json...\"
        if [ ! -f config-op/rollup.json ]; then
            echo \"  ❌ Error: config-op/rollup.json not found\"
            exit 1
        fi

        ROLLUP_CHAIN_ID=\$(jq -r '.l2_chain_id' config-op/rollup.json)
        if [ -z \"\$ROLLUP_CHAIN_ID\" ] || [ \"\$ROLLUP_CHAIN_ID\" = \"null\" ]; then
            echo \"  ❌ Error: Failed to read .l2_chain_id from rollup.json\"
            exit 1
        fi
        echo \"  l2_chain_id: \$ROLLUP_CHAIN_ID\"
        if [ \"\$ROLLUP_CHAIN_ID\" != \"196\" ]; then
            echo \"  ❌ Error: l2_chain_id must be 196, but got \$ROLLUP_CHAIN_ID\"
            exit 1
        fi
        echo \"  ✅ rollup.json validation passed\"

        echo \"\"
        echo \"=============================================\"
        echo \"✅ All Configuration Validations Passed\"
        echo \"=============================================\"
    "
}

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

# Function to review configuration files interactively
review_configuration_files() {
    # 1. Check .env file
    echo "=============================================="
    echo "1. Checking .env file"
    echo "=============================================="
    docker exec ${CONTAINER_NAME} bash -c "set -e && cd /app/test-pp-op && cat .env"
    wait_for_enter "Press ENTER to continue to next check..."

    # 2. Check genesis.json timestamp
    echo ""
    echo "=============================================="
    echo "2. Checking config-op/genesis.json timestamp"
    echo "=============================================="
    docker exec ${CONTAINER_NAME} bash -c "set -e && cd /app/test-pp-op && jq '.timestamp' config-op/genesis.json"
    echo ""
    echo "Full genesis.json preview (first 50 lines):"
    docker exec ${CONTAINER_NAME} bash -c "set -e && cd /app/test-pp-op && cat config-op/genesis.json | head -50"
    wait_for_enter "Press ENTER to continue to next check..."

    # 3. Check intent.toml
    echo ""
    echo "=============================================="
    echo "3. Checking config-op/intent.toml"
    echo "=============================================="
    docker exec ${CONTAINER_NAME} bash -c "set -e && cd /app/test-pp-op && cat config-op/intent.toml"
    wait_for_enter "Press ENTER to continue to next check..."

    # 4. Check rollup.json
    echo ""
    echo "=============================================="
    echo "4. Checking config-op/rollup.json"
    echo "=============================================="
    docker exec ${CONTAINER_NAME} bash -c "set -e && cd /app/test-pp-op && cat config-op/rollup.json"
    wait_for_enter "Press ENTER to start migration..."

    echo ""
    echo "✅ Configuration verification completed"
}

# Function to execute migration
execute_migration() {
    # Execute migration script inside container
    if docker exec -i ${CONTAINER_NAME} bash -c "
        set -e
        cd /app/test-pp-op
        ./4-migrate-op.sh

        # Verify output files exist
        if [ ! -f .env ]; then
            echo \"❌ Error: .env not found after migration\"
            exit 1
        fi
        if [ ! -f merged.genesis.json ]; then
            echo \"❌ Error: merged.genesis.json not found after migration\"
            exit 1
        fi
        if [ ! -d config-op ]; then
            echo \"❌ Error: config-op directory not found\"
            exit 1
        fi

        # Copy files separately for better error handling
        cp .env ${BACKUP_DIR}/ || exit 1
        cp merged.genesis.json ${BACKUP_DIR}/ || exit 1
        cp -rf config-op ${BACKUP_DIR}/config-op || exit 1
    "; then
        echo ""
        echo "✅ Migration completed successfully inside container"
        return 0
    else
        local exit_code=$?
        echo ""
        echo "❌ Migration failed with exit code: ${exit_code}"
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
        exit ${exit_code}
    fi
}

echo "=============================================="
echo "Step 1: Pre-flight checks"
echo "=============================================="

# Check if ramdisk is mounted
if ! mountpoint -q ${RAMDISK_PATH}; then
    echo "❌ Error: Ramdisk not mounted at ${RAMDISK_PATH}"
    echo "Please run m2-download-image.sh first to setup ramdisk"
    exit 1
fi
echo "✅ Ramdisk is mounted at ${RAMDISK_PATH}"

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

# Force remove existing container for clean migration
if docker ps -a --format '{{.Names}}' | grep -q "^${CONTAINER_NAME}$"; then
    echo "🗑️  Removing existing container ${CONTAINER_NAME} for clean migration..."

    # Give container time to gracefully shutdown (30 seconds)
    if docker ps --format '{{.Names}}' | grep -q "^${CONTAINER_NAME}$"; then
        echo "   Stopping container (graceful shutdown, timeout 30s)..."
        docker stop -t 30 ${CONTAINER_NAME} 2>/dev/null || true
    fi

    docker rm ${CONTAINER_NAME} 2>/dev/null || true
    echo "✅ Old container removed"
fi

# Start container if not running
if ! docker ps --format '{{.Names}}' | grep -q "^${CONTAINER_NAME}$"; then
    echo "Starting container ${CONTAINER_NAME}..."
    docker run \
        --name ${CONTAINER_NAME} \
        -v /var/run/docker.sock:/var/run/docker.sock \
        -v ${ERIGON_DATA_DIR}:${ERIGON_DATA_DIR} \
        -v ${BACKUP_DIR}:${BACKUP_DIR} \
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
echo "Step 3: Update ForkBlock And Check"
echo "=============================================="

# Prompt user for fork block number
read -p "Enter FORK_BLOCK number (the block to fork from): " FORK_BLOCK_INPUT

if [ -z "$FORK_BLOCK_INPUT" ]; then
    echo "❌ Error: FORK_BLOCK cannot be empty"
    exit 1
fi

# Validate FORK_BLOCK is a positive integer
if ! [[ "$FORK_BLOCK_INPUT" =~ ^[0-9]+$ ]]; then
    echo "❌ Error: FORK_BLOCK must be a positive integer"
    echo "   Got: $FORK_BLOCK_INPUT"
    exit 1
fi

# Check if block number is reasonable (not zero)
if [ "$FORK_BLOCK_INPUT" -eq 0 ]; then
    echo "❌ Error: FORK_BLOCK cannot be zero"
    exit 1
fi

# Fetch block data from RPC (on host)
fetch_block_data $FORK_BLOCK_INPUT

# Update configuration in container with fetched data
update_fork_configuration $FETCHED_FORK_BLOCK $FETCHED_BLOCK_HASH $FETCHED_TIMESTAMP

# Validate all configurations (pass RPC timestamp for validation)
validate_configuration $FETCHED_TIMESTAMP

# Call the review function
echo ""
echo "=============================================="
echo "Step 4: Configuration Verification"
echo "=============================================="
echo "Please review the configuration files before migration"
echo "Press ENTER to continue, any other key to abort"
echo ""
review_configuration_files

# Execute migration and copy results
echo ""
echo "=============================================="
echo "Step 5: Execute Migration"
echo "=============================================="
echo "Executing ./4-migrate-op.sh inside container..."
echo ""
execute_migration

echo ""
echo "=============================================="
echo "Step 6: Copy results to disk"
echo "=============================================="

SOURCE_PATH="$RAMDISK_PATH/test-pp-op/data/op-geth-seq"
TEMP_DIR="${BACKUP_DIR}.tmp"

# Verify source exists
if [ ! -d "$SOURCE_PATH" ]; then
    echo "❌ Error: Source directory not found: $SOURCE_PATH"
    echo "   Expected migration output in ramdisk, but directory does not exist"
    echo ""
    echo "   Checking alternative locations..."

    # Check if data might be elsewhere
    if [ -d "$RAMDISK_PATH/test-pp-op/data" ]; then
        echo "   Contents of $RAMDISK_PATH/test-pp-op/data/:"
        ls -la "$RAMDISK_PATH/test-pp-op/data/" 2>/dev/null || echo "   (unable to list)"
    fi

    echo ""
    echo "   Please verify:"
    echo "   1. Migration script completed successfully"
    echo "   2. Output was written to ramdisk"
    echo "   3. Container volume mounts are correct"
    exit 1
fi

# Verify source is not empty
SOURCE_SIZE=$(du -sb "$SOURCE_PATH" 2>/dev/null | awk '{print $1}')
if [ "$SOURCE_SIZE" -lt 1024 ]; then
    echo "⚠️  Warning: Source directory is very small (${SOURCE_SIZE} bytes)"
    echo "   This may indicate migration did not complete properly"
    read -p "Continue anyway? (y/N): " CONTINUE
    if [[ ! "$CONTINUE" =~ ^[Yy]$ ]]; then
        echo "Aborted by user"
        exit 1
    fi
fi

echo "Source size: $(du -sh "$SOURCE_PATH" 2>/dev/null | awk '{print $1}')"

echo "Backup directory: ${BACKUP_DIR}"
echo "Copying data from ramdisk to disk (using atomic operation)..."

# Safety check for TEMP_DIR
if [ -z "$TEMP_DIR" ] || [ "$TEMP_DIR" = "/" ] || [ "$TEMP_DIR" = "/tmp" ]; then
    echo "❌ Error: Invalid TEMP_DIR value: $TEMP_DIR"
    exit 1
fi

# Use temporary directory for atomic copy
rm -rf "$TEMP_DIR"
mkdir -p "$TEMP_DIR"

# Copy to temporary location
if ! cp -r "$SOURCE_PATH" "$TEMP_DIR/op-geth-seq"; then
    echo "❌ Error: Failed to copy data to temporary directory"
    rm -rf "$TEMP_DIR"
    exit 1
fi

# Move to final location (atomic operation)
if ! mv "$TEMP_DIR/op-geth-seq" "$BACKUP_DIR/"; then
    echo "❌ Error: Failed to move data to backup directory"
    rm -rf "$TEMP_DIR"
    exit 1
fi

# Clean up
rm -rf "$TEMP_DIR"

echo "✅ Files copied successfully"

echo ""
echo "=============================================="
echo "✅ Migration process completed successfully!"
echo "=============================================="
echo "Backup directory: ${BACKUP_DIR}"

echo "=============================================="
