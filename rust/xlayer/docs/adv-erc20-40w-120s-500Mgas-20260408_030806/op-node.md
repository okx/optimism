# xlayer Adventure ERC20 Benchmark — op-node

## Run metadata

| Field | Value |
|---|---|
| Date | 2026-04-08 |
| Chain | xlayer devnet (195) · 1s blocks |
| Consensus layer | Go op-node (`op-seq`) |
| Execution layer | OKX reth |
| Block gas limit | N/A gas |
| Warm-up | 30s (dual instances flood mempool) |
| Measurement window | 120s |
| Workers per instance | 40 (2 instances × 40 = 80 total) |
| Gas price | 100 gwei |
| Tx type | ERC20 transfer — 100k gas limit / ~35k gas actual |
| ERC20 contract | `` |
| Accounts | 20,000 (10k per instance, funded 0.2 ETH each) |
| Test window blocks | 8602156 → 8602276 |
| Actual chain time | N/A |
| Saturated | NO — blocks not yet full |

## Transaction summary

| Metric | Value | Notes |
|---|---|---|
| Txs submitted — instance A (est.) | 1383072 | avg BTPS × duration |
| Txs confirmed on-chain | N/A | block scan 8602156+1 → 8602276 |
| Txs pending at kill (est.) | N/A | submitted A − confirmed (B also contributed) |
| Tx errors | 0 | adventure exits on first error |

## Throughput

| Metric | Value | Notes |
|---|---|---|
| **Block-inclusion TPS (avg)** | **N/A TX/s** | confirmed TXs / actual chain time |
| Block-inclusion TPS (p50 per block) | N/A TX/s | median single-block TX rate |
| Block-inclusion TPS (p95 per block) | N/A TX/s | 95th percentile single-block |
| Peak confirmed TPS (single block) | N/A TX/s | best single block |
| Theoretical ceiling | 5714 TX/s | N/A gas / ~35k gas per tx |
| Mempool send rate — instance A (avg) | 11525.6 TX/s | adventure `[Summary] Average BTPS` |

### Block fill distribution

| Percentile | Fill % | Notes |
|---|---|---|
| p10 | N/A | 90% of blocks at least this full |
| p50 | N/A | median block fill |
| p90 | N/A | 10% of blocks this full or more |
| avg | N/A | mean across all measurement blocks |

> **Note on TPS across baseline vs optimised:** TPS ceiling is `gas_limit / ~35k gas` at 1 block/s — the CL cannot push reth faster than its engine processes blocks. Small TPS differences (±1–5%) between baseline and optimised are run-to-run noise, not regression. The priority fix targets sequencer **tail latency** (queue starvation under load), not throughput.

## Sequencer latency — priority fix evaluation (Go op-node)

> **Sequencer build wait** is the primary metric for evaluating the priority fix.
> Measured as `time.Since(ScheduledAt)` in `onBuildStart()` — from the moment the sequencer decides to build a block until `payloadId` is received back from the engine.
> Includes event dispatch time + FCU+attrs HTTP round-trip. Captures any scheduler delay.
> **FCU+attrs (HTTP only):** just the `engine_forkchoiceUpdatedV3+attrs` call inside `startPayload()`. Does NOT include scheduling delay.

### Block build cycle — internal flow

| Phase | Function | File (repo: optimism) |
|---|---|---|
| T0→T1 sync prep | `PreparePayloadAttributes()` | `op-node/node/sequencer.go` |
| T0→T3 timer | `time.Since(ScheduledAt)` | `op-node/node/sequencer.go` |
| T2 HTTP dispatch | `startPayload()` | `op-node/node/sequencer.go` |
| Driver mutex | `Driver.syncStep()` · `Driver.eventStep()` | `op-node/node/driver.go` |

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

### Sequencer build latency — T0→T3 timing model

| Phase | p50 | p99 | max |
|---|---|---|---|
| **T0→T3 &nbsp; Sequencer tick → payloadId (full cycle)** | 248.6 ms | 455.2 ms | 528.0 ms |
| &nbsp;&nbsp;▸&nbsp;T0→T1 &nbsp; Payload Prep — L1 fetch + attr construction | 246.8 ms | 453.1 ms | 522.4 ms |
| &nbsp;&nbsp;▸&nbsp;T1→T3 &nbsp; Engine actor wait ¹ | 2.2 ms | 110.7 ms | 118.6 ms |
| &nbsp;&nbsp;&nbsp;&nbsp;◦&nbsp;T2→T3 &nbsp; FCU+attrs HTTP round-trip | 2.2 ms | 110.6 ms | 118.5 ms |

> ¹ **Why sub-rows don't sum to parent:** Each p99/max is the 99th-worst event from its own per-block
> distribution. The worst total-build block is not always the same block as the worst Payload-Prep
> AND worst Engine-actor-wait simultaneously. Per-event values always sum: `total[i] = attr_prep[i] + build_wait[i]`.
>
> **T1→T2 Heap drain stall:** time Build{attrs} spends waiting inside the BinaryHeap for Consolidate tasks
> that arrived earlier to drain first. Distinct from the mpsc channel send at T1 (non-blocking, near-instant).

### Derivation engine calls (reference)

| Call | p50 | p99 | max | n |
|---|---|---|---|---|
| FCU (derivation) | N/A | N/A | N/A | — |
| new_payload | 61.3 ms | 335.6 ms | 527.1 ms | 124 |

## Engine API — reth EL log timings

> Extracted from reth docker logs (`engine::tree`). Measures reth's own processing time.

| Call | p50 | p99 | max | n |
|---|---|---|---|---|
| FCU (no attrs) | 3.6 ms | 102.4 ms | 177.9 ms | 176 |
| FCU+attrs | 0.1 ms | 108.6 ms | 116.2 ms | 125 |
| new_payload | 9.4 ms | 45.9 ms | 50.2 ms | 125 |

---
*Generated by bench-adventure.sh · 2026-04-08*
