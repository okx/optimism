#!/bin/bash
# =============================================================================
# Step21: Migrate Erigon state to OP Geth
# Function: Migrate Erigon state using geth migrate command
# Output: merged.genesis.json Updating rollup.json
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
echo "Step21: Migrate Erigon state to OP Geth"
echo "=========================================="

PWD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PWD_DIR=$(dirname "$PWD_DIR")
cd $PWD_DIR

# Validate required environment variables
if [ -z "$FORK_BLOCK" ]; then
    echo " FORK_BLOCKSettingRunningStep20"
    exit 1
fi

if [ -z "$PARENT_HASH" ]; then
    echo " PARENT_HASHSettingRunningStep20"
    exit 1
fi

# Validatinggenesis.jsonrollup.json
if [ ! -f "./config-op/genesis.json" ]; then
    echo " genesis.jsonnot foundRunningStep10"
    exit 1
fi

if [ ! -f "./config-op/rollup.json" ]; then
    echo " rollup.jsonnot foundRunningStep10"
    exit 1
fi

echo " Migrationparameter:"
echo "   Fork Block:  $FORK_BLOCK"
echo "   Parent Hash: $PARENT_HASH"
echo ""

# Preparinggenesisfile
echo " Preparinggenesisfile..."
cp ./config-op/genesis.json ./config-op/genesis-op-raw.json
cp ./config-op/genesis.json ./config-op/genesis-op-before-number.json

# genesis.json
jq --argjson block "$FORK_BLOCK" \
   '.config.legacyXLayerBlock = $block' \
   ./config-op/genesis.json > temp_genesis.json && mv temp_genesis.json ./config-op/genesis.json

sed_inplace 's/"parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000"/"parentHash": "'"$PARENT_HASH"'"/' ./config-op/genesis.json

sed_inplace '/"70997970c51812dc3a010c7d01b50e0d17dc79c8": {/,/}/ s/"balance": "[^"]*"/"balance": "0x446c3b15f9926687d2c40534fdb564000000000000"/' config-op/genesis.json

# Updatingrollup.json
jq --argjson block "$FORK_BLOCK" \
   '.genesis.l2.number = $block' \
   ./config-op/rollup.json > temp_rollup.json && mv temp_rollup.json ./config-op/rollup.json

cp ./config-op/genesis.json ./config-op/genesis-op-after-number.json

echo "SUCCESS: GenesisfilePreparing"
echo ""

# Extractcontractaddress
echo " Extractcontractaddress..."
STATE_JSON="$PWD_DIR/config-op/state.json"

if [ -f "$STATE_JSON" ]; then
    OPCD_TYPE=$(jq -r '.opChainDeployments | type' "$STATE_JSON" 2>/dev/null)

    if [ "$OPCD_TYPE" = "array" ]; then
        JQ_BASE=".opChainDeployments[0]"
    else
        JQ_BASE=".opChainDeployments"
    fi

    DISPUTE_GAME_FACTORY_ADDRESS=$(jq -r "${JQ_BASE}.DisputeGameFactoryProxy // empty" "$STATE_JSON")
    SYSTEM_CONFIG_PROXY_ADDRESS=$(jq -r "${JQ_BASE}.SystemConfigProxy // empty" "$STATE_JSON")

    if [ -n "$DISPUTE_GAME_FACTORY_ADDRESS" ]; then
        sed_inplace "s/DISPUTE_GAME_FACTORY_ADDRESS=.*/DISPUTE_GAME_FACTORY_ADDRESS=$DISPUTE_GAME_FACTORY_ADDRESS/" .env
    fi

    if [ -n "$SYSTEM_CONFIG_PROXY_ADDRESS" ]; then
        sed_inplace "s/SYSTEM_CONFIG_PROXY_ADDRESS=.*/SYSTEM_CONFIG_PROXY_ADDRESS=$SYSTEM_CONFIG_PROXY_ADDRESS/" .env
    fi

    source .env
fi

# SettingErigondatadirectory
if [ "$ENV" = "local" ]; then
    ERIGON_CHAINDATA_DIR=${PWD_DIR}/data/rpc/chaindata/
    ERIGON_SMTDATA_DIR=${PWD_DIR}/data/rpc/smt/
else
    ERIGON_CHAINDATA_DIR=/data/erigon-data/chaindata/
    ERIGON_SMTDATA_DIR=/data/erigon-data/smt/
fi

# ValidatingErigondatadirectory
if [ ! -d "$ERIGON_CHAINDATA_DIR" ]; then
    echo " Erigon chaindatadirectorynot found: $ERIGON_CHAINDATA_DIR"
    exit 1
fi

if [ ! -d "$ERIGON_SMTDATA_DIR" ]; then
    echo " Erigon SMTdirectorynot found: $ERIGON_SMTDATA_DIR"
    exit 1
fi

echo " datadirectory:"
echo "   Erigon Chaindata: $ERIGON_CHAINDATA_DIR"
echo "   Erigon SMT:       $ERIGON_SMTDATA_DIR"
echo ""

# gethExecutingfile
if [[ "$OSTYPE" == "darwin"* ]]; then
    export GETH_CMD=/usr/local/bin/geth

    if [ ! -f ${GETH_CMD} ]; then
        echo " Buildinggeth..."
        cd ./tmp/op-geth
        make geth
        sudo cp ./build/bin/geth /usr/local/bin/geth
        cd $PWD_DIR
    else
        echo "SUCCESS: gethalready exists: ${GETH_CMD}"
    fi
else
    export GETH_CMD=${GETH_CMD:-geth}
fi

# Settingdirectory
export OP_DATA_DIR=./data/op-geth-seq
export OP_GENESIS_PATH=${PWD_DIR}/config-op/genesis-op-after-number.json

echo " Migration..."
echo ""

# ExecutingMigration
/usr/local/bin/geth \
    --datadir=${OP_DATA_DIR} \
    --gcmode=archive \
    migrate \
    --state.scheme=hash \
    --ignore-addresses=0x000000000000000000000000000000005ca1ab1e \
    --chaindata=${ERIGON_CHAINDATA_DIR} \
    --smt-db-path=${ERIGON_SMTDATA_DIR} \
    --output merged.genesis.json \
    ${OP_GENESIS_PATH} 2>&1 | tee migrate.log

echo ""
echo "SUCCESS: MigrationcommandExecutingcompleted"
echo ""

# Waiting forfileSyncing
sleep 5

# logExtractL2Info
echo " ExtractMigrationResult..."

LOG_BLOCK=$(grep -A 5 "Update rollup.json file with the following information l2" migrate.log 2>/dev/null | tail -n 5)

if [ -z "$LOG_BLOCK" ]; then
    echo " MigrationlogL2Info"
    exit 1
fi

# Extract using simple grep and sed (matching original script)
L2_NUMBER=$(echo "$LOG_BLOCK" | grep '"number"' | sed 's/[^0-9]*\([0-9]*\).*/\1/')
L2_HASH=$(echo "$LOG_BLOCK" | grep '"hash"' | sed 's/.*"\(0x[0-9a-fA-F]*\)".*/\1/')

# ValidatingExtractResult
if [ -z "$L2_NUMBER" ] || [ -z "$L2_HASH" ]; then
    echo " logExtractL2Info"
    echo "   L2_NUMBER: ${L2_NUMBER:-<empty>}"
    echo "   L2_HASH:   ${L2_HASH:-<empty>}"
    exit 1
fi

if ! [[ "$L2_NUMBER" =~ ^[0-9]+$ ]]; then
    echo " L2_NUMBERformat: $L2_NUMBER"
    exit 1
fi

if ! [[ "$L2_HASH" =~ ^0x[0-9a-fA-F]{64}$ ]]; then
    echo " L2_HASHformat: $L2_HASH"
    exit 1
fi

echo "SUCCESS: ExtractMigrationResult:"
echo "   L2 Number: $L2_NUMBER"
echo "   L2 Hash:   $L2_HASH"
echo ""

# Validatingmerged.genesis.json
if [ ! -f "merged.genesis.json" ]; then
    echo " merged.genesis.jsonGenerate"
    exit 1
fi

if ! jq empty merged.genesis.json 2>/dev/null; then
    echo " merged.genesis.jsonJSON"
    exit 1
fi

echo "SUCCESS: merged.genesis.jsonValidating"
echo ""

# Updatingrollup.json
echo " Updatingrollup.json..."
jq --argjson num "$L2_NUMBER" --arg hash "$L2_HASH" \
   '.genesis.l2.number = $num | .genesis.l2.hash = $hash' \
   config-op/rollup.json > config-op/rollup.json.tmp && \
   mv config-op/rollup.json.tmp config-op/rollup.json

# Validatingrollup.jsonUpdating
VERIFY_NUMBER=$(jq -r '.genesis.l2.number' config-op/rollup.json)
VERIFY_HASH=$(jq -r '.genesis.l2.hash' config-op/rollup.json)

if [ "$VERIFY_NUMBER" != "$L2_NUMBER" ] || [ "$VERIFY_HASH" != "$L2_HASH" ]; then
    echo " rollup.jsonUpdatingfailed"
    exit 1
fi

echo "SUCCESS: rollup.jsonUpdating"
echo ""

# checksum
echo " file..."
$MD5SUM_CMD merged.genesis.json
echo ""

echo "SUCCESS: Step21completed: StateMigrationSUCCESS"
echo "   file:"
echo "   - merged.genesis.json (Migrationstate)"
echo "   - config-op/rollup.json (UpdatinggenesisInfo)"
echo "   - migrate.log (log)"

