#!/bin/bash

set -e

source .env

set -x

docker compose kill $RPC_TYPE
docker compose kill op-rpc
