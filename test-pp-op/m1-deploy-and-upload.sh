#!/bin/bash
set -e
set -x

if [ ! -f .env ];then
  echo "Please create .env file."
  exit 1
fi

source .env
source tools.sh
source utils.sh

IMAGE_NAME=$(echo "${OP_GETH_MIGRATION_IMAGE_TAG}" | cut -d':' -f1)
TAR_FILE="${IMAGE_NAME}.tar.gz"
ARCH="linux/amd64"
SKIP_BUILD_GETH=false; [[ "$*" =~ --skip-geth ]] && BUILD_GETH=true

echo ""
echo "=============================================="
echo "Step 1: Deploy OP Contracts"
echo "=============================================="
./2-deploy-op-contracts.sh

echo ""
echo "=============================================="
echo "Step 2: Build op-migrate image"
echo "=============================================="
[ "$SKIP_BUILD_GETH" = true ] || ./build_images.sh --op-geth-migrate --arch ${ARCH} --force

echo ""
echo "=============================================="
echo "Step 3: Save Docker image to tar.gz"
echo "=============================================="
docker save ${OP_GETH_MIGRATION_IMAGE_TAG} | gzip > ${TAR_FILE}
[ -n "$(docker images -q ${IMAGE_NAME})" ] || exit 1
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
