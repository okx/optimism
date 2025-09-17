#!/bin/bash

set -x
set -e

BRANCH_NAME=${1:-""}
PWD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OPTIMISM_DIR=$(git rev-parse --show-toplevel)
OP_GETH_DIR=${OPTIMISM_DIR}/op-geth

git submodule update --init --recursive

# If branch name is provided, checkout that branch for op-geth submodule
if [ -n "$BRANCH_NAME" ]; then
    echo "Switching op-geth submodule to branch: $BRANCH_NAME"
    cd $OP_GETH_DIR
    git fetch origin
    git checkout "$BRANCH_NAME"
    git pull origin "$BRANCH_NAME"
    cd $PWD_DIR
    
    echo "Performing thorough cleanup for branch switch..."
    
    # Stop and remove all containers
    docker compose down --volumes --remove-orphans
    
    # Remove specific op-geth image tag only
    echo "Removing existing $OP_GETH_IMAGE_TAG image..."
    docker rmi $OP_GETH_IMAGE_TAG 2>/dev/null || true
else 
    echo "No branch name provided, using default branch"
    cd $OP_GETH_DIR
    git fetch origin
    git checkout dev
    cd $PWD_DIR
fi

cp example.env .env

source .env

# TODO: need to further confirm why it fails if we do not add require in this contract
cp $PWD_DIR/contracts/Transactor.sol $OPTIMISM_DIR/packages/contracts-bedrock/src/periphery/Transactor.sol

cd $OPTIMISM_DIR

# Build OP_CONTRACTS image if it doesn't exist
if [ -z "$(docker images -q $OP_CONTRACTS_IMAGE_TAG)" ]; then
    echo "Building $OP_CONTRACTS_IMAGE_TAG..."
    docker build -t $OP_CONTRACTS_IMAGE_TAG -f ./Dockerfile-contracts .
else
    echo "Image $OP_CONTRACTS_IMAGE_TAG already exists, skipping build"
fi

# Build OP_STACK image if it doesn't exist
if [ -z "$(docker images -q $OP_STACK_IMAGE_TAG)" ]; then
    echo "Building $OP_STACK_IMAGE_TAG..."
    docker build -t $OP_STACK_IMAGE_TAG -f ./Dockerfile-opstack .
else
    echo "Image $OP_STACK_IMAGE_TAG already exists, skipping build"
fi

# Build OP_GETH image if branch name is provided
if [ -n "$BRANCH_NAME" ]; then
    echo "Building $OP_GETH_IMAGE_TAG for branch: $BRANCH_NAME..."
    cd $OP_GETH_DIR
    docker build -t $OP_GETH_IMAGE_TAG .
else
    # Else check if the image exists and build it if it doesn't
    if [ -z "$(docker images -q $OP_GETH_IMAGE_TAG)" ]; then
        echo "Building $OP_GETH_IMAGE_TAG..."
        docker build -t $OP_GETH_IMAGE_TAG .
    else
        echo "Image $OP_GETH_IMAGE_TAG already exists, skipping build"
    fi
fi