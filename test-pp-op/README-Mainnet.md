# Mainnet Migration Guide

Complete guide for migrating X Layer to Optimism on mainnet.

## Prerequisites

**Local Machine**: Docker 20.10+, osstool

**ECS Host**: Docker 20.10+, 128GB+ RAM, root access, Erigon data at `/data/erigon-data`, osstool

---

## Migration Steps

### 1. [Local] Deploy Contracts & Build Image

```bash
cd test-pp-op
make clean
cp mainnet.env .env

# Edit .env for mainnet config (FORK_BLOCK, PARENT_HASH, RPC endpoints, etc.)

./2-deploy-op-contracts.sh
./2.1-upload-image.sh  # Enter ticket ID when prompted
```

### 2. [ECS] Download Image & Setup

```bash
# need to copy scirpt to data first
cd /data/
sudo ./2.2-download-image.sh  # Enter ticket ID when prompted
```

### 3. [ECS] Execute Migration

```bash
# need to copy scirpt to data first
cd /data/
sudo ./4.0-migrate.sh
```

---
