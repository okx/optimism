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

# If branch name is provided, clone a separate op-geth repo and set variables
if [ -n "$BRANCH_NAME" ]; then
    echo "Building op-geth image for branch: $BRANCH_NAME"

    # Create temporary directory outside optimism repo
    TEMP_DIR=$PWD_DIR/tmp
    echo "Created temporary directory: $TEMP_DIR"

    # Clone op-geth to temporary directory
    echo "Cloning op-geth repository..."
    git clone https://github.com/okx/op-geth.git "$TEMP_DIR/op-geth"

    cd "$TEMP_DIR/op-geth"
    git fetch origin
    git checkout "$BRANCH_NAME"
    git pull origin "$BRANCH_NAME"

    OP_GETH_DIR="$TEMP_DIR/op-geth"

    cd "$PWD_DIR"
else
    echo "No branch name provided, using default submodule"
fi

source .env


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

echo "Building $OP_GETH_IMAGE_TAG"
cd $OP_GETH_DIR
docker build -t $OP_GETH_IMAGE_TAG .