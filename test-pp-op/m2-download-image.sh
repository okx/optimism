#!/bin/bash
set -e
set -x

IMAGE_NAME="op-migrate"
ARCH="amd64"
TAR_FILE="${IMAGE_NAME}-${ARCH}.tar.gz"
RAMDISK_PATH="/mnt/ramdisk_op"
RAMDISK_SIZE="128g"
DATA_DIR="/data"

echo "=============================================="
echo "Step 1: Setting up ramdisk"
echo "=============================================="

# Check if ramdisk is already mounted
if mountpoint -q ${RAMDISK_PATH}; then
    echo "⚠️  Ramdisk is already mounted at ${RAMDISK_PATH}"
    read -p "Do you want to unmount and remount? (y/N): " REMOUNT
    if [[ "$REMOUNT" =~ ^[Yy]$ ]]; then
        echo "Unmounting ${RAMDISK_PATH}..."
        umount ${RAMDISK_PATH}
    else
        echo "Using existing ramdisk mount"
    fi
fi

# Create directory if not exists
if [ ! -d "${RAMDISK_PATH}" ]; then
    echo "Creating directory ${RAMDISK_PATH}..."
    mkdir -p ${RAMDISK_PATH}
fi

# Mount ramdisk if not mounted
if ! mountpoint -q ${RAMDISK_PATH}; then
    echo "Mounting ramdisk (size: ${RAMDISK_SIZE})..."
    mount -t tmpfs -o size=${RAMDISK_SIZE} tmpfs ${RAMDISK_PATH}
    echo "✅ Ramdisk mounted successfully"
fi

# Show ramdisk info
df -hT ${RAMDISK_PATH}

echo ""
echo "=============================================="
echo "Step 2: Download image from OSS"
echo "=============================================="

# Change to data directory
cd ${DATA_DIR}
echo "Current directory: $(pwd)"

# Prompt for ticket ID
echo ""
read -p "Enter ticket ID: " TICKET_ID

if [ -z "$TICKET_ID" ]; then
    echo "❌ Error: Ticket ID cannot be empty"
    exit 1
fi

# Download from OSS
echo ""
echo "Downloading from OSS with ticket ID: ${TICKET_ID}..."
osstool download -ticket ${TICKET_ID}

echo ""
echo "=============================================="
echo "Step 3: Extract tar.gz file"
echo "=============================================="

if [ ! -f "${TAR_FILE}" ]; then
    echo "❌ Error: ${TAR_FILE} not found in ${DATA_DIR}"
    exit 1
fi

echo "Extracting ${TAR_FILE}..."
tar -xzvf ${TAR_FILE}

echo ""
echo "=============================================="
echo "Step 4: Load Docker image"
echo "=============================================="

echo "Loading Docker image from ${TAR_FILE}..."
docker load < ${TAR_FILE}

echo ""
echo "Ticket ID: ${TICKET_ID}"
echo "File: ${TAR_FILE}"
echo "Ramdisk: ${RAMDISK_PATH} (${RAMDISK_SIZE})"
echo ""
echo "Docker images:"
docker images | grep ${IMAGE_NAME} || echo "No ${IMAGE_NAME} images found"
echo ""
echo "=============================================="
echo "✅ Image download and load completed successfully!"
echo "=============================================="
