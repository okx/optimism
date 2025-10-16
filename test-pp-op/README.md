# Optimism Test Environment Setup Guide

## run on local
```bash
# git submodule update --recursive --force
make clean
cp local.env .env
# ./build_images.sh --all # build all images. add --force if want to force rebuild
./1-start-erigon.sh
./2-deploy-op-contracts.sh
./3-stop-erigon.sh
./4-migrate-op.sh
./5-start-op.sh
./6-build-op-program.sh
./7-setup-fraud-proof.sh
```

## run on testnet
```bash
make clean
cp testnet.env .env
./2-deploy-op-contracts.sh
# AFTER DEPLOYING OP CONTRACTS, CHECK TRANSACTOR ADDRESS ON SEPOLIA.
# NOTE: l1ProxyAdminOwner + opcm + transactor addr checking (check intent.toml)

# Update .env ()
# pause erigon, update .env fork_num.
Update FORK_BLOCK+1
# Overwrite .env to replace with inner node sepolia/beacon node
# Do this before you build op-migrate image.
https://fullnode-inner.okg.com/sepolia/fork/okbc/rpc
https://fullnode-inner.okg.com/ethsepoliabeacon/native/layer1/rpc

# LOCAL ENVIRONMENT
# ----------------------------------------------------------------------------
# Build the image locally after deploying contracts (rollup.json and genesis.json).
docker build \
  --platform linux/amd64 \
  --build-arg CHAIN_ID=196 \
  --build-arg OP_STACK_IMAGE=op-stack:amd64 \
  --progress=plain \
  -t op-migrate:amd64 -f dockerfile/Dockerfile.op-program .

docker save op-migrate:amd64 | gzip > op-migrate-amd64.tar.gz

# INSIDE DACs TERMINAL
# ----------------------------------------------------------------------------
# Calculate md5 hash to create OSS ticket.
md5sum op-migrate-amd64.tar.gz
# Use osstool to upload images to ECS.
./osstool -f op-migrate-amd64.tar.gz -a upload -ticket ${ticket-id}

# INSIDE ECS MACHINE
# ----------------------------------------------------------------------------
# If not mounted memory, do this ONCE.
mkdir -p /mnt/ramdisk_op
mount -t tmpfs -o size=128g tmpfs /mnt/ramdisk_op
df -hT /mnt/ramdisk_op

# In disk
cd /data
# download from OSS
osstool download -ticket ${ticket-id}
# untar the uploaded file
tar -xzvf op-migrate-amd64.tar.gz
# load the docker image into local registry
docker load < op-migrate-amd64.tar.gz

# START REGENESIS (ECS host machine)
# ----------------------------------------------------------------------------
docker run \
  --name $CONTAINER_NAME \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v /data/erigon-data:/data/erigon-data \
  -v /mnt/ramdisk_op:/mnt/ramdisk_op \
  -v /mnt/ramdisk_op/test-pp-op/data/op-geth-seq:/app/op-geth/test-pp-op/data/op-geth-seq \
  -v /mnt/ramdisk_op/test-pp-op/data/cannon-data:/app/op-program/bin \
  -e DOCKER_HOST=unix:///var/run/docker.sock \
  -d op-migrate:amd64 sleep infinity

cd /app/op-geth
# Execute regenesis only.
./4-migrate-op.sh
# Copy configs to mount location
cp -rfv /app/op-geth/test-pp-op/* /app/op-geth/test-pp-op/.* /mnt/ramdisk_op/test-pp-op
exit

# Inside ECS host, save everything needed (to start new sequencer and build OP program) to disk.
cp /mnt/ramdisk_op/test-pp-op/{.env,config-op/rollup.json,merged.genesis.json,data/op-geth-seq} /data
```

## Prerequisites

### System Requirements
- Docker 20.10.0 or higher
- Docker Compose
- At least 32GB RAM
- At least 32GB available disk space

> **Important**: If you encounter performance issues, increase Docker engine memory limit to over 32GB
> ![issue](./images/stuck.png)
> ![solution](./images/docker.png)

### Initial Setup (First Time Only)
1. Run `./init.sh` to initialize the environment (only needed once):
   - Install all git submodules
   - Build required Docker images
   - Prepare base environment

> Important: `init.sh` should only be run once during initial setup. Re-run only if you need to rebuild Docker images after code changes.

### Code Updates and Image Rebuilding (Optional)
If you've updated the Optimism codebase and need to rebuild Docker images:

1. **Update image tags** in `example.env`:
   ```bash
   # Example: increment version numbers
   OP_GETH_IMAGE_TAG=op-geth:v1.101512.0-patch
   OP_STACK_IMAGE_TAG=op-stack:v1.13.5
   OP_CONTRACTS_IMAGE_TAG=op-contracts:v1.13.5
   ```

2. **Apply changes**:
   ```bash
   ./clean.sh  # This will update .env from example.env
   ```

3. **Rebuild images**:
   ```bash
   ./init.sh  # Rebuilds all Docker images

   # Or rebuild specific images only (optional)
   source .env && cd .. && docker build -t ${OP_STACK_IMAGE_TAG} -f Dockerfile-opstack . && cd -
   ```

### Directory Structure
```
test/
├── 0-all.sh            # One-click deployment script
├── init.sh             # Initialization script
├── clean.sh            # Environment cleanup script
├── 1-start-l1.sh       # L1 chain startup script
├── 2-deploy-op-contracts.sh  # Contract deployment script
├── 3-op-init.sh        # Environment initialization script
├── 4-op-start-service.sh    # Service startup script
├── scripts/            # Utility scripts
│   ├── transfer_leader.sh      # Leader transfer script
│   ├── stop_leader_sequencer.sh # Sequencer stop script
│   ├── active_sequencer.sh      # Check active sequencer
│   ├── add_game_type.sh         # Add dispute game type
│   ├── deposit-from-l1.sh      # L1 to L2 deposit script
│   ├── deposit-from-banker.sh  # transfer ETH from banker script
│   └── show-dev-accounts.sh   # Display dev accounts info
├── config-op/          # Configuration directory
├── data/              # Data storage directory
├── contracts/         # Smart contracts
├── images/            # Documentation images
├── example.env        # Environment template
```

## Quick Start

### One-Click Deployment
Run `./0-all.sh` to automatically:
- Initialize the environment
- Start all required components
- Complete all configurations and deployments

⚠️ **Important Notes**:

1. Configuration Management:
   - Always make configuration changes in `example.env`
   - Never modify `.env` directly as it will be reset by `clean.sh`
   - Run `clean.sh` to apply changes from `example.env`

2. Environment Reset:
   - `clean.sh` will stop all containers
   - Clean all data directories
   - Reset `.env` to values from `example.env`

> Note: For first-time setup, we recommend following the step-by-step deployment process to better understand each component and troubleshoot any potential issues.

### Step-by-Step Deployment
For more granular control or troubleshooting, follow the steps below.

## Deployment Process

### 1. L1 Environment Setup
Run `./1-start-l1.sh`:
- Starts a complete PoS L1 test chain (EL + CL)
- CL node handles blob data storage
- Automatically funds test accounts:
  - Batcher
  - Proposer
  - Challenger

### 2. Smart Contract Deployment
Run `./2-deploy-op-contracts.sh`:
- Deploys Transactor contract
- Deploys and initializes all Optimism L1 contracts
- Generates configuration files:
  - `rollup.json`: op-node configuration
  - `genesis.json`: L2 initial state

### 3. Environment Initialization
Run `./3-op-init.sh`:
- Initializes op-geth database
  - Sequencer node
  - RPC node
- Generates dispute game components:
  - Compiles op-program
  - Generates prestate files
  - Creates state proofs

### 4. Service Startup
Run `./4-op-start-service.sh`:
- Launches core services:
  - op-batcher: L2 transaction batch processing
  - op-proposer: L2 state submission
  - op-node: State sync and validation
  - op-geth: L2 execution engine
  - op-challenger: State validation
  - op-dispute-mon: Dispute monitoring
  - op-conductor: Sequencer HA management

### 5. Conductor Management
The test environment includes a 3-node conductor cluster for sequencer high availability (HA).

#### Architecture
- **Cluster Type**: 3-node Raft consensus cluster
- **Active Sequencer**: Only runs on leader node
- **Failover**: Automatic when leader becomes unhealthy
- **High Availability**: Ensures continuous L2 block production

#### Configuration
Enable or disable conductor cluster in `example.env`:
```bash
# Enable HA mode with conductor cluster
CONDUCTOR_ENABLED=true

# Disable HA, run single sequencer
CONDUCTOR_ENABLED=false
```

#### Network Ports
Each conductor node uses three ports:
- **RPC Port**: Management API
  - Node 1: 8547
  - Node 2: 8548
  - Node 3: 8549

- **Consensus Port**: Raft protocol
  - Node 1: 50050
  - Node 2: 50051
  - Node 3: 50052

- **Sequencer Port**: L2 execution
  - Node 1: 9545
  - Node 2: 9546
  - Node 3: 9547

#### Health Monitoring
The conductor cluster monitors each node's:
- Sync status with L1
- P2P network connectivity
- Block production rate

When leader becomes unhealthy:
- Automatically transfers leadership
- Deactivates unhealthy sequencer
- Activates sequencer on new leader

#### Leadership Management
There are two ways to trigger leader transfer:

1. Using `transfer-leader.sh`:
```bash
# Auto transfer to any healthy node
./scripts/transfer-leader.sh

# Transfer to specific node (1 or 2 or 3)
./scripts/transfer-leader.sh 2
```

2. Force transfer by stopping leader's sequencer:
```bash
# Stops block production while keeping the container running
# Automatically triggers leader transfer after health check timeout
# Can be run multiple times to test different leadership scenarios
./scripts/stop-leader-sequencer.sh
```

This method simulates a sequencer failure scenario, enabling comprehensive testing of automatic failover mechanisms. Each execution stops the current leader's sequencer and triggers a transfer to another node, allowing you to test different leadership scenarios by running the script multiple times. The cluster maintains high availability through dynamic role switching - when a sequencer stops producing blocks, it transitions to follower status while another node assumes leadership. The system remains resilient as any follower can automatically promote to leader if the current leader encounters issues.

3. Gray upgrade using op-conductor:
```bash
# Emulate the whole gray upgrade process to achieve 0 downtime
./scripts/gray-upgrade-simulation.sh
# Meanwhile, open another terminal window to load test
polycli loadtest --rpc-url http://localhost:8124 \
  --private-key "0x4bbbf85ce3377467afe5d46f804f221813b2bb87f24d81f60f1fcdbf7cbf4356" \
  --verbosity 700 --requests 50000  -c 1 --rate-limit -1
```

The `scripts/gray-upgrade-simulation.sh` script simulates a rolling upgrade process for the sequencer cluster managed by op-conductor. It upgrades a follower sequencer while keeping the leader running, then transfers leadership to the upgraded node. This approach ensures service continuity and validates the cluster's resilience during upgrades.

## Utility Scripts

### L1 to L2 Deposit Script

The `scripts/deposit-from-l1.sh` script facilitates testing L1 to L2 cross-chain deposits:

#### Features
- **Automatic Deposit**: Deposits 3000 ETH from L1 to L2
- **Balance Monitoring**: Monitors L2 balance changes in real-time
- **Wait Time Tracking**: Measures total deposit confirmation time
- **Status Updates**: Provides clear progress feedback

#### Usage
```bash
# Run the deposit script
./scripts/deposit-from-l1.sh
```

#### What it does
1. **Funds Test Account**: Sends ETH to the test account on L1
2. **Creates Deposit Transaction**: Calls OptimismPortal.depositTransaction()
3. **Monitors L2 Balance**: Continuously checks L2 balance until change detected
4. **Reports Results**: Shows deposit confirmation time and balance changes

#### Configuration
The script uses these default parameters:
- **Deposit Amount**: 3000 ETH
- **Gas Limit**: 100,000
- **Target Address**: Same as sender (self-deposit)
- **L2 RPC**: http://127.0.0.1:8123

This script is useful for:
- Testing cross-chain deposit functionality
- Measuring deposit confirmation times
- Verifying L1/L2 synchronization
- Validating OptimismPortal contract integration

### Banker Account Deposit Script

The `scripts/deposit-from-banker.sh` script enables large-scale testing using a banker account with massive balance:

#### Features
- **Large Amount Transfers**: Transfers 1,000,000 ETH (1 million ETH)
- **Banker Account**: Uses a pre-funded account with astronomical balance
- **Simple Transfer**: Direct ETH transfer without cross-chain complexity
- **Legacy Transaction**: Uses legacy transaction format for compatibility

#### Usage
```bash
# Run the banker deposit script
./scripts/deposit-from-banker.sh
```

#### What it does
1. **Uses Banker Account**: Leverages account `0x70997970C51812dc3A010C7d01b50e0d17dc79C8`
2. **Large Transfer**: Sends 1,000,000 ETH to target address
3. **Direct Transfer**: Simple ETH transfer on L2 network
4. **Legacy Format**: Uses legacy transaction format

### Development Accounts Display Script

The `scripts/show-dev-accounts.sh` script displays all development accounts with their private keys:

#### Features
- **Account Listing**: Shows all 30 development accounts (paths 0-29)
- **Private Key Display**: Reveals private keys for testing
- **Address Generation**: Shows corresponding addresses
- **Mnemonic Path**: Displays derivation paths

#### Usage
```bash
# Display all dev accounts
./scripts/show-dev-accounts.sh
```

#### What it does
1. **Generates Accounts**: Creates 30 accounts from standard mnemonic (paths 0-29)
2. **Shows Details**: Displays address, private key, and derivation path
3. **Standard Mnemonic**: Uses "test test test test test test test test test test test junk"
4. **Path Format**: Uses `m/44'/60'/0'/0/{i}` derivation paths

#### Important Notes
- **Balance Status**: Most dev accounts are pre-funded with 10,000 ETH, but some accounts may have zero initial balance

## Troubleshooting

### Common Issues

#### 1. Service Startup Failures
- **Check Docker logs**: `docker compose logs <service-name>`
- **Verify port availability**: Ensure ports 8545, 8546, 4000, 3500 are free
- **Validate environment variables**: Check `.env` file matches `example.env`
- **Memory issues**: Increase Docker memory limit to 32GB+

#### 2. Contract Deployment Issues
- **Verify L1 node is running**: Check `docker compose ps`
- **Check account balances**: Ensure test accounts have sufficient ETH
- **Validate gas settings**: Check gas limit and price in deployment logs

#### 3. Synchronization Issues
- **L2 not syncing**: Check op-geth-seq is running and producing blocks
- **RPC node issues**: Verify op-geth-rpc can connect to op-geth-seq
- **Genesis mismatch**: Ensure rollup.json matches actual L2 genesis

#### 4. Conductor Cluster Issues
- **Leader election problems**: Check Raft consensus logs
- **Sequencer not switching**: Verify health check configuration
- **P2P connectivity**: Check network configuration and firewall

## Service Ports Overview

| Service | Port | Description |
|---------|------|-------------|
| **L1 Services** | | |
| l1-geth | 8545 | L1 Ethereum RPC |
| l1-geth | 8546 | L1 Ethereum WebSocket |
| l1-geth | 8551 | L1 Ethereum Engine API |
| l1-beacon-chain | 4000 | L1 Beacon RPC |
| l1-beacon-chain | 3500 | L1 Beacon HTTP |
| l1-beacon-chain | 18080 | L1 Beacon Metrics |
| **L2 Services** | | |
| op-geth-seq | 8123 | L2 Sequencer RPC |
| op-geth-seq | 7546 | L2 Sequencer WebSocket |
| op-geth-seq | 8552 | L2 Sequencer Engine API |
| op-geth-rpc | 9123 | L2 RPC Node RPC |
| op-seq | 9545 | L2 Node RPC |
| op-seq | 7070 | L2 Node P2P |
| op-seq | 9223 | L2 Node P2P (UDP) |
| op-rpc | 9555 | L2 RPC Node RPC |
| **Conductor Cluster** | | |
| op-conductor | 8547 | Conductor 1 RPC |
| op-conductor | 50050 | Conductor 1 Consensus |
| op-conductor2 | 8548 | Conductor 2 RPC |
| op-conductor2 | 50051 | Conductor 2 Consensus |
| op-conductor3 | 8549 | Conductor 3 RPC |
| op-conductor3 | 50052 | Conductor 3 Consensus |
| **Other Services** | | |
| op-batcher | 8548 | Batcher RPC |
| op-proposer | 8560 | Proposer RPC |
