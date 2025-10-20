#!/bin/bash
set -e
set -x

if [ -f .env ];then
  echo "Please create .env file."
  exit 1
fi

source .env
source tools.sh
source utils.sh

echo ""
echo "=============================================="
echo "Step 1: Deploy OP Contracts"
echo "=============================================="
./2-deploy-op-contracts.sh

IMAGE_NAME="op-geth-migrate"
ARCH="amd64"
TAR_FILE="${IMAGE_NAME}-${ARCH}.tar.gz"
BUILD_GETH=false; [[ "$*" =~ --build-geth ]] && BUILD_GETH=true

echo ""
echo "=============================================="
echo "Step 2: Build op-migrate image"
echo "=============================================="
[ "$BUILD_GETH" = true ] && ./build_images.sh --op-geth-migrate --arch linux/amd64 --force

echo ""
echo "=============================================="
echo "Step 3: Save Docker image to tar.gz"
echo "=============================================="
[ -n "$(docker images -q ${IMAGE_NAME})" ] || exit 1
docker save ${IMAGE_NAME}:latest | gzip > ${TAR_FILE}
echo "✅ Image saved to ${TAR_FILE}"

echo ""
echo "=============================================="
echo "Step 4: Calculate MD5 hash"
echo "=============================================="
if [[ "$OSTYPE" == "darwin"* ]]; then
    MD5_HASH=$(md5 -q ${TAR_FILE})
    echo "MD5 Hash: ${MD5_HASH}"
else
    md5sum ${TAR_FILE}
    MD5_HASH=$(md5sum ${TAR_FILE} | awk '{print $1}')
fi

echo ""
echo "=============================================="
echo "Step 5: Upload to OSS"
echo "=============================================="
echo "Please create an OSS ticket with the MD5 hash: ${MD5_HASH}."
