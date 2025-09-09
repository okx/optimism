#!/bin/bash
set -e

sed_inplace() {
  if [[ "$OSTYPE" == "darwin"* ]]; then
    sed -i '' "$@"
  else
    sed -i "$@"
  fi
}

# NOTE: change to the real location of genesis file
GENESIS_FILE="../../genesis.json"

if [ -f config-op/genesis.json ]; then
    mv config-op/genesis.json config-op/genesis.json.bak
fi

cp $GENESIS_FILE config-op/genesis.json

current_timestamp=$(date +%s)
hex_timestamp=$(printf "0x%x\n" "$current_timestamp")
echo "hex_timestamp: $hex_timestamp"

sed_inplace "s/\"timestamp\": \"0x[0-9a-fA-F]*\"/\"timestamp\": \"$hex_timestamp\"/" config-op/genesis.json
