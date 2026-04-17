# OP Stack Finality: Unsafe → Safe → Finalized

Every L2 transaction passes through three confidence levels before it is irreversible.
These are not separate systems — they are phases that the same block progresses through
over time, driven by different parts of the stack.

---

## The Three Phases

```
User sends TX
      │
      ▼
xlayer-node (reth) includes it in the next L2 block
      │
      ├──► UNSAFE
      │         Block exists only in the sequencer's memory.
      │         Not submitted to L1 yet. Sequencer said it happened — trust it for now.
      │         Re-org possible if the sequencer crashes or equivocates.
      │
      │    op-batcher collects completed L2 blocks, compresses them,
      │    and submits a batch transaction to L1.
      │    kona reads that L1 transaction and advances its safe head.
      │
      ├──► SAFE
      │         The L2 block data now exists on L1 as calldata/blobs.
      │         Anyone with an L1 node can independently re-derive this L2 block.
      │         Re-org only possible if L1 itself re-orgs (very unlikely).
      │
      │    The L1 block that contains the batcher transaction accumulates
      │    ~64 more L1 blocks (two Ethereum epochs ≈ 12.8 min on mainnet,
      │    faster on devnet). kona watches L1 finality and advances its finalized head.
      │
      └──► FINALIZED
                The L1 block is finalized by Ethereum's proof-of-stake consensus.
                Cannot be reverted. Done.
```

### When does each transition happen?

| Phase | Trigger | Devnet | Mainnet |
|-------|---------|--------|---------|
| **UNSAFE** | reth seals the L2 block (~1s on X Layer) | ~1s | ~2s |
| **SAFE** | op-batcher submits batch to L1 AND kona reads it | ~20–120s | ~1–5 min |
| **FINALIZED** | L1 block containing the batch reaches PoS finality | ~1–3 min | ~13 min |

### What eats the time in each transition?

```
TX submit
  │
  │  reth sequencer: waits up to 1s to fill block, then seals
  ▼ ~1s
UNSAFE
  │
  │  op-batcher: polls L2 every ~2s for new blocks
  │  op-batcher: accumulates blocks into a channel (up to BATCHER_MAX_CHANNEL_DURATION L1 blocks)
  │  op-batcher: compresses + submits batch TX to L1
  │  L1: mines the batch TX into an L1 block (~2-4s on devnet, 12s on mainnet)
  │  kona: polls L1, reads the batch TX, derives L2 blocks, advances safe head
  ▼ ~20–120s (devnet)  /  ~1–5 min (mainnet)
SAFE
  │
  │  L1 PoS consensus: waits for 2 Ethereum epochs (~64 L1 blocks)
  │  = 64 × 2s ≈ 2min on devnet  /  64 × 12s ≈ 12.8min on mainnet
  │  kona: sees L1 finality signal, advances finalized head
  ▼ ~1–3 min (devnet)  /  ~13 min (mainnet)
FINALIZED
```

**Why SAFE can be slow on a fresh devnet restart**: op-batcher must work through any backlog
of unsealed L2 blocks before reaching your TX's block. After the batcher catches up, SAFE
for new TXs typically reaches in 20–40s on devnet.

---

## Where Does This Information Live?

This is the part that confuses most people. There are three ports — they serve completely
different purposes and give you different information.

```
┌──────────────────────────────────────────────────────────────────────┐
│                         xlayer-node process                           │
│                                                                        │
│   ┌─────────────────────┐  in-memory channel  ┌────────────────────┐ │
│   │  reth (execution)   │◄───────────────────►│  kona (consensus)  │ │
│   │                     │   (no TCP — direct   │                    │ │
│   │  builds blocks      │    function calls)   │  watches L1        │ │
│   │  executes TXs       │                      │  tracks safe head  │ │
│   │  serves eth_* API   │                      │  serves rollup API │ │
│   └─────────────────────┘                      └────────────────────┘ │
│            │                                            │              │
└────────────┼────────────────────────────────────────── ┼──────────────┘
             │                                            │
          :8123                                        :9545
     execution RPC                               rollup RPC (kona)
     eth_blockNumber                             optimism_syncStatus
     eth_getTransaction*                         optimism_outputAtBlock
     eth_sendRawTransaction                      optimism_rollupConfig
     (unsafe head only)                          (ALL THREE heads)
```

```
:8552  Engine API — internal, JWT-protected, localhost only (127.0.0.1)
       The private channel for any EXTERNAL consensus client to talk to reth.
       Embedded kona does NOT use this port — it uses the in-memory channel.
       If a request arrives without a valid JWT → 401, connection dropped.
```

### Port 8123 vs port 9545

- **Port 8123** (`eth_blockNumber`) only ever returns the **unsafe** head. It is a standard
  execution layer RPC — it has no concept of safe or finalized.

- **Port 9545** (`optimism_syncStatus`) returns **all three heads** simultaneously. This is
  kona's external API. If port 9545 is down, safe and finalized are invisible to you.

---

## Querying the Heads Directly

```bash
# All three heads in one call — always L2 block numbers
curl -s -X POST http://localhost:9545 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' \
  | jq '.result | {
      unsafe_l2:    .unsafe_l2.number,
      safe_l2:      .safe_l2.number,
      finalized_l2: .finalized_l2.number,
      current_l1:   .current_l1.number
    }'
```

```json
{
  "unsafe_l2":    "0x5391",   ← highest L2 block reth has built
  "safe_l2":      "0x5370",   ← highest L2 block op-batcher has submitted to L1
  "finalized_l2": "0x5280",   ← highest L2 block whose L1 batch is finalized
  "current_l1":   "0x390"     ← how far kona has read on L1
}
```

All four numbers are in hex. All comparisons for unsafe/safe/finalized are done in
**L2 block numbers only**. You never need to look at L1 block numbers to check a TX's
phase — kona does the L1↔L2 correlation internally.

---

## How test-tx.sh Checks Each Phase

The script does three things: send, find the block, then poll until each head catches up.

### Step 1 — Send the TX and record submit timestamp

```bash
TS_BEFORE=$(date +%s)
TX_HASH=$(cast send \
    --rpc-url http://localhost:8123 \
    --private-key "$TEST_SENDER_KEY" \
    "$TEST_RECIPIENT" --value "0.01ether" \
    --json | jq -r '.transactionHash')
```

### Step 2 — Poll optimism_syncStatus until each head passes the TX's block

```bash
# After UNSAFE: get the L2 block the TX landed in
BLOCK_NUM=$(cast receipt --rpc-url http://localhost:8123 "$TX_HASH" \
    | grep "^blockNumber" | awk '{print $2}')  # e.g. 8595267

# Call kona's rollup RPC (port 9545) — has all three heads
STATUS=$(curl -s -X POST http://localhost:9545 \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}')

# TX is SAFE when:  BLOCK_NUM ≤ safe_l2.number
# TX is FINALIZED when: BLOCK_NUM ≤ finalized_l2.number
```

### Step 3 — Default output

```
0xcc4cc6...f1
  UNSAFE      ✅ +0s
  SAFE        ✅ +115s
  FINALIZED   ✅ +63s
```

### Step 4 — Verbose output (`--verbose` flag)

```bash
./scripts/devnet/test-tx.sh --verbose
```

Adds per-component detail after each phase, plus a total breakdown:

```
0xcc4cc6aba1bf0c2b5ddaf98c0a69e4f53be16df6b51d4a146f67dd4fbeca37f1
  UNSAFE      ✅ +0s
    └─ L2 block 8595267 | TX→block sealed: ~2s  [reth sequencer]
  SAFE        ✅ +115s
    └─ L1 current block ~1118 | op-batcher submitted batch, kona derived  [batcher + kona]
  FINALIZED   ✅ +63s
    └─ L1 current block ~1147 | 29 L1 blocks for PoS finality  [L1 consensus]

  ┌─ Component breakdown ──────────────────────────────────────
  │  TX submit → UNSAFE     (reth seals block):    ~0s
  │  UNSAFE    → SAFE       (op-batcher + kona):   ~115s
  │  SAFE      → FINALIZED  (L1 PoS finality):     ~63s
  └────────────────────────────────────────────────────────────
     Total TX submit → FINALIZED:                   ~178s
```

The L1 block numbers tell you exactly where in the L1 chain each transition was anchored.
29 L1 blocks at ~2s each = ~58s for devnet PoS finality (vs. 64 × 12s ≈ 13 min on mainnet).

Exit code `0` = all TXs reached FINALIZED. Exit code `1` = any phase timed out.

---

## What Abnormal Looks Like

| Symptom | Cause | Fix |
|---------|-------|-----|
| `safe == unsafe`, never advancing | op-batcher not running or not submitting to L1 | Check batcher logs; restart batcher |
| `finalized` far behind `safe` | Normal — L1 finality takes time | Wait; expected on devnet |
| `unsafe` stuck | reth stopped producing blocks | Check `logs/xlayer-node.log` |
| All three heads at `0` | kona not running / port 9545 not responding | Check xlayer-node process |
| Port 9545 connection refused | xlayer-node not started yet | Start the node first |

A **safe lag** (unsafe − safe) of 10–100 blocks is normal on devnet.
Above 200 blocks, op-batcher is behind (gas spike, crash, misconfiguration, or backlog from
a fresh restart — give it a few minutes to catch up before investigating).

---

## Why Pre-Warming Only Cares About Unsafe

The pre-warming feature (`crates/transaction-pool/src/pre_warming/`) operates entirely
in the **unsafe window** — the ~400ms between when the sequencer starts building a block
and when it seals it. Pre-warmed cache entries must be ready before `build_payload` is
called. Safe and finalized are irrelevant to pre-warming: by the time a block is safe,
it was built and sealed seconds ago.

---

## Further Reading

- [OP Stack specs — L2 chain derivation](https://specs.optimism.io/protocol/derivation.html)
- [OP Stack specs — Sequencer](https://specs.optimism.io/protocol/sequencer.html)
- `docs/concepts/main-rs-deep-dive.md` — xlayer-node binary entrypoint and component wiring
- `docs/concepts/l2-explainer.md` — what all the L2 URLs and ports are
