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
| Test window blocks | 8596042 → 8596162 |
| Actual chain time | 120s |
| Saturated | NO — blocks not yet full |

## Transaction summary

| Metric | Value | Notes |
|---|---|---|
| Txs submitted — instance A (est.) | 1479456 | avg BTPS × duration |
| Txs confirmed on-chain | 1446601 | block scan 8596042+1 → 8596162 |
| Txs pending at kill (est.) | 32855 | submitted A − confirmed (B also contributed) |
| Tx errors | 0 | adventure exits on first error |

## Throughput

| Metric | Value | Notes |
|---|---|---|
| **Block-inclusion TPS (avg)** | **12055.0 TX/s** | confirmed TXs / actual chain time |
| Block-inclusion TPS (p50 per block) | 14267.0 TX/s | median single-block TX rate |
| Block-inclusion TPS (p95 per block) | 14267.0 TX/s | 95th percentile single-block |
| Peak confirmed TPS (single block) | 14267.0 TX/s | best single block |
| Theoretical ceiling | 14286 TX/s | 500M gas / ~35k gas per tx |
| Mempool send rate — instance A (avg) | 12328.8 TX/s | adventure `[Summary] Average BTPS` |

### Block fill distribution

| Percentile | Fill % | Notes |
|---|---|---|
| p10 | 56.4% | 90% of blocks at least this full |
| p50 | 100.0% | median block fill |
| p90 | 100.0% | 10% of blocks this full or more |
| avg | 84.5% | mean across all measurement blocks |

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
| T1→T2 engine processor | `rx.recv().await` — reads one message per iteration | `kona/crates/node/engine/src/engine_request_processor.rs` |
| T2 HTTP dispatch | `BuildTask::execute()` → `start_build()` | `kona/crates/node/engine/src/task_queue/tasks/build/task.rs` |
| T3 metric emit | `build_request_start.elapsed()` → `sequencer_build_wait` | `kona/crates/node/sequencer/src/actor.rs` |

```
kona-okx-baseline — Rust async Tokio actors (NO priority fix)
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
        │  │    timestamp, prevRandao, suggestedFeeRecipient,           │
        │  │    transactions: [L1InfoTx], withdrawals: [],              │
        │  │    parentBeaconBlockRoot                                   │
        │  │  }                                                         │
        │  └────────────────────────────────────────────────────────────┘
        │  other Tokio tasks run concurrently during this await
        │
T1 ── build_request_start = Instant::now()            ← engine actor clock STARTS here
      EngineMessage::Build(attrs) sent via mpsc::Sender (non-blocking, instant return)
        │
        │  ┌── Engine actor event loop ─────────────────────────────────────────────┐
        │  │                                                                         │
        │  │  NO flush_pending_messages() — reads ONE message per event loop iter  │
        │  │  rx.recv().await → picks up next pending msg (may be Consolidate)     │
        │  │                                                                         │
        │  │  BinaryHeap after receiving a few individual messages:                 │
        │  │  ┌─────────────────────────────────────────────────────────────────┐  │
        │  │  │  Consolidate     [DERIVATION = priority LOW]  ← dequeued 1st   │  │
        │  │  │  Consolidate     [DERIVATION = priority LOW]  ← dequeued 2nd   │  │
        │  │  │  Build{attrs}    [SEQUENCER  = priority HIGH] ← STARVED        │  │
        │  │  └─────────────────────────────────────────────────────────────────┘  │
        │  │                                                                         │
        │  │  Root cause: Build IS highest priority — but the heap only knows       │
        │  │  about messages already read from the channel one at a time.           │
        │  │  Consolidate tasks already in the heap are dequeued before Build       │
        │  │  is even received from the channel. Build waits until the heap clears. │
        │  │                                                                         │
        │  │  Under sustained full-block load, derivation bursts 3–5 Consolidate   │
        │  │  tasks per block → Build delays up to 37ms at 200M (max).             │
        │  │                                                                         │
        │  └─────────────────────────────────────────────────────────────────────────┘
        │
T2 ── BuildTask::execute() dispatched (after BinaryHeap drains Consolidate tasks)
        │  HTTP POST engine_forkchoiceUpdatedV3 { ... } ──→ reth
        │  ←─ { payloadStatus: "VALID", payloadId: "0x..." }
        │
T3 ── payloadId received
      build_request_start.elapsed() → sequencer_build_wait emitted
      log: "build request completed" sequencer_build_wait=Xms sequencer_total_wait=Xms
```

### Sequencer build latency — T0→T3 timing model

| Phase | p50 | p99 | max |
|---|---|---|---|
| **T0→T3 &nbsp; Sequencer tick → payloadId (full cycle)** | 110.6 ms | 298.3 ms | 331.4 ms |
| &nbsp;&nbsp;▸&nbsp;T0→T1 &nbsp; Payload Prep — L1 fetch + attr construction | 104.0 ms | 224.4 ms | 280.7 ms |
| &nbsp;&nbsp;▸&nbsp;T1→T3 &nbsp; Engine actor wait ¹ | 2.2 ms | 114.4 ms | 194.3 ms |
| &nbsp;&nbsp;&nbsp;&nbsp;◦&nbsp;T1→T2 &nbsp; Heap drain stall | 0.1 ms | 111.2 ms | 192.9 ms |
| &nbsp;&nbsp;&nbsp;&nbsp;◦&nbsp;T2→T3 &nbsp; FCU+attrs HTTP round-trip | 2.0 ms | 36.8 ms | 41.1 ms |

> ¹ **Why sub-rows don't sum to parent:** Each p99/max is the 99th-worst event from its own per-block
> distribution. The worst total-build block is not always the same block as the worst Payload-Prep
> AND worst Engine-actor-wait simultaneously. Per-event values always sum: `total[i] = attr_prep[i] + build_wait[i]`.
>
> **T1→T2 Heap drain stall:** time Build{attrs} spends waiting inside the BinaryHeap for Consolidate tasks
> that arrived earlier to drain first. Distinct from the mpsc channel send at T1 (non-blocking, near-instant).

### Derivation engine calls (reference)

| Call | p50 | p99 | max | n |
|---|---|---|---|---|
| FCU (derivation) | 4.4 ms | 32.6 ms | 77.4 ms | 234 |
| new_payload (derivation) | 67.1 ms | 189.8 ms | 241.1 ms | 138 |
| Block import / seal | 100.8 ms | 404.0 ms | 572.9 ms | 138 |

## Engine API — reth EL log timings

> Extracted from reth docker logs (`engine::tree`). Measures reth's own processing time.

| Call | p50 | p99 | max | n |
|---|---|---|---|---|
| FCU (no attrs) | 4.0 ms | 28.3 ms | 76.7 ms | 188 |
| FCU+attrs | 0.1 ms | 31.9 ms | 39.3 ms | 140 |
| new_payload | 9.7 ms | 27.2 ms | 38.9 ms | 140 |

---
*Generated by bench-adventure.sh · 2026-04-08*
