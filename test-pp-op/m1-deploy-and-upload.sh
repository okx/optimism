#!/bin/bash
set -e

if [ ! -f .env ];then
  echo "Please create .env file."
  exit 1
fi

source .env
source tools.sh
source utils.sh

UPLOAD_DIR="upload-to-oss"
IMAGE_NAME=$(echo "${OP_GETH_MIGRATION_IMAGE_TAG}" | cut -d':' -f1)
TAR_FILE="${UPLOAD_DIR}.tar.gz"
ARCH="linux/amd64"
SKIP_BUILD_GETH=false; [[ "$*" =~ --skip-geth ]] && SKIP_BUILD_GETH=true
SKIP_DEPLOY=false; [[ "$*" =~ --skip-deploy ]] && SKIP_DEPLOY=true

echo ""
echo "=============================================="
echo "Step 1: Deploy OP Contracts"
echo "=============================================="
if [ "$SKIP_DEPLOY" = true ]; then
    echo "⏭️  Skipping 2-deploy-op-contracts.sh (--skip-deploy flag detected)"
else
    ./2-deploy-op-contracts.sh
fi

echo ""
echo "=============================================="
echo "Step 2: Build op-migrate image"
echo "=============================================="

# Remove previous uploads to keep size of docker image small.
echo "🗑️ Removing existing container ${UPLOAD_DIR}..."
rm -rf $UPLOAD_DIR ${UPLOAD_DIR}.tar.gz

if [ "$SKIP_BUILD_GETH" = true ]; then
    echo "⏭️  Skipping build_images.sh (--skip-geth flag detected)"
else
    ./build_images.sh --op-geth-migrate --arch ${ARCH} --force
fi

echo ""
echo "=============================================="
echo "Step 3: Save Docker image to tar.gz"
echo "=============================================="
docker save ${OP_GETH_MIGRATION_IMAGE_TAG} | gzip > ${IMAGE_NAME}.tar.gz
[ -n "$(docker images -q ${IMAGE_NAME})" ] || exit 1
echo "✅ Image saved to ${IMAGE_NAME}.tar.gz"

echo ""
echo "=============================================="
echo "Step 4: Create folder to store upload files"
echo "=============================================="
mkdir -p $UPLOAD_DIR
mv ${IMAGE_NAME}.tar.gz $UPLOAD_DIR
cp ./m2-migrate.sh $UPLOAD_DIR
tar -czvf $UPLOAD_DIR.tar.gz $UPLOAD_DIR
echo "✅ Upload file ${TAR_FILE} is created."

echo ""
echo "=============================================="
echo "Step 5: Calculate MD5 hash"
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
echo "Step 6: Upload to OSS"
echo "=============================================="
echo "Please create an OSS ticket using ${TAR_FILE} and its MD5 hash: ${MD5_HASH}."
