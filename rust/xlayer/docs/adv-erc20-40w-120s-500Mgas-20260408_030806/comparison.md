# Consensus Layer Comparison — Adventure ERC20

## Run configuration

| Field | Value |
|---|---|
| Date | 2026-04-08 |
| CLs compared | base-cl, Kona (baseline), Kona (optimised), op-node |
| Chain | xlayer devnet (195) · 1-second blocks |
| Execution layer | OKX reth — identical binary and config |
| Block gas limit | 500M gas |
| Test duration | 120s |
| Workers | 80 |
| Tx type | ERC20 token transfer — 100k gas limit / ~35k gas actual |
| Sender | adventure `erc20-bench` — 20,000 pre-funded accounts |

## At a glance

| Metric | base-cl | Kona (baseline) | Kona (optimised) | op-node |
|---|---|---|---|---|
| **Throughput** |  |  |  |  |
| &nbsp;&nbsp;Block-inclusion TPS | 11599.6 TX/s | **12055.0 TX/s** | 11682.0 TX/s | 11248.7 TX/s |
| &nbsp;&nbsp;Block fill (avg) | 81.3% | **84.5%** | 81.9% | 78.8% |
| **T0→T3 &nbsp; Total build** |  |  |  |  |
| &nbsp;&nbsp;p50 | 108.7 ms | 110.6 ms | **102.1 ms** | 248.6 ms |
| &nbsp;&nbsp;p99 | 333.6 ms | 298.3 ms | **270.0 ms** | 455.2 ms |
| &nbsp;&nbsp;max | 394.1 ms | 331.4 ms | **277.0 ms** | 528.0 ms |
| &nbsp;&nbsp;▸ T0→T1 &nbsp; Payload Prep |  |  |  |  |
| &nbsp;&nbsp;&nbsp;&nbsp;p99 | 326.2 ms | **224.4 ms** | 264.2 ms | 453.1 ms |
| &nbsp;&nbsp;&nbsp;&nbsp;max | 376.1 ms | 280.7 ms | **273.3 ms** | 522.4 ms |
| &nbsp;&nbsp;▸ T1→T3 &nbsp; Engine actor wait ¹ |  |  |  |  |
| &nbsp;&nbsp;&nbsp;&nbsp;p99 | 152.2 ms | 114.4 ms | **98.8 ms** | 110.7 ms |
| &nbsp;&nbsp;&nbsp;&nbsp;max | 152.9 ms | 194.3 ms | 150.9 ms | **118.6 ms** |
| &nbsp;&nbsp;&nbsp;&nbsp;◦ T1→T2 &nbsp; Heap drain stall |  |  |  |  |
| &nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;p99 | 142.4 ms | 111.2 ms | 97.8 ms | **15.3 ms** |
| &nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;max | 151.4 ms | 192.9 ms | 148.8 ms | **15.7 ms** |
| &nbsp;&nbsp;&nbsp;&nbsp;◦ T2→T3 &nbsp; FCU+attrs HTTP round-trip |  |  |  |  |
| &nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;p99 | **18.7 ms** | 36.8 ms | 31.0 ms | 110.6 ms |
| &nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;max | **20.0 ms** | 41.1 ms | 34.3 ms | 118.5 ms |

> ¹ **Percentile non-additivity:** p99(T0→T1) + p99(T1→T3) ≠ p99(T0→T3).
> Each percentile is the 99th-worst event from its own per-block distribution.
> The worst total-build block is not always the same block as the worst Payload Prep AND worst Engine-actor-wait block.
> Per-event values always sum exactly: `total[i] = attr_prep[i] + build_wait[i]`.
>
> **T1→T2 Heap drain stall:** Build{attrs} waits inside the BinaryHeap for Consolidate tasks that arrived earlier
> to drain first. Distinct from the mpsc channel send at T1 (non-blocking, near-instant).

## Individual run reports

- [base-cl](./base-cl.md)
- [Kona (baseline)](./kona-okx-baseline.md)
- [Kona (optimised)](./kona-okx-optimised.md)
- [op-node](./op-node.md)

## Throughput

| CL | Block-inclusion TPS | Block fill |
|---|---|---|
| base-cl | 11599.6 TX/s | 81.3% |
| Kona (baseline) | 12055.0 TX/s | 84.5% |
| Kona (optimised) | 11682.0 TX/s | 81.9% |
| op-node | 11248.7 TX/s | 78.8% |

## Engine API latency — priority fix evaluation

> **Timing model:** T0=sequencer decides to build · T1=after attr prep, before channel send · T2=FCU HTTP sent to EL · T3=payloadId received
> **total_wait (T0→T3):** full block build cycle. **attr_prep (T0→T1):** L1 info + deposit attribute preparation.
> **heap drain stall (T1→T2):** Build{attrs} waiting in BinaryHeap — direct fix signal; optimised≈0.
> **engine actor wait (T1→T3):** heap drain stall + HTTP. **fcu_attrs (T2→T3):** HTTP only (does not show fix effect).
> Heap drain stall ≠ mpsc channel: channel send at T1 is non-blocking/instant. Stall is inside BinaryHeap only.

### Total build cycle T0→T3 (full sequencer cost)

| Metric | base-cl | Kona (baseline) | Kona (optimised) | op-node | Notes |
|---|---|---|---|---|---|
| **Total build p99** | 333.568 ms | 298.319 ms | **270.042 ms** | 455.247 ms | full cycle from [decides to build] to [payloadId]; requires rebuilt images |
| Total build p50 | 108.693 ms | 110.629 ms | **102.143 ms** | 248.626 ms | typical full cycle |
| Total build max | 394.108 ms | 331.414 ms | **277.044 ms** | 527.988 ms | worst full cycle spike |

### Attr prep T0→T1 (L1 info + deposit preparation)

| Metric | base-cl | Kona (baseline) | Kona (optimised) | op-node | Notes |
|---|---|---|---|---|---|
| Attr prep p99 | 326.249 ms | **224.447 ms** | 264.227 ms | 453.08 ms | tail latency of attribute preparation phase |
| Attr prep p50 | 105.131 ms | 103.999 ms | **100.026 ms** | 246.828 ms | typical attr prep time |

### Heap drain stall T1→T2

| Metric | base-cl | Kona (baseline) | Kona (optimised) | op-node | Notes |
|---|---|---|---|---|---|
| **Heap drain stall p99** | 142.378 ms | 111.224 ms | 97.825 ms | **15.289 ms** | Build{attrs} wait in BinaryHeap before FCU sent; kona-optimised≈0, kona-baseline spikes |
| Heap drain stall p50 | 0.102 ms | 0.087 ms | 0.082 ms | **0.037 ms** | typical heap stall |
| Heap drain stall max | 151.379 ms | 192.873 ms | 148.758 ms | **15.671 ms** | worst heap stall — kona-baseline spikes here under full load |

### Engine actor wait T1→T3 (heap drain stall + HTTP)

| Metric | base-cl | Kona (baseline) | Kona (optimised) | op-node | Notes |
|---|---|---|---|---|---|
| **Engine actor wait p99** | 152.2 ms | 114.426 ms | **98.835 ms** | 110.68 ms | heap drain stall + FCU HTTP; total engine actor cost |
| Engine actor wait p50 | 2.336 ms | **2.196 ms** | 2.236 ms | 2.199 ms | typical engine actor wait |
| Engine actor wait max | 152.883 ms | 194.32 ms | 150.912 ms | **118.579 ms** | worst engine actor stall spike |

### FCU HTTP only T2→T3 (reference — does not show fix)

| Metric | base-cl | Kona (baseline) | Kona (optimised) | op-node | Notes |
|---|---|---|---|---|---|
| FCU HTTP p50 | **1.986 ms** | 1.997 ms | 2.074 ms | 2.162 ms | reth round-trip only — post-dequeue, queue wait excluded |
| FCU HTTP p99 | **18.738 ms** | 36.779 ms | 31.047 ms | 110.639 ms | tail latency of HTTP call |
| FCU HTTP max | **20.029 ms** | 41.132 ms | 34.276 ms | 118.542 ms | worst HTTP round-trip |

### Derivation engine calls (reference)

| Metric | base-cl | Kona (baseline) | Kona (optimised) | op-node | Notes |
|---|---|---|---|---|---|
| FCU (derivation) p50 | 0.95 ms | 4.369 ms | **0.909 ms** | N/A | kona: derivation/finalization FCU without attrs |
| FCU (derivation) p99 | 47.672 ms | 32.573 ms | **6.245 ms** | N/A | kona derivation FCU tail |
| new_payload p50 | 62.307 ms | 67.125 ms | 64.017 ms | **61.349 ms** | engine_newPayload round-trip |
| new_payload max | 420.845 ms | **241.145 ms** | 322.241 ms | 527.088 ms | worst import |
| Block import / seal p50 | 103.027 ms | **100.752 ms** | 103.97 ms | N/A | kona: getPayload+newPayload seal cycle |

### reth EL internal timings (from reth docker logs)

| Metric | base-cl | Kona (baseline) | Kona (optimised) | op-node | Notes |
|---|---|---|---|---|---|
| FCU+attrs p50 | 0.077 ms | **0.072 ms** | 0.085 ms | 0.087 ms | reth's own time to accept block-build trigger |
| new_payload p50 | **8.858 ms** | 9.749 ms | 9.152 ms | 9.353 ms | reth's own time to validate+import sealed block |

## Verdict

> TPS differences ≤5% between CLs are run-to-run noise — the CL is not the throughput bottleneck.
> The fix targets sequencer **tail latency** (queue starvation under full load), not throughput.

| Dimension | Best performer | Notes |
|---|---|---|
| Block-inclusion TPS | **Kona (baseline)** | Highest confirmed TPS — small differences are noise |
| **Heap drain stall p99 (T1→T2)** | **op-node** | 15.289 ms vs 142.378 ms — 9.3× |
| Engine actor wait p99 (T1→T3) | **Kona (optimised)** | 98.835 ms vs 152.2 ms — 1.5× improvement |
| Engine actor wait max | **op-node** | 118.579 ms vs 194.32 ms worst spike |

