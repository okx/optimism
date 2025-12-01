#!/bin/bash
set -e

git submodule update --init --recursive

# ========== Configuration ==========
JOVIAN_TIME=1764327600
CHAIN_ID=1952
# ===================================

# Cleanup function
cleanup() {
    rm -f *-old-clean.json *-new-clean.json *.bak
    rm -f merged.genesis.json merged.genesis.json.bak
    rm -f op-program/chainconfig/configs/*-old-clean.json op-program/chainconfig/configs/*-new-clean.json op-program/chainconfig/configs/*.bak
}
trap cleanup EXIT

echo "=== X Layer Testnet Prestate Build (Chain: $CHAIN_ID, Jovian: $JOVIAN_TIME) ==="

# ========== Startup Cleanup ==========
echo "Cleaning up previous run artifacts..."

# Remove modified genesis (will be re-extracted and re-modified)
rm -f merged.genesis.json merged.genesis.json.bak

# Clean configs directory (except placeholder.json)
find op-program/chainconfig/configs -type f ! -name 'placeholder.json' ! -name 'depsets.json' -delete 2>/dev/null || true

# Clean bin directory (will be rebuilt)
rm -rf op-program/bin/* 2>/dev/null || true

echo "✓ Cleanup complete"
echo ""

# ========== Download & Extract Genesis ==========
GENESIS_FILE="testnet.json.tar.gz"
GENESIS_URL="https://okg-pub-hk.oss-cn-hongkong.aliyuncs.com/cdn/chain/xlayer/snapshot/merged.genesis.json.tar.gz"

[ -f "$GENESIS_FILE" ] && echo "✓ Genesis archive exists" || {
    echo "Downloading genesis..."
    wget -q -O "$GENESIS_FILE" "$GENESIS_URL"
    echo "✓ Downloaded"
}

[ -f "merged.genesis.json" ] && echo "✓ Genesis extracted" || {
    echo "Extracting..."
    tar -xzf "$GENESIS_FILE"
    echo "✓ Extracted"
}

# Verify genesis Chain ID
echo "Verifying genesis Chain ID..."
GENESIS_CHAIN_ID=$(jq -r '.config.chainId' merged.genesis.json)
[ "$GENESIS_CHAIN_ID" == "$CHAIN_ID" ] || {
    echo "❌ Error: Genesis Chain ID mismatch!"
    echo "   Expected: $CHAIN_ID"
    echo "   Got: $GENESIS_CHAIN_ID"
    echo "   File: merged.genesis.json"
    exit 1
}
echo "✓ Genesis Chain ID verified: $CHAIN_ID"

# ========== Function: Add JSON field with validation ==========
add_json_field() {
    local file=$1
    local field_path=$2
    local field_name=$3
    local field_value=$4
    local insert_after=$5
    local insert_after_path=$6
    local jq_update=$7

    echo ""
    echo "=== Modifying $(basename $file) ==="

    # Check if target field already exists
    if jq -e "$field_path" "$file" >/dev/null 2>&1; then
        echo "❌ Error: $field_name already exists in $file"
        echo "   Current value: $(jq -r "$field_path" "$file")"
        echo "   Cannot proceed - field must not exist"
        exit 1
    fi

    # Check if insert_after field exists
    if ! jq -e "$insert_after_path" "$file" >/dev/null 2>&1; then
        echo "❌ Error: Required field '$insert_after' not found in $file"
        echo "   Cannot insert $field_name after non-existent field"
        exit 1
    fi

    # Backup and modify
    cp "$file" "${file}.bak"
    echo "Adding $field_name after $insert_after..."
    eval "$jq_update" > "${file}.tmp"
    mv "${file}.tmp" "$file"

    # Show and validate changes
    echo "Changes:"
    diff -u "${file}.bak" "$file" | grep -A1 -B1 "$field_name" || true

    # Ensure only target field was added
    local clean_old="${file%.json}-old-clean.json"
    local clean_new="${file%.json}-new-clean.json"
    jq "del($field_path)" "${file}.bak" > "$clean_old"
    jq "del($field_path)" "$file" > "$clean_new"

    if ! diff -q "$clean_old" "$clean_new" >/dev/null 2>&1; then
        echo "❌ Error: Unexpected changes detected!"
        diff -u "$clean_old" "$clean_new" | head -20
        rm -f "$clean_old" "$clean_new" "${file}.bak"
        exit 1
    fi

    # Clean up immediately after validation
    rm -f "$clean_old" "$clean_new" "${file}.bak"
    echo "✓ Validated: only $field_name added"
}

# ========== Modify Genesis ==========
add_json_field \
    "merged.genesis.json" \
    ".config.jovianTime" \
    "jovianTime" \
    "$JOVIAN_TIME" \
    "isthmusTime" \
    ".config.isthmusTime" \
    "jq --argjson jt $JOVIAN_TIME '.config |= (to_entries | map(if .key == \"isthmusTime\" then [., {key: \"jovianTime\", value: \$jt}] else [.] end) | flatten | from_entries)' merged.genesis.json"

# ========== Compress Genesis ==========
mkdir -p op-program/chainconfig/configs
gzip -c merged.genesis.json > op-program/chainconfig/configs/${CHAIN_ID}-genesis-l2.json
echo "✓ Genesis compressed to chainconfig/configs"

# ========== Download Rollup Config ==========
echo ""
echo "=== Rollup Config ==="
ROLLUP_FILE="op-program/chainconfig/configs/${CHAIN_ID}-rollup.json"
ROLLUP_URL="https://raw.githubusercontent.com/okx/xlayer-toolkit/main/rpc-setup/presets/rollup-testnet.json"

[ -f "$ROLLUP_FILE" ] && echo "✓ Rollup config exists" || {
    echo "Downloading rollup config..."
    curl -sS -o "$ROLLUP_FILE" "$ROLLUP_URL"
    echo "✓ Downloaded"
}

# Verify Chain ID
DOWNLOADED_CHAIN_ID=$(jq -r .l2_chain_id "$ROLLUP_FILE")
[ "$DOWNLOADED_CHAIN_ID" == "$CHAIN_ID" ] || {
    echo "❌ Error: Chain ID mismatch (expected: $CHAIN_ID, got: $DOWNLOADED_CHAIN_ID)"
    exit 1
}
echo "✓ Chain ID verified: $CHAIN_ID"

# ========== Modify Rollup Config ==========
add_json_field \
    "$ROLLUP_FILE" \
    ".jovian_time" \
    "jovian_time" \
    "$JOVIAN_TIME" \
    "isthmus_time" \
    ".isthmus_time" \
    "jq --argjson jt $JOVIAN_TIME 'to_entries | map(if .key == \"isthmus_time\" then [., {key: \"jovian_time\", value: \$jt}] else [.] end) | flatten | from_entries' \"$ROLLUP_FILE\""

# ========== Final Verification ==========
echo ""
echo "=== Final Verification ==="
echo "Genesis jovianTime: $(gunzip -c op-program/chainconfig/configs/${CHAIN_ID}-genesis-l2.json | jq -r '.config.jovianTime')"
echo "Rollup jovian_time: $(jq -r '.jovian_time' "$ROLLUP_FILE")"

# ========== Build ==========
echo ""
echo "=== Building Reproducible Prestate ==="

# Ensure configs directory is clean before build
echo "Cleaning up any remaining temporary files..."
rm -f op-program/chainconfig/configs/*.bak
rm -f op-program/chainconfig/configs/*-old-clean.json
rm -f op-program/chainconfig/configs/*-new-clean.json
echo "✓ Configs directory cleaned"

make reproducible-prestate -e TARGETOS=linux -e TARGETARCH=amd64

# ========== Result ==========
echo ""
echo "=== Prestate Hash ==="
PRESTATE_HASH=$(cat op-program/bin/prestate-proof-mt64.json | jq -r .pre)
echo "$PRESTATE_HASH"
echo ""
echo "✓ Build completed successfully!"

# ========== Package Results ==========
echo ""
echo "=== Packaging Results ==="

# Create build info
BUILD_TIME=$(date '+%Y-%m-%d %H:%M:%S')
BUILD_INFO="op-program/BUILD_INFO.txt"

cat > "$BUILD_INFO" << EOF
X Layer Testnet Prestate Build
================================
Chain ID: $CHAIN_ID
Jovian Time: $JOVIAN_TIME
Build Time: $BUILD_TIME
Prestate Hash: $PRESTATE_HASH
Git Commit: $(git rev-parse --short HEAD 2>/dev/null || echo "unknown")
EOF

echo "✓ Build info created"

# Package bin and configs
TIMESTAMP=$(date +%Y%m%d-%H%M%S)
PACKAGE_NAME="xlayer-testnet-prestate-${CHAIN_ID}-${TIMESTAMP}.tar.gz"

tar -czf "$PACKAGE_NAME" \
    -C op-program bin configs BUILD_INFO.txt

echo "✓ Package created: $PACKAGE_NAME"
echo "  Size: $(du -h "$PACKAGE_NAME" | cut -f1)"
echo "  SHA256: $(shasum -a 256 "$PACKAGE_NAME" | cut -d' ' -f1)"
echo ""
echo "✓ All done!"
