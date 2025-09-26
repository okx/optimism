#!/bin/bash

set -x
set -e

BRANCH_NAME=${1:-""}
PWD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OPTIMISM_DIR=$(git rev-parse --show-toplevel)

[ ! -f .env ] && cp example.env .env

source .env

if [ "$OP_GETH_LOCAL_DIRECTORY" = "" ]; then
    git submodule update --init --recursive
    OP_GETH_DIR="$OPTIMISM_DIR/op-geth"
else
    OP_GETH_DIR="$OP_GETH_LOCAL_DIRECTORY"
fi

# Switch to specified branch if provided
if [ -n "$BRANCH_NAME" ]; then
    echo "Switching op-geth to branch: $BRANCH_NAME"
    cd $OP_GETH_DIR
    git fetch origin
    git checkout "$BRANCH_NAME"
    git pull origin "$BRANCH_NAME"
    cd "$PWD_DIR"
else
    echo "Using op-geth default branch"
fi

# TODO: need to further confirm why it fails if we do not add require in this contract
cp $PWD_DIR/contracts/Transactor.sol $OPTIMISM_DIR/packages/contracts-bedrock/src/periphery/Transactor.sol

cd $OPTIMISM_DIR

# Build OP_CONTRACTS image if not skipping
if [ $SKIP_OP_CONTRACTS_BUILD = "true" ]; then
    echo "skipping op-contracts build"
else
    echo "Building $OP_CONTRACTS_IMAGE_TAG..."
    docker build -t $OP_CONTRACTS_IMAGE_TAG -f ./Dockerfile-contracts .
fi

# Build OP_STACK image if not skipping
if [ $SKIP_OP_STACK_BUILD = "true" ]; then
    echo "skipping op-stack build"
else
    echo "Building $OP_STACK_IMAGE_TAG..."
    docker build -t $OP_STACK_IMAGE_TAG -f ./Dockerfile-opstack .
fi

# Build OP_GETH image if not skipping
if [ $SKIP_OP_GETH_BUILD = "true" ]; then
    echo "skipping op-geth build"
else
    echo "Building $OP_GETH_IMAGE_TAG"
    cd $OP_GETH_DIR
    if [ $DB_ENGINE = "rocksdb" ]; then
      docker build -t $OP_GETH_IMAGE_TAG -f ./Dockerfile.rocksdb .
    else
      docker build -t $OP_GETH_IMAGE_TAG .
    fi
fi
