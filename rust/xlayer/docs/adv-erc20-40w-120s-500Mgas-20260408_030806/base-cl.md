# xlayer Adventure ERC20 Benchmark — base-consensus

## Run metadata

| Field | Value |
|---|---|
| Date | 2026-04-08 |
| Chain | xlayer devnet (195) · 1s blocks |
| Consensus layer | base-consensus (Rust) (`op-base-cl`) |
| Execution layer | OKX reth |
| Block gas limit | 500M gas |
| Warm-up | 30s (dual instances flood mempool) |
| Measurement window | 120s |
| Workers per instance | 40 (2 instances × 40 = 80 total) |
| Gas price | 100 gwei |
| Tx type | ERC20 transfer — 100k gas limit / ~35k gas actual |
| ERC20 contract | `` |
| Accounts | 20,000 (10k per instance, funded 0.2 ETH each) |
| Test window blocks | 8596039 → 8596159 |
| Actual chain time | 120s |
| Saturated | NO — blocks not yet full |

## Transaction summary

| Metric | Value | Notes |
|---|---|---|
| Txs submitted — instance A (est.) | 1428768 | avg BTPS × duration |
| Txs confirmed on-chain | 1391952 | block scan 8596039+1 → 8596159 |
| Txs pending at kill (est.) | 36816 | submitted A − confirmed (B also contributed) |
| Tx errors | 0 | adventure exits on first error |

## Throughput

| Metric | Value | Notes |
|---|---|---|
| **Block-inclusion TPS (avg)** | **11599.6 TX/s** | confirmed TXs / actual chain time |
| Block-inclusion TPS (p50 per block) | 14039.0 TX/s | median single-block TX rate |
| Block-inclusion TPS (p95 per block) | 14267.0 TX/s | 95th percentile single-block |
| Peak confirmed TPS (single block) | 14267.0 TX/s | best single block |
| Theoretical ceiling | 14286 TX/s | 500M gas / ~35k gas per tx |
| Mempool send rate — instance A (avg) | 11906.4 TX/s | adventure `[Summary] Average BTPS` |

### Block fill distribution

| Percentile | Fill % | Notes |
|---|---|---|
| p10 | 43.8% | 90% of blocks at least this full |
| p50 | 98.4% | median block fill |
| p90 | 100.0% | 10% of blocks this full or more |
| avg | 81.3% | mean across all measurement blocks |

> **Note on TPS across baseline vs optimised:** TPS ceiling is `gas_limit / ~35k gas` at 1 block/s — the CL cannot push reth faster than its engine processes blocks. Small TPS differences (±1–5%) between baseline and optimised are run-to-run noise, not regression. The priority fix targets sequencer **tail latency** (queue starvation under load), not throughput.

## Sequencer latency — priority fix evaluation (base-consensus (Rust))

> **Sequencer build wait** is the primary metric for evaluating the priority fix.
> Measured as `build_request_start.elapsed()` in `actor.rs` — from before the sequencer event is sent to the engine channel until `payloadId` is received back.
> Includes BinaryHeap queue wait + FCU+attrs HTTP round-trip. **This is what the priority fix reduces.**
> **FCU+attrs (HTTP only):** just the `engine_forkchoiceUpdatedV3+attrs` call inside `BuildTask::execute()`. Starts AFTER the event is dequeued — does NOT show queue wait.

### Block build cycle — internal flow

```
base-cl — Rust async Tokio actors (BinaryHeap, NO fix applied)
─────────────────────────────────────────────────────────────────────────────────

T0 ── sequencer tick fires
        │  prepare_payload_attributes()  — async Tokio (same pattern as kona)
        │
T1 ── Build{attrs} → mpsc::Sender → engine actor channel
        │
        │  ┌── Engine actor (NO flush_pending_messages) ──────────────────────────┐
        │  │  BinaryHeap — derivation Consolidate flood blocks Build dispatch:    │
        │  │  ┌─────────────────────────────────────────────────────────────────┐ │
        │  │  │  Consolidate [LOW]  ← floods queue under sustained load         │ │
        │  │  │  Consolidate [LOW]  ← dequeued one at a time                    │ │
        │  │  │  Build{attrs}[HIGH] ← STARVED — worst case 331ms at 200M        │ │
        │  │  └─────────────────────────────────────────────────────────────────┘ │
        │  └────────────────────────────────────────────────────────────────────────┘
        │
T2 ── HTTP POST engine_forkchoiceUpdatedV3 { ... } ──→ reth
        │  ←─ { payloadId: "0x..." }
        │
T3 ── payloadId received

### Sequencer build latency — T0→T3 timing model

| Phase | p50 | p99 | max |
|---|---|---|---|
| **T0→T3 &nbsp; Sequencer tick → payloadId (full cycle)** | 108.7 ms | 333.6 ms | 394.1 ms |
| &nbsp;&nbsp;▸&nbsp;T0→T1 &nbsp; Payload Prep — L1 fetch + attr construction | 105.1 ms | 326.2 ms | 376.1 ms |
| &nbsp;&nbsp;▸&nbsp;T1→T3 &nbsp; Engine actor wait ¹ | 2.3 ms | 152.2 ms | 152.9 ms |
| &nbsp;&nbsp;&nbsp;&nbsp;◦&nbsp;T1→T2 &nbsp; Heap drain stall | 0.1 ms | 142.4 ms | 151.4 ms |
| &nbsp;&nbsp;&nbsp;&nbsp;◦&nbsp;T2→T3 &nbsp; FCU+attrs HTTP round-trip | 2.0 ms | 18.7 ms | 20.0 ms |

> ¹ **Why sub-rows don't sum to parent:** Each p99/max is the 99th-worst event from its own per-block
> distribution. The worst total-build block is not always the same block as the worst Payload-Prep
> AND worst Engine-actor-wait simultaneously. Per-event values always sum: `total[i] = attr_prep[i] + build_wait[i]`.
>
> **T1→T2 Heap drain stall:** time Build{attrs} spends waiting inside the BinaryHeap for Consolidate tasks
> that arrived earlier to drain first. Distinct from the mpsc channel send at T1 (non-blocking, near-instant).

### Derivation engine calls (reference)

| Call | p50 | p99 | max | n |
|---|---|---|---|---|
| FCU | 0.9 ms | 47.7 ms | 47.7 ms | 48 |
| new_payload | 62.3 ms | 205.1 ms | 420.8 ms | 135 |

## Engine API — reth EL log timings

> Extracted from reth docker logs (`engine::tree`). Measures reth's own processing time.

| Call | p50 | p99 | max | n |
|---|---|---|---|---|
| FCU (no attrs) | 3.7 ms | 66.5 ms | 70.4 ms | 184 |
| FCU+attrs | 0.1 ms | 7.9 ms | 14.9 ms | 137 |
| new_payload | 8.9 ms | 36.4 ms | 39.9 ms | 136 |

---
*Generated by bench-adventure.sh · 2026-04-08*
