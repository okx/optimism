#!/bin/bash
set -e

# Debug mode - set to true to enable verbose output
DEBUG=${DEBUG:-false}
if [ "$DEBUG" = "true" ]; then
    set -x
fi

FORK_BLOCK=$1

IMAGE_NAME="op-geth-migrate:latest"
CONTAINER_NAME="op-migrate-container"
RAMDISK_PATH="/mnt/ramdisk_op"
DATA_DIR="/data"
ERIGON_DATA_DIR="/data/erigon-data"
BACKUP_DIR="${DATA_DIR}/migration-backup-$(date +%Y%m%d)"
L2_RPC_URL="${L2_RPC_URL:-http://10.2.29.232:8545}"
ENV=${ENV:-mainnet}
# Set expected chain ID based on ENV variable
EXPECTED_CHAIN=$([ "$ENV" = "testnet" ] && echo "1952" || echo "196")
EXPECTED_L1_CHAIN_ID=$([ "$ENV" = "testnet" ] && echo "11155111" || ([ "$ENV" = "fakemainnet" ] && echo "11155111" || echo "1"))
CHECK_BLOCK=${CHECK_BLOCK:-true}

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
    # max(old genesis timestamp, latest L2 block timestamp + 1)
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
            echo \"   Please ensure .env contains 'CHAIN_ID=$EXPECTED_CHAIN'\"
            exit 1
        fi
        echo \"  CHAIN_ID: \$ENV_CHAIN_ID\"
        if [ \"\$ENV_CHAIN_ID\" != \"$EXPECTED_CHAIN\" ]; then
            echo \"  ❌ Error: .env CHAIN_ID must be $EXPECTED_CHAIN, but got \$ENV_CHAIN_ID\"
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
        if [ \"\$CHAIN_ID\" != \"$EXPECTED_CHAIN\" ]; then
            echo \"  ❌ Error: Chain ID must be $EXPECTED_CHAIN, but got \$CHAIN_ID\"
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

        #if [ \$RPC_TS_DEC -ge \$GENESIS_TS_DEC ]; then
        #    echo \"  ❌ Error: RPC timestamp (\$RPC_TS_DEC) must be < genesis.json timestamp (\$GENESIS_TS_DEC)\"
        #    echo \"   This indicates the fork block is at or after the genesis block.\"
        #    echo \"   Please specify an earlier fork block number.\"
        #    exit 1
        #fi
        #echo \"  ✅ genesis.json timestamp validation passed (RPC < genesis)\"

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
        if [ \"\$L1_CHAIN_ID\" != \"$EXPECTED_L1_CHAIN_ID\" ]; then
            echo \"  ❌ Error: l1ChainID must be $EXPECTED_L1_CHAIN_ID, but got \$L1_CHAIN_ID\"
            exit 1
        fi

        CHAIN_ID_HEX=\$(grep '^[[:space:]]*id[[:space:]]*=' config-op/intent.toml | head -1 | sed 's/.*\"\\(0x[0-9a-fA-F]*\\)\".*/\\1/')
        if [ -z \"\$CHAIN_ID_HEX\" ]; then
            echo \"  ❌ Error: Failed to extract chains[0].id from intent.toml\"
            exit 1
        fi
        CHAIN_ID_DEC=\$((\$CHAIN_ID_HEX))
        echo \"  chains[0].id: \$CHAIN_ID_HEX (decimal: \$CHAIN_ID_DEC)\"
        if [ \"\$CHAIN_ID_DEC\" != \"$EXPECTED_CHAIN\" ]; then
            echo \"  ❌ Error: chains[0].id must be $EXPECTED_CHAIN, but got \$CHAIN_ID_DEC\"
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
        if [ \"\$ROLLUP_CHAIN_ID\" != \"$EXPECTED_CHAIN\" ]; then
            echo \"  ❌ Error: l2_chain_id must be $EXPECTED_CHAIN, but got \$ROLLUP_CHAIN_ID\"
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

# Function to extract and display configuration fields
extract_configuration_fields() {
    local phase="$1"  # "before" or "after"

    # Extract .env values
    echo "=== .env Configuration ==="
    docker exec ${CONTAINER_NAME} bash -c "set -e && cd /app/test-pp-op && \
        echo 'CHAIN_ID='\$(grep '^CHAIN_ID=' .env | cut -d'=' -f2) && \
        echo 'OKB_TOKEN_ADDRESS='\$(grep '^OKB_TOKEN_ADDRESS=' .env | cut -d'=' -f2) && \
        echo 'BATCHER_ADDRESS='\$(grep '^BATCHER_ADDRESS=' .env | cut -d'=' -f2) && \
        echo 'PROPOSER_ADDRESS='\$(grep '^PROPOSER_ADDRESS=' .env | cut -d'=' -f2) && \
        echo 'CHALLENGER_ADDRESS='\$(grep '^CHALLENGER_ADDRESS=' .env | cut -d'=' -f2) && \
        echo 'ADMIN_OWNER_ADDRESS='\$(grep '^ADMIN_OWNER_ADDRESS=' .env | cut -d'=' -f2) && \
        echo 'TIME_LOCK_DELAY='\$(grep '^TIME_LOCK_DELAY=' .env | cut -d'=' -f2) && \
        echo 'TEMP_MAX_CLOCK_DURATION='\$(grep '^TEMP_MAX_CLOCK_DURATION=' .env | cut -d'=' -f2) && \
        echo 'TEMP_CLOCK_EXTENSION='\$(grep '^TEMP_CLOCK_EXTENSION=' .env | cut -d'=' -f2) && \
        echo 'PROOF_MATURITY_DELAY_SECONDS='\$(grep '^PROOF_MATURITY_DELAY_SECONDS=' .env | cut -d'=' -f2) && \
        echo 'MAX_CLOCK_DURATION='\$(grep '^MAX_CLOCK_DURATION=' .env | cut -d'=' -f2) && \
        echo 'CLOCK_EXTENSION='\$(grep '^CLOCK_EXTENSION=' .env | cut -d'=' -f2) && \
        echo 'DISPUTE_GAME_FINALITY_DELAY_SECONDS='\$(grep '^DISPUTE_GAME_FINALITY_DELAY_SECONDS=' .env | cut -d'=' -f2) && \
        echo 'CHALLENGE_PERIOD_SECONDS='\$(grep '^CHALLENGE_PERIOD_SECONDS=' .env | cut -d'=' -f2) && \
        echo 'WITHDRAWAL_DELAY_SECONDS='\$(grep '^WITHDRAWAL_DELAY_SECONDS=' .env | cut -d'=' -f2) && \
        echo 'TRANSACTOR='\$(grep '^TRANSACTOR=' .env | cut -d'=' -f2)"

    echo ""
    echo "=== intent.toml Configuration ==="
    docker exec ${CONTAINER_NAME} bash -c "set -e && cd /app/test-pp-op && \
        echo 'l1ChainID='\$(grep '^l1ChainID' config-op/intent.toml | head -1 | sed 's/.*=[[:space:]]*\\([0-9]*\\).*/\\1/') && \
        echo 'opcmAddress='\$(grep '^opcmAddress' config-op/intent.toml | sed 's/.*\"\\(.*\\)\".*/\\1/') && \
        echo 'id='\$(grep '^[[:space:]]*id[[:space:]]*=' config-op/intent.toml | head -1 | sed 's/.*\"\\(.*\\)\".*/\\1/') && \
        echo 'baseFeeVaultRecipient='\$(grep 'baseFeeVaultRecipient' config-op/intent.toml | sed 's/.*\"\\(.*\\)\".*/\\1/') && \
        echo 'l1FeeVaultRecipient='\$(grep 'l1FeeVaultRecipient' config-op/intent.toml | sed 's/.*\"\\(.*\\)\".*/\\1/') && \
        echo 'sequencerFeeVaultRecipient='\$(grep 'sequencerFeeVaultRecipient' config-op/intent.toml | sed 's/.*\"\\(.*\\)\".*/\\1/') && \
        echo 'l2GenesisBlockGasLimit='\$(grep 'l2GenesisBlockGasLimit' config-op/intent.toml | sed 's/.*\"\\(.*\\)\".*/\\1/') && \
        echo 'l2GenesisBlockBaseFeePerGas='\$(grep 'l2GenesisBlockBaseFeePerGas' config-op/intent.toml | sed 's/.*\"\\(.*\\)\".*/\\1/') && \
        echo 'eip1559DenominatorCanyon='\$(grep 'eip1559DenominatorCanyon' config-op/intent.toml | cut -d'=' -f2 | tr -d ' ') && \
        echo 'eip1559Denominator='\$(grep 'eip1559Denominator' config-op/intent.toml | head -1 | cut -d'=' -f2 | tr -d ' ') && \
        echo 'eip1559Elasticity='\$(grep 'eip1559Elasticity' config-op/intent.toml | cut -d'=' -f2 | tr -d ' ') && \
        echo 'operatorFeeScalar='\$(grep 'operatorFeeScalar' config-op/intent.toml | cut -d'=' -f2 | tr -d ' ') && \
        echo 'operatorFeeConstant='\$(grep 'operatorFeeConstant' config-op/intent.toml | cut -d'=' -f2 | tr -d ' ') && \
        echo 'gasLimit='\$(grep 'gasLimit' config-op/intent.toml | cut -d'=' -f2 | tr -d ' ') && \
        echo 'l1ProxyAdminOwner='\$(grep 'l1ProxyAdminOwner' config-op/intent.toml | sed 's/.*\"\\(.*\\)\".*/\\1/') && \
        echo 'l2ProxyAdminOwner='\$(grep 'l2ProxyAdminOwner' config-op/intent.toml | sed 's/.*\"\\(.*\\)\".*/\\1/') && \
        echo 'systemConfigOwner='\$(grep 'systemConfigOwner' config-op/intent.toml | sed 's/.*\"\\(.*\\)\".*/\\1/') && \
        echo 'unsafeBlockSigner='\$(grep 'unsafeBlockSigner' config-op/intent.toml | sed 's/.*\"\\(.*\\)\".*/\\1/') && \
        echo 'batcher='\$(grep 'batcher' config-op/intent.toml | sed 's/.*\"\\(.*\\)\".*/\\1/') && \
        echo 'proposer='\$(grep 'proposer' config-op/intent.toml | sed 's/.*\"\\(.*\\)\".*/\\1/') && \
        echo 'challenger='\$(grep 'challenger' config-op/intent.toml | sed 's/.*\"\\(.*\\)\".*/\\1/')"

    echo ""
    echo "=== rollup.json Configuration ==="
    docker exec ${CONTAINER_NAME} bash -c "set -e && cd /app/test-pp-op && \
        echo 'hash='\$(jq -r '.genesis.l2.hash' config-op/rollup.json 2>/dev/null || echo 'N/A') && \
        echo 'number='\$(jq -r '.genesis.l2.number' config-op/rollup.json 2>/dev/null || echo 'N/A') && \
        echo 'l1_chain_id='\$(jq -r '.l1_chain_id' config-op/rollup.json 2>/dev/null || echo 'N/A') && \
        echo 'l2_chain_id='\$(jq -r '.l2_chain_id' config-op/rollup.json 2>/dev/null || echo 'N/A') && \
        echo 'batch_inbox_address='\$(jq -r '.batch_inbox_address' config-op/rollup.json 2>/dev/null || echo 'N/A') && \
        echo 'deposit_contract_address='\$(jq -r '.deposit_contract_address' config-op/rollup.json 2>/dev/null || echo 'N/A') && \
        echo 'l1_system_config_address='\$(jq -r '.l1_system_config_address' config-op/rollup.json 2>/dev/null || echo 'N/A') && \
        echo 'protocol_versions_address='\$(jq -r '.protocol_versions_address' config-op/rollup.json 2>/dev/null || echo 'N/A')"

    # Only show merged.genesis.json if it exists (after migration)
    if [ "$phase" = "after" ]; then
        echo ""
        echo "=== merged.genesis.json Configuration ==="
        docker exec ${CONTAINER_NAME} bash -c "set -e && cd /app/test-pp-op && \
            echo 'chainId='\$(head -n 20 merged.genesis.json | grep -o '\"chainId\":[[:space:]]*[0-9]*' | cut -d':' -f2 | tr -d ' ' || echo 'N/A') && \
            echo 'legacyXLayerBlock='\$(head -n 20 merged.genesis.json | grep -o '\"legacyXLayerBlock\":[[:space:]]*[0-9]*' | cut -d':' -f2 | tr -d ' ' || echo 'N/A') && \
            echo 'eip1559Elasticity='\$(head -n 50 merged.genesis.json | grep 'eip1559Elasticity' | head -1 | cut -d':' -f2 | tr -d ' ,' || echo 'N/A') && \
            echo 'eip1559Denominator='\$(head -n 50 merged.genesis.json | grep 'eip1559Denominator' | head -1 | cut -d':' -f2 | tr -d ' ,' || echo 'N/A') && \
            echo 'eip1559DenominatorCanyon='\$(head -n 50 merged.genesis.json | grep 'eip1559DenominatorCanyon' | head -1 | cut -d':' -f2 | tr -d ' ,' || echo 'N/A') && \
            echo 'parentHash='\$(tail -n 20 merged.genesis.json | grep -o '\"parentHash\":[[:space:]]*\"0x[0-9a-fA-F]*\"' | cut -d':' -f2 | tr -d ' \"' || echo 'N/A') && \
            echo 'baseFeePerGas='\$(tail -n 20 merged.genesis.json | grep -o '\"baseFeePerGas\":[[:space:]]*\"0x[0-9a-fA-F]*\"' | cut -d':' -f2 | tr -d ' \"' || echo 'N/A') && \
            echo 'timestamp='\$(head -n 50 merged.genesis.json | grep 'timestamp' | head -1 | cut -d':' -f2 | tr -d ' ,' || echo 'N/A')"
    else
        echo ""
        echo "=== merged.genesis.json Configuration ==="
        echo "⚠️  merged.genesis.json will be created during migration"
        echo "   This file contains the final genesis configuration after migration"
    fi

    echo ""
    echo "=============================================="
    wait_for_enter "Press ENTER to continue..."

    echo ""
    echo "✅ Configuration fields extracted and reviewed"
}

# Function to execute migration
execute_migration() {
    # Execute migration script inside container
    if docker exec -i ${CONTAINER_NAME} bash -c "
        set -e
        cd /app/test-pp-op
        ./4-migrate-op.sh

        if [ ! -f merged.genesis.json ]; then
            echo \"❌ Error: merged.genesis.json not found after migration\"
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

# Clean up any existing data from previous migrations
SOURCE_PATH="$RAMDISK_PATH/test-pp-op/data/op-geth-seq"
if [ -d "$SOURCE_PATH" ]; then
    echo "🗑️  Removing contents from $SOURCE_PATH..."
    rm -rf "$SOURCE_PATH"/*
    echo "✅ Previous data contents cleaned up"
else
    echo "✅ No existing data found at $SOURCE_PATH"
fi

mkdir -p ${SOURCE_PATH}

# Force remove existing container for clean migration
if docker ps -a --format '{{.Names}}' | grep -q "^${CONTAINER_NAME}$"; then
    echo "🗑️  Removing existing container ${CONTAINER_NAME} for clean migration..."

    # Give container time to gracefully shutdown (30 seconds)
    if docker ps --format '{{.Names}}' | grep -q "^${CONTAINER_NAME}$"; then
        echo "   Stopping container (graceful shutdown, timeout 30s)..."
        docker stop ${CONTAINER_NAME} 2>/dev/null || true
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
        -v ${ERIGON_DATA_DIR}:/data/erigon-data \
        -v ${BACKUP_DIR}:${BACKUP_DIR} \
        -v ${RAMDISK_PATH}:${RAMDISK_PATH} \
        -v ${RAMDISK_PATH}/test-pp-op/data/op-geth-seq:/app/test-pp-op/data/op-geth-seq \
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
echo "Step 3: Check configuration"
echo "=============================================="

if [ "$CHECK_BLOCK" = "true" ]; then
    if [ -z "$FORK_BLOCK" ]; then
      echo "❌ Error: FORK_BLOCK not set. Pls specify ./m2-migrate.sh FORK_BLOCK"
      exit 1
    fi

    # Fetch block data from RPC (on host)
    fetch_block_data $FORK_BLOCK
    # Validate all configurations (pass RPC timestamp for validation)
    validate_configuration $FETCHED_TIMESTAMP
fi

echo ""
echo "=============================================="
echo "Step 4: Review Configuration Before Migration"
echo "=============================================="
extract_configuration_fields "before"

echo ""
echo "=============================================="
echo "Step 5: Execute Migration"
echo "=============================================="
echo "Executing ./4-migrate-op.sh inside container..."
echo ""
execute_migration

echo ""
echo "=============================================="
echo "Step 6: Review Configuration After Migration"
echo "=============================================="
extract_configuration_fields "after"

echo ""
echo "=============================================="
echo "Step 7: Copy results to disk"
echo "=============================================="

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
