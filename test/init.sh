#!/bin/bash

set -x
set -e

BRANCH_NAME=${1:-""}
PWD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OPTIMISM_DIR=$(git rev-parse --show-toplevel)

[ ! -f .env ] && cp example.env .env

# Update .env with branch-specific image tag if needed
if [ -n "$BRANCH_NAME" ]; then
    # Replace OP_GETH_IMAGE_TAG in .env file with branch-specific tag
    if [[ "$OSTYPE" == "darwin"* ]]; then
        sed -i '' "s|OP_GETH_IMAGE_TAG=.*|OP_GETH_IMAGE_TAG=$OP_GETH_IMAGE_TAG|" .env
    else
        sed -i "s|OP_GETH_IMAGE_TAG=.*|OP_GETH_IMAGE_TAG=$OP_GETH_IMAGE_TAG|" .env
    fi
fi

source .env


if [ "$OP_GETH_LOCAL_DIRECTORY" = "" ]; then
    OP_GETH_DIR="$OPTIMISM_DIR/op-geth"
else
    OP_GETH_DIR="$OP_GETH_LOCAL_DIRECTORY"
fi

# If branch name is provided, clone a separate op-geth repo and build branch-specific image
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
    
    echo "Performing cleanup for branch-specific build..."
    docker compose down
    
    # Create Docker-safe tag by replacing slashes with hyphens
    BRANCH_TAG=$(echo "$BRANCH_NAME" | sed 's/\//-/g')
    BRANCH_SPECIFIC_TAG="op-geth:$BRANCH_TAG"
    echo "Removing existing $BRANCH_SPECIFIC_TAG image..."
    docker rmi "$BRANCH_SPECIFIC_TAG" 2>/dev/null || true
    
    # Build branch-specific image
    echo "Building Docker image with tag: $BRANCH_SPECIFIC_TAG"
    docker build -t "$BRANCH_SPECIFIC_TAG" .
    
    # Return to original directory but keep TEMP_DIR for testing
    cd "$PWD_DIR"
    
    # Export TEMP_DIR for use in testing
    export OP_GETH_TEMP_DIR="$TEMP_DIR/op-geth"
    echo "Temporary op-geth directory available at: $OP_GETH_TEMP_DIR"
    
    # Update OP_GETH_IMAGE_TAG for this session
    export OP_GETH_IMAGE_TAG="$BRANCH_SPECIFIC_TAG"
else 
    echo "No branch name provided, using default submodule"
fi


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

# Build OP_GETH image from submodule if no branch was specified
if [ "$BRANCH_NAME" == ""  ]; then
    echo "Building $OP_GETH_IMAGE_TAG"
    cd $OP_GETH_DIR
    docker build -t $OP_GETH_IMAGE_TAG .
fi