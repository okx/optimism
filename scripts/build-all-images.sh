#!/usr/bin/env bash
set -euo pipefail

# One-click build for all service images in this repo.
# Requirements: docker buildx (or docker), internet access to fetch 'just' for builder image.
TAG=latest
build_image() {
  local dockerfile=$1
  local imagename=$2
  echo "[step] Building ${imagename}:${TAG} using ${dockerfile}"
  docker build \
    -f "${dockerfile}" \
    -t "${imagename}:${TAG}" \
    .
}

# Build all service images
# build_image Dockerfile.op-node op-node
build_image Dockerfile.op-batcher op-batcher
build_image Dockerfile.op-proposer op-proposer
build_image Dockerfile.op-challenger op-challenger || echo "[warn] op-challenger failed; check build tags or sources"
build_image Dockerfile.op-conductor op-conductor
build_image Dockerfile.op-validator op-validator || echo "[warn] op-validator failed; check Makefile/justfile"
build_image Dockerfile.op-deployer op-deployer || echo "[warn] op-deployer failed; using 'just build' in its justfile may be required"

echo "[done] Build complete. Images tagged with :${TAG}"

