# Block Transitions: Unsafe → Safe → Finalized

## Overview

In the OP Stack (and xlayer-node), L2 blocks progress through three states:

```
Unsafe → Safe → Finalized
```

This document explains **who** moves blocks between states, **when** it happens, **how** the transitions work, and **expected timelines** on devnet.

---

## Quick Definitions

| State | Meaning | Trust Level |
|-------|---------|-------------|
| **Unsafe** | Sequencer tip — L2 blocks produced by the sequencer but not yet proven on L1 | Lowest — trust the sequencer |
| **Safe** | L2 blocks that have been derived from L1 data (batch on L1 allows reconstruction) | Medium — verifiable from L1 |
| **Finalized** | L2 blocks whose corresponding L1 blocks have been finalized by the beacon chain | Highest — canonical and irreversible |

---

## Who Does What

### Actors and Their Responsibilities

| Actor | Role in Transitions |
|-------|---------------------|
| **kona-node** (consensus) | Produces unsafe blocks, derives safe blocks from L1, marks finalized blocks based on L1 finality |
| **reth / xlayer-node** (execution) | Executes transactions, seals blocks, stores state |
| **op-batcher** | Reads unsafe L2 blocks, batches them, submits batch transactions to L1 |
| **L1 geth** (execution) | Mines/includes batch transactions on L1 |
| **L1 beacon chain** | Finalizes L1 blocks via PoS consensus (attestations) |

---

## Transition Flow (Step-by-Step)

### 1. TX → Unsafe (~ 1 second)

**What happens:**
- User sends a transaction to the L2 RPC (port 8123).
- Transaction enters reth's mempool.
- Kona calls `engine_forkchoiceUpdatedV3` with payload attributes → reth starts building a block.
- Reth seals the block and returns a `PayloadId`.
- Kona retrieves the payload via `engine_getPayloadV3/V4` and executes it with `engine_newPayloadV4`.
- Kona updates the **unsafe head** to the new block.

**Timeline:**
- **Devnet:** ~1 second per block (configured block time)
- **Actor:** kona-node + reth (in-process)

**Check:**
```bash
# Query rollup RPC for unsafe head
curl -s -X POST http://localhost:9545 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' \
  | jq '.result.unsafe_l2.number'
```

---

### 2. Unsafe → Batch Submitted to L1 (~ 2–10 seconds)

**What happens:**
- `op-batcher` polls the L2 rollup RPC (port 9545) and execution RPC (port 8123) every 2 seconds.
- It detects new unsafe L2 blocks that haven't been batched yet.
- It accumulates blocks into a batch (compression: brotli-10), respecting `--max-channel-duration` (devnet: 10 blocks).
- It submits a batch transaction to L1 geth via `eth_sendRawTransaction`.
- The batch TX is broadcast and eventually mined into an L1 block.

**Timeline:**
- **Poll interval:** 2s (configurable via `--poll-interval`)
- **L1 block time:** ~8s (devnet Prysm)
- **Total:** ~2–10s from unsafe block production to L1 inclusion

**Configuration knobs:**
- `BATCHER_MAX_CHANNEL_DURATION` (default: 10 blocks)
- `BATCHER_NUM_CONFIRMATIONS` (default: 1)
- `--poll-interval` (default: 2s)

**Check:**
```bash
# View op-batcher logs for batch submissions
docker logs -f op-batcher

# Check batcher nonce on L1 (increases with each batch TX)
BATCHER_PK=$(grep ^OP_BATCHER_PRIVATE_KEY config/devnet/.env | cut -d= -f2)
BATCHER_ADDR=$(cast wallet address "$BATCHER_PK")
cast nonce --rpc-url http://localhost:8545 $BATCHER_ADDR
```

---

### 3. L1 Inclusion → Kona Derives → Safe (~ 8–20 seconds total)

**What happens:**
- Kona monitors L1 via beacon API (port 3500) and execution RPC (port 8545).
- When a new L1 block appears containing a batch transaction, kona waits for a configured number of L1 confirmations (typically 5 blocks to avoid reorg issues).
- Once confirmed, kona runs its **L1 derivation pipeline**:
  1. Reads the batch transaction from L1.
  2. Decompresses and decodes the batch data.
  3. Reconstructs the L2 blocks from the batch.
  4. Validates that the reconstructed blocks match the previously produced unsafe blocks.
  5. Calls `engine_newPayload` and `engine_forkchoiceUpdated` to advance reth's safe head.
- Kona updates the **safe head** to include the derived blocks.

**Timeline:**
- **L1 confirmations wait:** ~40s (5 confirmations × ~8s L1 block time)
- **Derivation processing:** < 1s
- **Total from unsafe:** ~8–20s (depends on when batch was submitted + confirmations)

**Configuration knobs:**
- L1 confirmations (rollup config, typically 5)
- L1 block time (devnet: ~8s)

**Check:**
```bash
# Query safe head
curl -s -X POST http://localhost:9545 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' \
  | jq '.result.safe_l2.number'

# Compute safe lag (blocks behind unsafe)
curl -s -X POST http://localhost:9545 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' \
  | jq '{unsafe: .result.unsafe_l2.number, safe: .result.safe_l2.number}'
```

---

### 4. Safe → Finalized (~ 20–40 seconds additional)

**What happens:**
- The L1 beacon chain finalizes L1 blocks after **two epochs** of attestations from validators.
- Kona monitors L1 beacon finality via the beacon REST API (port 3500).
- When an L1 block (or its descendants) containing a batch is finalized, kona marks the corresponding derived L2 blocks as **finalized**.
- Kona updates the **finalized head**.

**Timeline:**
- **L1 finality:** ~20–40s on devnet (depends on epoch length and validator set)
- **Total from unsafe:** ~30–60s (cumulative)

**Check:**
```bash
# Query finalized head
curl -s -X POST http://localhost:9545 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' \
  | jq '.result.finalized_l2.number'

# Check L1 beacon finalized checkpoint
curl -s http://localhost:3500/eth/v1/beacon/states/finalized/finality_checkpoints \
  | jq .
```

---

## Timeline Summary (Devnet)

| Transition | Time from Previous | Cumulative Time | Bottleneck |
|------------|-------------------|-----------------|------------|
| TX → Unsafe | ~1s | ~1s | Sequencer block time |
| Unsafe → Batch submitted | ~2–10s | ~3–11s | Batcher poll + L1 mining |
| Batch submitted → Safe | ~8–20s | ~11–31s | L1 confirmations + derivation |
| Safe → Finalized | ~20–40s | ~31–71s | L1 beacon finality |

**Realistic devnet numbers:**
- Unsafe: immediate (< 1s after TX sent)
- Safe: 10–30s after TX
- Finalized: 30–70s after TX

---

## Configuration Knobs That Affect Timing

### Op-Batcher Settings (config/devnet/.env)

```bash
BATCHER_MAX_CHANNEL_DURATION=10        # Max blocks per batch
BATCHER_NUM_CONFIRMATIONS=1            # L1 confirmations before considering batch "sent"
```

### Op-Batcher Flags (docker/docker-compose.devnet.yml)

```yaml
--poll-interval=2s                     # How often to poll for new unsafe blocks
--num-confirmations=1                  # L1 confirmations to wait
--max-channel-duration=10              # Max blocks per channel/batch
```

### Rollup Config (config/devnet/rollup.json or xlayer-node.toml)

- `channel_timeout`: How long to wait before closing a channel
- `l1_confirmations`: Number of L1 blocks to wait before deriving (default: 5)

### L1 Settings

- **L1 block time:** Controlled by Prysm beacon config (~8s devnet)
- **Finality epochs:** PoS finality (~2 epochs, ~20–40s devnet)

---

## Observing Transitions Live

### Single Snapshot

```bash
./scripts/devnet/4-health-check.sh --once
```

Output includes:
- Unsafe head
- Safe head (with lag)
- Finalized head (with lag)
- op-batcher status (container, admin RPC, L1 nonce/balance)

### Continuous Monitoring

```bash
./scripts/devnet/4-health-check.sh
```

Refreshes every 5s (or use `--watch 2` for 2s refresh).

### Watch Heads with `watch`

```bash
watch -n 1 'curl -s -X POST http://localhost:9545 \
  -H "Content-Type: application/json" \
  -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"optimism_syncStatus\",\"params\":[]}" \
  | jq "{unsafe:.result.unsafe_l2.number, safe:.result.safe_l2.number, finalized:.result.finalized_l2.number}"'
```

---

## Troubleshooting Stalled Transitions

### If Safe Head Is Stuck (Not Advancing)

**Checklist:**
1. Is `op-batcher` running?
   ```bash
   docker ps --filter "name=op-batcher"
   ```

2. Is op-batcher submitting TXs to L1?
   ```bash
   docker logs -f op-batcher
   # Look for "submitted batch" or TX hashes
   ```

3. Check batcher L1 nonce (should increase over time):
   ```bash
   BATCHER_PK=$(grep ^OP_BATCHER_PRIVATE_KEY config/devnet/.env | cut -d= -f2)
   BATCHER_ADDR=$(cast wallet address "$BATCHER_PK")
   cast nonce --rpc-url http://localhost:8545 $BATCHER_ADDR
   ```

4. Check batcher L1 balance (needs funds):
   ```bash
   cast balance --rpc-url http://localhost:8545 $BATCHER_ADDR
   ```

5. Is L1 producing blocks?
   ```bash
   cast bn --rpc-url http://localhost:8545
   # Run multiple times; block number should increase
   ```

6. Can kona reach L1 RPC and beacon?
   ```bash
   # Check xlayer-node logs for L1 connection errors
   tail -f logs/xlayer-node.log | grep -i "l1\|beacon\|derive"
   ```

### If Finalized Head Is Stuck

**Checklist:**
1. Is L1 beacon finalizing blocks?
   ```bash
   curl -s http://localhost:3500/eth/v1/beacon/states/finalized/finality_checkpoints \
     | jq '.data.finalized'
   # Run multiple times; epoch should increase
   ```

2. Are L1 validators running?
   ```bash
   docker ps --filter "name=l1-validator"
   docker logs -f l1-validator
   ```

3. Is kona receiving L1 finality updates?
   ```bash
   # Check xlayer-node logs for finality-related messages
   tail -f logs/xlayer-node.log | grep -i "final"
   ```

---

## Diagram: Complete Flow

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         L2 Block Lifecycle                                   │
└─────────────────────────────────────────────────────────────────────────────┘

  User TX
     │
     ▼
┌─────────────────────┐
│   reth mempool      │
│   (port 8123)       │
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐      ┌──────────────────────┐
│   kona-node         │◄────►│   reth (execution)   │
│   (consensus)       │      │   Engine API         │
│   port 9545         │      │   (in-process)       │
└──────────┬──────────┘      └──────────────────────┘
           │
           │ forge block, seal
           ▼
     ┌──────────┐
     │  UNSAFE  │  ◄─── Sequencer tip (trust sequencer)
     └─────┬────┘
           │
           │ op-batcher polls every 2s
           ▼
┌─────────────────────┐
│   op-batcher        │
│   (reads L2,        │
│    posts to L1)     │
└──────────┬──────────┘
           │
           │ batch TX submitted
           ▼
┌─────────────────────┐      ┌──────────────────────┐
│   L1 geth           │◄────►│  L1 beacon chain     │
│   (port 8545)       │      │  (port 3500)         │
└──────────┬──────────┘      └──────────┬───────────┘
           │                             │
           │ batch mined                 │ finalize L1 blocks
           ▼                             ▼
     ┌──────────┐              ┌─────────────────┐
     │   SAFE   │              │   FINALIZED     │
     └──────────┘              └─────────────────┘
           ▲                             ▲
           │                             │
           └─────────────────────────────┘
              kona derives from L1 + monitors finality
```

---

## Key Takeaways

1. **Unsafe is instant** — produced by the sequencer (kona + reth) as soon as a block is sealed (~1s).

2. **Safe requires L1 proof** — op-batcher must submit a batch to L1, and kona must derive the L2 blocks from that L1 data (~10–30s).

3. **Finalized requires L1 finality** — L1 beacon chain must finalize the L1 block containing the batch (~30–70s total).

4. **Bottlenecks:**
   - Unsafe → Safe: op-batcher submission rate, L1 block time, L1 confirmations
   - Safe → Finalized: L1 beacon finality (epochs, attestations)

5. **Health check is your friend:**
   ```bash
   ./scripts/devnet/4-health-check.sh
   ```
   Now includes op-batcher status, making it easy to spot issues.

---

## Related Documentation

- [OP Stack Finality](./op-stack-finality.md) — Deep dive into OP Stack finality semantics
- [L2 Explainer](./l2-explainer.md) — High-level L2 architecture
- [Devnet Commands](../commands/devnet-commands.md) — Quick reference for common devnet operations
- [Devnet Ports](../host-port/devnet-ports.md) — Port mapping reference

---

## Further Reading

- [OP Stack Specs: Derivation](https://specs.optimism.io/protocol/derivation.html)
- [OP Stack Specs: Batcher](https://specs.optimism.io/protocol/batcher.html)
- [Ethereum Beacon Chain Finality](https://ethereum.org/en/developers/docs/consensus-mechanisms/pos/#finality)

