#!/bin/bash
set -e
set -x

source .env
source tools.sh
source utils.sh

make clean
cp mainnet.env .env

./2-deploy-op-contracts.sh

# =============================================================================
# This script builds, saves, and uploads the op-migrate image to OSS
# =============================================================================

IMAGE_NAME="op-migrate"
ARCH="amd64"
TAR_FILE="${IMAGE_NAME}-${ARCH}.tar.gz"

echo "=============================================="
echo "Step 1: Building op-migrate image"
echo "=============================================="
./build_images.sh --op-geth-migrate --arch linux/amd64 --force

echo ""
echo "=============================================="
echo "Step 2: Saving Docker image to tar.gz"
echo "=============================================="
docker save ${IMAGE_NAME}:${ARCH} | gzip > ${TAR_FILE}
echo "✅ Image saved to ${TAR_FILE}"

echo ""
echo "=============================================="
echo "Step 3: Calculating MD5 hash"
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
echo "Step 4: Upload to OSS"
echo "=============================================="
echo "Please create an OSS ticket with the MD5 hash above."
echo ""
read -p "Enter ticket ID: " TICKET_ID

if [ -z "$TICKET_ID" ]; then
    echo "❌ Error: Ticket ID cannot be empty"
    exit 1
fi

echo ""
echo "Uploading ${TAR_FILE} with ticket ID: ${TICKET_ID}"
./osstool -f ${TAR_FILE} -a upload -ticket ${TICKET_ID}

echo ""
echo "=============================================="
echo "✅ Upload completed successfully!"
echo "=============================================="
echo "Ticket ID: ${TICKET_ID}"
echo "File: ${TAR_FILE}"
echo "MD5: ${MD5_HASH}"
echo ""
echo "Next steps (on ECS machine):"
echo "  osstool download -ticket ${TICKET_ID}"
echo "  docker load < ${TAR_FILE}"
