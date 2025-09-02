#!/bin/bash

set -x
set -e

git submodule update --init --recursive

cp example.env .env

source .env

PWD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OPTIMISM_DIR=$(git rev-parse --show-toplevel)
OP_GETH_DIR=${OPTIMISM_DIR}/op-geth

# TODO: need to further confirm why it fails if we do not add require in this contract
cp $PWD_DIR/contracts/Transactor.sol $OPTIMISM_DIR/packages/contracts-bedrock/src/periphery/Transactor.sol

cd $OPTIMISM_DIR
docker build -t $OP_CONTRACTS_IMAGE_TAG -f ./Dockerfile-contracts .
docker build -t $OP_STACK_IMAGE_TAG -f ./Dockerfile-opstack .

cd $OP_GETH_DIR
docker build -t $OP_GETH_IMAGE_TAG .
