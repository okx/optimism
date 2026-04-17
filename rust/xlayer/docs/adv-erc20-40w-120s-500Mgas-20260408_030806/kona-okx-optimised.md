# xlayer Adventure ERC20 Benchmark — kona

## Run metadata

| Field | Value |
|---|---|
| Date | 2026-04-08 |
| Chain | xlayer devnet (195) · 1s blocks |
| Consensus layer | kona (Rust) (`op-kona`) |
| Execution layer | OKX reth |
| Block gas limit | 500M gas |
| Warm-up | 30s (dual instances flood mempool) |
| Measurement window | 120s |
| Workers per instance | 40 (2 instances × 40 = 80 total) |
| Gas price | 100 gwei |
| Tx type | ERC20 transfer — 100k gas limit / ~35k gas actual |
| ERC20 contract | `` |
| Accounts | 20,000 (10k per instance, funded 0.2 ETH each) |
| Test window blocks | 8596036 → 8596156 |
| Actual chain time | 120s |
| Saturated | NO — blocks not yet full |

## Transaction summary

| Metric | Value | Notes |
|---|---|---|
| Txs submitted — instance A (est.) | 1448268 | avg BTPS × duration |
| Txs confirmed on-chain | 1401836 | block scan 8596036+1 → 8596156 |
| Txs pending at kill (est.) | 46432 | submitted A − confirmed (B also contributed) |
| Tx errors | 0 | adventure exits on first error |

## Throughput

| Metric | Value | Notes |
|---|---|---|
| **Block-inclusion TPS (avg)** | **11682.0 TX/s** | confirmed TXs / actual chain time |
| Block-inclusion TPS (p50 per block) | 14267.0 TX/s | median single-block TX rate |
| Block-inclusion TPS (p95 per block) | 14267.0 TX/s | 95th percentile single-block |
| Peak confirmed TPS (single block) | 14267.0 TX/s | best single block |
| Theoretical ceiling | 14286 TX/s | 500M gas / ~35k gas per tx |
| Mempool send rate — instance A (avg) | 12068.9 TX/s | adventure `[Summary] Average BTPS` |

### Block fill distribution

| Percentile | Fill % | Notes |
|---|---|---|
| p10 | 41.3% | 90% of blocks at least this full |
| p50 | 100.0% | median block fill |
| p90 | 100.0% | 10% of blocks this full or more |
| avg | 81.9% | mean across all measurement blocks |

> **Note on TPS across baseline vs optimised:** TPS ceiling is `gas_limit / ~35k gas` at 1 block/s — the CL cannot push reth faster than its engine processes blocks. Small TPS differences (±1–5%) between baseline and optimised are run-to-run noise, not regression. The priority fix targets sequencer **tail latency** (queue starvation under load), not throughput.

## Sequencer latency — priority fix evaluation (kona (Rust))

> **Sequencer build wait** is the primary metric for evaluating the priority fix.
> Measured as `build_request_start.elapsed()` in `actor.rs` — from before the sequencer event is sent to the engine channel until `payloadId` is received back.
> Includes BinaryHeap queue wait + FCU+attrs HTTP round-trip. **This is what the priority fix reduces.**
> **FCU+attrs (HTTP only):** just the `engine_forkchoiceUpdatedV3+attrs` call inside `BuildTask::execute()`. Starts AFTER the event is dequeued — does NOT show queue wait.

### Block build cycle — internal flow

| Phase | Function | File (repo: okx-optimism) |
|---|---|---|
| T0→T1 attr prep | `prepare_payload_attributes()` | `kona/crates/node/sequencer/src/actor.rs` |
| T1 timer start | `build_request_start = Instant::now()` | `kona/crates/node/sequencer/src/actor.rs` |
| T1→T2 engine processor | `flush_pending_messages()` | `kona/crates/node/engine/src/engine_request_processor.rs` |
| T2 HTTP dispatch | `BuildTask::execute()` → `start_build()` | `kona/crates/node/engine/src/task_queue/tasks/build/task.rs` |
| T3 metric emit | `build_request_start.elapsed()` → `sequencer_build_wait` | `kona/crates/node/sequencer/src/actor.rs` |

```
kona-okx-optimised — Rust async Tokio actors (flush_pending_messages fix applied)
─────────────────────────────────────────────────────────────────────────────────

T0 ── sequencer tick fires (every 1 second, aligned to L2 block time)
        │
        │  prepare_payload_attributes()               ← async Tokio future, non-blocking
        │  ┌────────────────────────────────────────────────────────────┐
        │  │  eth_getBlockByNumber("latest")  ← L1 RPC (async await)  │
        │  │    → L1 block hash, basefee, timestamp, mix_hash          │
        │  │  construct L1InfoTx (deposit transaction):                 │
        │  │    setL1BlockValues(number, timestamp, basefee,            │
        │  │                    blockHash, seqNum, batcherAddr, ...)    │
        │  │  assemble PayloadAttributes {                              │
        │  │    timestamp        = next_l2_block_time,                 │
        │  │    prevRandao       = l1_mix_hash,                        │
        │  │    suggestedFeeRecipient = sequencer_fee_wallet,           │
        │  │    transactions     = [L1InfoTx],                          │
        │  │    withdrawals      = [],                                  │
        │  │    parentBeaconBlockRoot = l1_parent_beacon_root           │
        │  │  }                                                         │
        │  └────────────────────────────────────────────────────────────┘
        │  other Tokio tasks (derivation, safe-head tracking) run concurrently
        │
T1 ── build_request_start = Instant::now()            ← engine actor clock STARTS here
      EngineMessage::Build(attrs) sent via mpsc::Sender (non-blocking, instant return)
        │
        │  ┌── Engine actor event loop ─────────────────────────────────────────────┐
        │  │                                                                         │
        │  │  flush_pending_messages()                ← KEY FIX                    │
        │  │  ┌─────────────────────────────────────────────────────────────────┐  │
        │  │  │  loop {                                                         │  │
        │  │  │    match self.rx.try_recv() {                                   │  │
        │  │  │      Ok(msg) => self.heap.push(msg),  // drain ALL pending msgs │  │
        │  │  │      Err(_)  => break,                // channel empty, stop   │  │
        │  │  │    }                                                            │  │
        │  │  │  }                                                              │  │
        │  │  └─────────────────────────────────────────────────────────────────┘  │
        │  │  heap now has COMPLETE view of all pending work                        │
        │  │                                                                         │
        │  │  BinaryHeap (max-heap ordered by EngineMessage::priority()):           │
        │  │  ┌─────────────────────────────────────────────────────────────────┐  │
        │  │  │  Build{attrs}    [SEQUENCER_BUILD = priority HIGH]  ← TOP      │  │
        │  │  │  Consolidate     [DERIVATION      = priority LOW ]             │  │
        │  │  │  Consolidate     [DERIVATION      = priority LOW ]             │  │
        │  │  └─────────────────────────────────────────────────────────────────┘  │
        │  │  heap.pop() → Build{attrs}  always wins — never starved               │
        │  │                                                                         │
        │  └─────────────────────────────────────────────────────────────────────────┘
        │
T2 ── BuildTask::execute() dispatched
        │  HTTP POST engine_forkchoiceUpdatedV3:
        │  ┌─────────────────────────────────────────────────────────────────────┐
        │  │  forkchoiceState: {                                                 │
        │  │    headBlockHash:       current_unsafe_head_hash,                  │
        │  │    safeBlockHash:       current_safe_head_hash,                    │
        │  │    finalizedBlockHash:  current_finalized_hash                     │
        │  │  },                                                                 │
        │  │  payloadAttributes: {                                              │
        │  │    timestamp, prevRandao, suggestedFeeRecipient,                   │
        │  │    transactions: [L1InfoTx], withdrawals: [],                      │
        │  │    parentBeaconBlockRoot                                           │
        │  │  }                                                                 │
        │  └─────────────────────────────────────────────────────────────────────┘
        │  reth engine::tree: validates head, starts payload builder goroutine
        │  ←─ HTTP response: { payloadStatus: "VALID", payloadId: "0x1a2b..." }
        │
T3 ── payloadId received
      build_request_start.elapsed() → sequencer_build_wait emitted
      log: "build request completed" sequencer_build_wait=Xms sequencer_total_wait=Xms
      reth independently builds the block; getPayload called at next tick
```

### Sequencer build latency — T0→T3 timing model

| Phase | p50 | p99 | max |
|---|---|---|---|
| **T0→T3 &nbsp; Sequencer tick → payloadId (full cycle)** | 102.1 ms | 270.0 ms | 277.0 ms |
| &nbsp;&nbsp;▸&nbsp;T0→T1 &nbsp; Payload Prep — L1 fetch + attr construction | 100.0 ms | 264.2 ms | 273.3 ms |
| &nbsp;&nbsp;▸&nbsp;T1→T3 &nbsp; Engine actor wait ¹ | 2.2 ms | 98.8 ms | 150.9 ms |
| &nbsp;&nbsp;&nbsp;&nbsp;◦&nbsp;T1→T2 &nbsp; Heap drain stall | 0.1 ms | 97.8 ms | 148.8 ms |
| &nbsp;&nbsp;&nbsp;&nbsp;◦&nbsp;T2→T3 &nbsp; FCU+attrs HTTP round-trip | 2.1 ms | 31.0 ms | 34.3 ms |

> ¹ **Why sub-rows don't sum to parent:** Each p99/max is the 99th-worst event from its own per-block
> distribution. The worst total-build block is not always the same block as the worst Payload-Prep
> AND worst Engine-actor-wait simultaneously. Per-event values always sum: `total[i] = attr_prep[i] + build_wait[i]`.
>
> **T1→T2 Heap drain stall:** time Build{attrs} spends waiting inside the BinaryHeap for Consolidate tasks
> that arrived earlier to drain first. Distinct from the mpsc channel send at T1 (non-blocking, near-instant).

### Derivation engine calls (reference)

| Call | p50 | p99 | max | n |
|---|---|---|---|---|
| FCU (derivation) | 0.9 ms | 6.2 ms | 6.2 ms | 47 |
| new_payload (derivation) | 64.0 ms | 266.4 ms | 322.2 ms | 138 |
| Block import / seal | 104.0 ms | 1019.7 ms | 1085.3 ms | 138 |

## Engine API — reth EL log timings

> Extracted from reth docker logs (`engine::tree`). Measures reth's own processing time.

| Call | p50 | p99 | max | n |
|---|---|---|---|---|
| FCU (no attrs) | 3.8 ms | 52.0 ms | 54.1 ms | 188 |
| FCU+attrs | 0.1 ms | 3.8 ms | 26.6 ms | 140 |
| new_payload | 9.2 ms | 21.6 ms | 29.9 ms | 140 |

---
*Generated by bench-adventure.sh · 2026-04-08*
