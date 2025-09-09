#!/bin/bash

docker compose down
rm -rf data

rm -rf config-op/genesis.json
rm -rf config-op/genesis.json.gz
rm -rf config-op/implementations.json
rm -rf config-op/intent.toml
rm -rf config-op/rollup.json
rm -rf config-op/state.json
rm -rf config-op/superchain.json
rm -rf l1-geth/consensus/beacondata/
rm -rf l1-geth/consensus/genesis.ssz
rm -rf l1-geth/consensus/validatordata/
rm -rf l1-geth/execution/genesis.json
rm -rf l1-geth/execution/geth/
