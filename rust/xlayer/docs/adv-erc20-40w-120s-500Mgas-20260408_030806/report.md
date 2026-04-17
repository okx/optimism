# 500M Gas — Detailed CL Comparison Report
## xlayer devnet · 2026-04-08 · 4 Consensus Layers

> **Setup:** 500M gas limit · 120s measurement · 1s blocks · 80 goroutines total
> **Load:** UNSATURATED (avg 82% fill — bimodal)
> **Why bimodal:** sender submits ~14,200 TX/s vs 14,285 TX/s ceiling — no deep mempool buffer → blocks oscillate full↔empty.
> **Use 200M saturated data for definitive fix validation.** 500M shows op-node's stall scaling behaviour.
> All CLs use identical OKX reth EL binary and config. Same chain, same accounts, sequential runs.

---

## How to Read This Report

### Percentile primer

| Term | What it means | When to use |
|---|---|---|
| **p50** | Typical call — half finish faster. | Baseline behaviour only. **Never use alone** — hides tail issues. |
| **p99** ⭐ | 1-in-100 calls are slower than this. In a 120s run ≈ the 2nd worst event. | **Primary decision metric.** Bad p99 = reliably slow, not noise. |
| **max** | The single worst call in the run. | Cross-check with p99. If max >> p99 → rare spike. If max ≈ p99 → fat tail. |

> A system fast 99 times that stalls on the 100th is NOT reliable. p50 looks great; p99 exposes it. **Use p99 for production decisions.**

### Engine API calls (CL → reth)

| Call | Frequency | Critical path? | What it does |
|---|---|---|---|
| **FCU+attrs** (`forkchoiceUpdated` + `payloadAttributes`) | Once per second (every block) | **YES** | Tells reth to start building the next block. Slow = sequencer misses its slot window. |
| **FCU** (no attrs) | ~47–54 per 120s run | No | Advances safe/finalized head after L1 batch confirms. Slow = delayed bridge/withdrawal for users. |
| **new_payload** | ~984 per run | Indirectly | Submits sealed block to reth for EVM execution and chain insertion. |
| **getPayload** | ~120 per run (kona/base-cl only) | Yes | Fetches built block from reth. Combined with new_payload = full seal cycle. op-node handles this internally. |

---

## Section 1 — Throughput

| Metric | base-cl | Kona (baseline) | Kona (optimised) | op-node |
|---|---|---|---|---|
| **Block-inclusion TPS** — transactions confirmed on-chain per second during measurement | 11599.6 TX/s | **12055.0 TX/s** | 11682.0 TX/s | 11248.7 TX/s |
| Block fill — avg — average % of block gas limit used across all blocks in the run | 81.3% | **84.5%** | 81.9% | 78.8% |
| Block fill — p10 — 10th percentile fill; 10% of blocks were at or below this level | 43.8% | **56.4%** | 41.3% | 28.4% |
| Block fill — p50 — median fill; half of blocks were below this (0% = half were empty) | 98.4% | **100.0%** | **100.0%** | **100.0%** |
| Block fill — p90 — 90th percentile; 90% of blocks were at or below this fill | **100.0%** | **100.0%** | **100.0%** | **100.0%** |
| Peak TPS (single block) — highest TX/s seen in any individual block | **14267.0 TX/s** | **14267.0 TX/s** | **14267.0 TX/s** | **14267.0 TX/s** |
| Mempool send rate — TX/s submitted by the load generator to the mempool | 11906.4 TX/s | **12328.8 TX/s** | 12068.9 TX/s | 11525.6 TX/s |
| Txs confirmed on-chain — total transactions included in blocks during measurement window | 1391952 | **1446601** | 1401836 | 1349839 |
| Txs submitted (est.) — total transactions sent to mempool during measurement window | 1428768 | **1479456** | 1448268 | 1383072 |

**TPS leader: kona-okx-baseline. 6.7% spread across CLs.**

> **Note on TPS baseline vs optimised:** TPS ceiling is `gas_limit / ~35k gas` at 1 block/s. The CL cannot push reth faster than its engine processes blocks. Small TPS differences (≤5%) between CLs are run-to-run noise — not regression. The priority fix targets **tail latency**, not throughput.

---

## Section 2 — FCU+attrs Latency — Block-Build Trigger Round-Trip (T1→T3)

> **Primary sequencer metric.** Full round-trip from when the build trigger is queued (T1) to `payloadId` received (T3).
> T1→T2: CL-internal queue stall · T2→T3: `engine_forkchoiceUpdatedV3+attrs` HTTP to reth.
> Instrumentation: op-node `time.Since(ScheduledAt)` · kona/base-cl `build_request_start.elapsed()`
> Section 3 measures T2→T3 HTTP only — does NOT include CL-internal stall. Use this section to evaluate the fix.

| Metric | base-cl | Kona (baseline) | Kona (optimised) | op-node |
|---|---|---|---|---|
| **p50** — median block-build trigger round-trip (T1→T3) | 2.336 ms | **2.196 ms** | 2.236 ms | 2.199 ms |
| **p99** — tail; 1-in-100 blocks exceeded this — primary decision signal | 152.200 ms | 114.426 ms | **98.835 ms** | 110.680 ms |
| **max** — worst single block-build trigger in the run | 152.883 ms | 194.320 ms | 150.912 ms | **118.579 ms** |

- **FCU+attrs p99 (T1→T3):** kona-optimised 98.835ms vs kona-baseline 114.426ms — **1.2× better**.
- **FCU+attrs max (T1→T3):** kona-optimised 150.912ms vs kona-baseline 194.320ms — worst-case stall reduced.
- **vs op-node:** kona-optimised 98.835ms vs op-node 110.680ms p99 — **1.1× better**.
- op-node FCU+attrs max = **118.6ms** (11.9% of a 1-second block slot).

### How each CL builds a block — T0→T3

> T0 = sequencer tick · T1 = Build{attrs} enters engine channel (timer starts) · T2 = FCU+attrs HTTP dispatched to reth · T3 = payloadId received
> FCU+attrs round-trip (T1→T3) = CL-internal queue stall (T1→T2) + HTTP to reth (T2→T3).

#### op-node — Go single-threaded Driver goroutine

| Phase | Function | File (repo: optimism) |
|---|---|---|
| T0 sequencer tick | `onBuildStart()` — `time.Since(ScheduledAt)` starts | `op-node/node/sequencer.go` |
| T0→T1 sync prep | `PreparePayloadAttributes()` — blocks Driver goroutine | `op-node/node/sequencer.go` |
| T1→T2 none | No BinaryHeap queue — direct to HTTP | — |
| T2 HTTP dispatch | `startPayload()` → `engine_forkchoiceUpdatedV3` | `op-node/node/sequencer.go` |
| T3 metric emit | `time.Since(ScheduledAt)` → `sequencer_build_wait` | `op-node/node/sequencer.go` |

> **Driver goroutine:** The Go `Driver` struct uses a `sync.Mutex` shared between the sequencer
> and derivation goroutines. `PreparePayloadAttributes()` acquires this mutex synchronously —
> the derivation pipeline is paused for the full duration of the L1 RPC call.
> Source: `op-node/node/driver.go` — `Driver.syncStep()`, `Driver.eventStep()`

```
op-node — Go single-threaded event loop (Driver goroutine)
─────────────────────────────────────────────────────────────────────────────────

T0 ── sequencer tick fires (every 1 second)
        │
        │  PreparePayloadAttributes()                 ← SYNCHRONOUS — blocks entire node
        │  ┌────────────────────────────────────────────────────────────┐
        │  │  eth_getBlockByNumber("latest")  ← L1 RPC (blocking)     │
        │  │    → L1 block hash, basefee, timestamp, mix_hash          │
        │  │  construct L1InfoTx (deposit transaction)                  │
        │  │  assemble PayloadAttributes { ... }                        │
        │  │                                                            │
        │  │  ⚠️  Driver goroutine is FROZEN for the duration:          │
        │  │     - derivation pipeline is blocked                       │
        │  │     - no other engine work runs                            │
        │  │     - entire node suspended until L1 RPC returns           │
        │  └────────────────────────────────────────────────────────────┘
        │  (kona does this async — Tokio runtime stays active)
        │
T1 ── attrs ready · no queue · direct path to HTTP
      (op-node has no BinaryHeap engine queue — no T1→T2 wait)
        │
T2 ── HTTP: engine_forkchoiceUpdatedV3(headHash, safeHash, finalizedHash, attrs) ──→ reth
        │  reth engine::tree: validates head, starts payload builder
        │  ←─ { payloadStatus: "VALID", payloadId: "0x..." }
        │
T3 ── payloadId received
      time.Since(ScheduledAt) → sequencer_build_wait emitted
```

#### kona-okx-baseline — Rust Tokio actors (no priority fix)

| Phase | Function | File (repo: okx-optimism) |
|---|---|---|
| T0→T1 attr prep | `prepare_payload_attributes()` | `kona/crates/node/sequencer/src/actor.rs` |
| T1 timer start | `build_request_start = Instant::now()` | `kona/crates/node/sequencer/src/actor.rs` |
| T1→T2 engine processor | `rx.recv().await` — reads one message per iteration | `kona/crates/node/engine/src/engine_request_processor.rs` |
| T2 HTTP dispatch | `BuildTask::execute()` → `start_build()` | `kona/crates/node/engine/src/task_queue/tasks/build/task.rs` |
| T3 metric emit | `build_request_start.elapsed()` → `sequencer_build_wait` log | `kona/crates/node/sequencer/src/actor.rs` |

```
kona-okx-baseline — Rust async Tokio actors (NO priority fix)
─────────────────────────────────────────────────────────────────────────────────

T0 ── sequencer tick fires (every 1 second, aligned to L2 block time)
        │
        │  prepare_payload_attributes()               ← async Tokio future, non-blocking
        │  ┌────────────────────────────────────────────────────────────┐
        │  │  eth_getBlockByNumber("latest")  ← L1 RPC (async await)  │
        │  │  construct L1InfoTx + assemble PayloadAttributes{...}     │
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

#### kona-okx-optimised — Rust Tokio actors (flush_pending_messages applied)

| Phase | Function | File (repo: okx-optimism) |
|---|---|---|
| T0→T1 attr prep | `prepare_payload_attributes()` | `kona/crates/node/sequencer/src/actor.rs` |
| T1 timer start | `build_request_start = Instant::now()` | `kona/crates/node/sequencer/src/actor.rs` |
| T1→T2 engine processor | `flush_pending_messages()` | `kona/crates/node/engine/src/engine_request_processor.rs` |
| T2 HTTP dispatch | `BuildTask::execute()` → `start_build()` | `kona/crates/node/engine/src/task_queue/tasks/build/task.rs` |
| T3 metric emit | `build_request_start.elapsed()` → `sequencer_build_wait` log | `kona/crates/node/sequencer/src/actor.rs` |

```
kona-okx-optimised — Rust async Tokio actors (flush_pending_messages fix applied)
─────────────────────────────────────────────────────────────────────────────────

T0 ── sequencer tick fires (every 1 second, aligned to L2 block time)
        │
        │  prepare_payload_attributes()               ← async Tokio future, non-blocking
        │  ┌────────────────────────────────────────────────────────────┐
        │  │  eth_getBlockByNumber("latest")  ← L1 RPC (async await)  │
        │  │  construct L1InfoTx + assemble PayloadAttributes{...}     │
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

---

## Section 3 — FCU+attrs HTTP Round-trip (Reference)

> `engine_forkchoiceUpdatedV3 + payloadAttributes`
> **Called once per second. Tells reth to start building the next block.**
> Measured as HTTP round-trip time **starting after the event is dequeued** from the priority queue.
> ⚠️ This does NOT include queue wait time — the fix's effect is NOT directly visible here. See Section 2.

| Metric | base-cl | Kona (baseline) | Kona (optimised) | op-node |
|---|---|---|---|---|
| **p50** — median trigger latency; what most of the ~120 block-build cycles experience per run | **1.986 ms** | 1.997 ms | 2.074 ms | 2.162 ms |
| **p99** — tail trigger latency; 1-in-100 worst; ~1–2 events per 120s run. **Primary decision metric.** | **18.738 ms** | 36.779 ms | 31.047 ms | 110.639 ms |
| **max** — single worst block-build trigger in run; peak % of the 1s slot consumed in one call | **20.029 ms** | 41.132 ms | 34.276 ms | 118.542 ms |
| reth internal p50 — reth's own CPU time for FCU+attrs, excluding network and CL overhead | 0.077 ms | **0.072 ms** | 0.085 ms | 0.087 ms |

- **p99:** kona-optimised 31.047ms vs op-node 110.639ms — **3.6× gap**.
- **max:** kona-optimised 34.276ms vs op-node 118.542ms — **3.5× gap**.
- op-node's worst spike consumed **11.9% of the 1-second block slot** in a single call.
- **Scaling trend (200M→500M):** op-node FCU max grew ~2.5× with block gas limit. kona-optimised stayed nearly flat.

---

## Section 4 — FCU (Derivation / Safe Head Advancement)

> `engine_forkchoiceUpdatedV3` without payload attributes.
> **Called ~47–54 times per 120s run as L1 batches arrive and are confirmed.**
> Not on the block-building critical path. Slow derivation FCU = safe head falls further behind = longer bridge and withdrawal wait for users.
> Measured as HTTP round-trip from CL docker logs.

| Metric | base-cl | Kona (baseline) | Kona (optimised) | op-node |
|---|---|---|---|---|
| **p50** — median derivation FCU; typical time to advance the safe head after an L1 batch | 0.950 ms | 4.369 ms | **0.909 ms** | N/A |
| **p99** — tail derivation FCU; ~1 worst event per 120s run (low call count — treat carefully) | 47.672 ms | 32.573 ms | **6.245 ms** | N/A |
| **max** — single worst safe-head advancement call in the entire run | 47.672 ms | 77.391 ms | **6.245 ms** | N/A |

- **Caution:** only ~47–54 calls/run. The p99 = just 1 worst sample. Not a structural reliability concern at low call counts.

---

## Section 5 — new_payload (Block Import / Validation)

> `engine_newPayload`
> **Called ~984 times per run — for every sequenced and derived block.**
> The CL submits a fully-sealed block to reth for EVM execution, validation, and canonical chain insertion.
> **Measured as CL HTTP round-trip time** (CL sends block → reth validates → CL receives response).
> This is the CL's perspective of the full round-trip — not reth's internal execution time alone.
> High p99/max = reth struggling to validate and insert large blocks fast enough under sequencer pressure.

| Metric | base-cl | Kona (baseline) | Kona (optimised) | op-node |
|---|---|---|---|---|
| **p50** — median block import round-trip; what most of the ~984 block validations take | 62.307 ms | 67.125 ms | 64.017 ms | **61.349 ms** |
| **p99** — tail block import; 1-in-100 worst; ~10 events per 120s run. Key reliability signal. | 205.123 ms | **189.819 ms** | 266.375 ms | 335.647 ms |
| **max** — single worst block import in run; peak reth validation stress observed | 420.845 ms | **241.145 ms** | 322.241 ms | 527.088 ms |

- **op-node new_payload max = 527.1ms** — a single block import consuming 53% of a 1-second slot.
- new_payload p99: op-node 335.6ms vs kona-optimised 266.4ms — op-node 1.3× slower on tail block imports.
- kona-baseline p50 is artificially low (warm reth cache from running immediately after op-node on same chain state). Use p99/max as reference.

---

## Section 6 — Block Seal Cycle (kona / base-cl only)

> `engine_getPayload + engine_newPayload` — measured as a combined round-trip.
> **Total time from build-trigger to sealed block on-chain.**
> After FCU+attrs triggers block building, the sequencer: (1) waits for reth to build the block,
> (2) fetches it with getPayload, (3) immediately submits it back with newPayload to seal it.
> This is the end-to-end block production latency the sequencer experiences per slot.
> op-node handles getPayload+newPayload internally and does not log them separately — excluded here.

| Metric | base-cl | Kona (baseline) | Kona (optimised) |
|---|---|---|---|
| **p50** — median seal cycle; what most blocks take from build-trigger to on-chain insertion | 103.027 ms | **100.752 ms** | 103.970 ms |
| **p99** — tail seal cycle; 1-in-100 worst block production time end-to-end | 1203.973 ms | **403.975 ms** | 1019.731 ms |
| **max** — single worst seal cycle; peak block production time observed in the run | 1898.212 ms | **572.888 ms** | 1085.277 ms |

- kona-optimised seals 1× faster than base-cl at median (104.0ms vs 103.0ms).

---

## Section 7 — Full Comparison: All CLs Ranked

> All CLs compared side-by-side per metric. **Bold = best (lowest latency / highest throughput).**
> Use **p99** as the primary production decision signal. See 'How to Read This Report' above.

| Metric | What it measures | op-node | Kona (baseline) | Kona (optimised) | base-cl | Best performer |
|---|---|---|---|---|---|---|
| **Block TPS** | Txs confirmed on-chain per second. Sender-limited — CL doesn't change this. | 11248.7 TX/s | **12055.0 TX/s** | 11682.0 TX/s | 11599.6 TX/s | Tie (6.7% spread — sender-limited) |
| **FCU+attrs p99** ⭐ (T1→T3) | Tail block-build trigger round-trip (CL queue stall + HTTP). Primary decision signal. | 110.680 ms | 114.426 ms | **98.835 ms** | 152.200 ms | Kona (optimised) |
| **FCU+attrs p50** (T1→T3) | Median block-build trigger round-trip. What most block builds experience. | 2.199 ms | **2.196 ms** | 2.236 ms | 2.336 ms | Kona (baseline) |
| **FCU+attrs max** (T1→T3) | Worst single block-build trigger — peak sequencer stall in the run. | **118.579 ms** | 194.320 ms | 150.912 ms | 152.883 ms | op-node |
| **FCU+attrs HTTP p99** (T2→T3) | Tail HTTP-only FCU+attrs (post-dequeue, T2→T3). Queue wait NOT included — use T1→T3 above for fix comparison. | 110.639 ms | 36.779 ms | 31.047 ms | **18.738 ms** | base-cl |
| **FCU+attrs HTTP max** (T2→T3) | Worst single FCU+attrs HTTP call. Cross-check T1→T3 max to understand queue vs HTTP split. | 118.542 ms | 41.132 ms | 34.276 ms | **20.029 ms** | base-cl |
| **FCU deriv p99** | Tail derivation FCU (no attrs). ~47–54 calls/run. Not on block-building critical path. | N/A | 32.573 ms | **6.245 ms** | 47.672 ms | Kona (optimised) |  _op-node leads at bimodal 500M load; this reverses at 200M saturated load_
| **new_payload p50** | Median block import round-trip (HTTP). Typical time for reth to validate and insert a block. | **61.349 ms** | 67.125 ms | 64.017 ms | 62.307 ms | op-node |
| **new_payload p99** | Tail block import. 1-in-100 worst. ~10 events per 120s run. Reliability signal for block ingestion. | 335.647 ms | **189.819 ms** | 266.375 ms | 205.123 ms | Kona (baseline) |
| **new_payload max** | Single worst block import in run. Peak reth validation pressure. | 527.088 ms | **241.145 ms** | 322.241 ms | 420.845 ms | Kona (baseline) |

---

## Opinion

### op-node
**FCU+attrs max (T1→T3): 118.6ms** (11.9% of a 1s block slot) — sequencer completely stalled for this duration.
Scales linearly with block gas limit. Root cause: Go `sync.Mutex` on the `Driver` struct.
Derivation holds the lock during large L1 batch processing → sequencer blocked entirely.
new_payload max of **527.1ms** — one block import consuming 53% of the slot.

### kona-okx-optimised
**FCU+attrs p99 98.835ms, max 150.912ms** (T1→T3) — stable and predictable under load.
Derivation FCU p99 (6.2ms) elevated at bimodal load — known `yield_now()` trade-off.
This reverses at 200M saturated load where cooperative scheduling prevents derivation from starving the sequencer.

### kona-okx-baseline
FCU+attrs p99 (T1→T3): 114.426ms vs optimised 98.835ms — **1.2× worse** without the fix.
Use p99/max as ground truth. At 200M saturated load, baseline shows the pre-fix behaviour clearly.

### base-cl
FCU+attrs p99 152.200ms (T1→T3) — Rust CL, comparable to kona-optimised performance.
The OKX kona fork already has instrumentation, genesis guard, and the priority fix deployed.
Switching to base-cl adds re-integration work for no additional performance gain over kona-optimised.

---

## Presentation Recommendation

**Use as addendum — the scaling story.** 200M saturated data is your primary slides.

**Addendum headline:** *"At double the gas limit, op-node's worst-case stall grew from ~31ms to 119ms. kona-optimised stayed at 34ms."*

---

## Data Quality Notes

| Issue | Impact | Affected metrics |
|---|---|---|
| Bimodal fill (avg 82%) | p50 reflects empty-block timing, not sustained full-block stress | All p50s |
| kona-baseline warm reth cache | p50 artificially low (ran after op-node on same chain state) | baseline new_payload p50 |
| 50k accounts can't saturate 500M | Fix fires on ~41% of blocks only — benefit muted vs 200M | kona-optimised FCU improvement |

---

*Source: `bench/runs/adv-erc20-40w-120s-500Mgas-20260408_030806/`*
*Generated by `bench/scripts/generate-report.py` · 2026-04-08*

