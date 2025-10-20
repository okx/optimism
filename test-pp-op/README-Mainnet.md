# Mainnet Migration Guide

Complete guide for migrating X Layer to Optimism on mainnet.

## Prerequisites

**Local Machine**: Docker 20.10+, osstool

**ECS Host**: Docker 20.10+, 128GB+ RAM, root access, Erigon data at `/data/erigon-data`, osstool

---

## Migration Steps

### 1. [Local] Deploy Contracts & Build Image

```bash
./m1-deploy-and-upload.sh
```

### 2. [ECS] Download Image & Setup

```bash
# need to copy scirpt to data first
cd /data/
sudo ./m2-download-image.sh  # Enter ticket ID when prompted
```

### 3. [ECS] Execute Migration

```bash
# need to copy scirpt to data first
cd /data/
sudo ./m3-migrate.sh
```

---
