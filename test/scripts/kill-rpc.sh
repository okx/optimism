#!/bin/bash

set -e

source .env

set -x

docker compose kill $RPC_TYPE
docker compose kill op-rpc
docker rm -f $RPC_TYPE
docker rm -f op-rpc
rm -rf data/$RPC_TYPE
rm -rf data/op-rpc
