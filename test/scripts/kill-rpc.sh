#!/bin/bash

set -e

source .env

set -x

docker compose kill $RPC_TYPE
docker compose kill op-rpc
rm -rf data/$RPC_TYPE
rm -rf data/op-rpc
