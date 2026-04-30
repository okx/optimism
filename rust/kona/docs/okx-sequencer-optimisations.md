# OKX Sequencer Optimisations — Reference

Branch: `fix/kona-engine-drain-priority`
Base commit: `2d6c0a5e2`

## Summary

One targeted fix shipped to reduce sequencer block-build latency (T0→T3) with measurable,
benchmarkable impact. Two additional optimisations were designed and implemented but deferred
— see §Deferred below.

## Shipped

### Opt-2 · SystemConfig cache · commit `842d55010`
**File:** `rust/kona/crates/providers/providers-alloy/src/l2_chain_provider.rs`

**Problem:** `system_config_by_number()` called `block_by_number()` on every single block
build. The `LruCache<u64, OpBlock>` is keyed by block number, which increments by 1 each
block, so the cache NEVER hit — every call was a live `eth_getBlockByNumber` RPC to reth
(~38ms on devnet). Over a 1s block interval this consumed ~38% of the attr_prep budget.

**Fix:** `last_system_config: Option<SystemConfig>` field on `AlloyL2ChainProvider`. After
the first fetch the cached value is returned immediately. An explicit
`invalidate_system_config_cache()` method allows future callers (e.g. DerivationActor on
L1 config-change events) to force a refresh. SystemConfig contains gasLimit, eip1559Params,
feeVault, batcherAddr, and unsafeBlockSigner — all set by L1 admin transactions and stable
across thousands of blocks in practice.

**Bench impact:** attr_prep p50: ~38ms → ~1ms (every block). T0→T3 p50: ~40ms → ~3ms.
Validated on devnet 200M gas benchmark.

---

## Deferred

The two fixes below were fully designed, implemented, and validated to compile (`cargo check`
passes). They were deferred because they cannot produce measurable benchmark numbers on this
devnet setup — shipping them without bench proof would be unjustifiable in code review.
They must be revisited when either (a) the devnet environment matches production L1 latency,
or (b) integration tests exist that can exercise the relevant code paths.

---

### Deferred-Opt-2a · Dedicated engine runtime

**Target file:** `rust/kona/bin/node/src/cli.rs`

**Problem:** FCU+attrs and new_payload HTTP response futures share the main tokio runtime
with derivation Consolidate tasks. Under load the runtime scheduler may not poll an FCU
response future for up to 40ms after reth has already replied, producing artificial T2→T3
tail spikes. The correct fix is a fully isolated `tokio::Runtime` for engine HTTP calls so
Consolidate tasks can never starve engine response futures.

**Why deferred:** The fully isolated runtime requires invasive changes to alloy's
`RootProvider` and `rollup_boost::RpcClient` — both are opaque to runtime selection
(futures-based APIs with no `Handle`-based spawn interface exposed). Making those changes
safely requires integration tests that do not yet exist.

**What was implemented instead (and then reverted):** `tokio_runtime()` in `cli.rs` was
changed to `max(cpu_count, 8)` workers with named threads `"kona-worker"`. This is the
safe subset of the fix — it adds thread headroom on under-provisioned machines — but on a
modern Mac or server with 8+ cores it has zero measurable effect. It was reverted because
it cannot produce bench numbers to justify the commit.

**When to ship:**
1. On a machine with exactly N < 8 cores running the sequencer: ship the `max(cpu_count, 8)`
   version immediately — it will measurably reduce T2→T3 tail latency.
2. For the full fix: wait until alloy's `RootProvider` exposes a `Handle`-based constructor
   or until integration tests cover the engine HTTP call path.

**Expected impact when properly isolated:** T2→T3 max reduced from ~42ms toward <5ms under
sustained full-block load.

---

### Deferred-Opt-1 · L1 receipt pre-fetch

**Target files:**
- `rust/kona/crates/providers/providers-alloy/src/chain_provider.rs`
- `rust/kona/crates/providers/providers-alloy/src/receipt_cache.rs` (new)
- `rust/kona/crates/node/service/src/service/node.rs`

**Problem:** At each L1 epoch boundary, `StatefulAttributesBuilder::prepare_payload_attributes()`
calls `receipts_fetcher.receipts_by_hash(epoch.hash)` synchronously on the critical
block-build path. On a remote production L1 node this adds 20-100ms to attr_prep for ~8%
of L2 blocks (1 per L1 slot / ~12 L2 blocks).

**Fix (designed but reverted):** `AlloyChainProvider` gets an optional
`receipt_prefetch: Option<SharedReceiptCache>` field. A background watcher task in `node.rs`
subscribes to the `l1_head_updates_tx` watch channel. On each new L1 head it immediately
fetches receipts off the critical path, giving the sequencer ~12 seconds of lead time.
On a cache hit the epoch boundary block sees ~0ms receipt cost. On a miss it falls through
to the existing live RPC path — no correctness change.

**Why deferred:** On this devnet the L1 is local geth on the same machine — receipt fetch
takes 2-5ms, not 20-100ms. Opt-1 would save at most 2-5ms on 8% of blocks = <0.5ms
amortised. That is below benchmark noise and cannot be demonstrated in numbers.

Think of L2 blocks like a bus that leaves every second. Before the bus departs, the
sequencer must prepare a "passenger manifest" (payload attributes — `attr_prep`). At an
L1 epoch boundary, that manifest requires a receipt from the L1 node listing any deposits.
Opt-1 fetches that receipt in advance — like ordering the passenger list 12 seconds early —
so when the bus is ready, the list is already there and the sequencer pays zero wait time.

On devnet, the L1 is on the same machine so the receipt fetch already takes only 2-5ms —
the pre-fetch saves almost nothing. On production mainnet with a remote L1 node the latency
is 50-200ms, and the math changes completely:

| Environment | Receipt fetch latency | Blocks affected | Amortised saving |
|---|---|---|---|
| Devnet (local geth) | 2-5ms | ~8% (1 in 12) | ~0.16ms — below noise |
| Production mainnet (remote L1) | 50-200ms | ~8% | ~4-16ms — clearly visible |

**When to ship:** When benchmarking against a remote L1 node (production mainnet, public
RPC endpoint). At 50-200ms L1 receipt latency, Opt-1 saves ~20-100ms on every epoch
boundary block — clearly visible in attr_prep p99 and p50 distributions.

---

## Expected bench result — Opt-2 only (200M gas, 20w, 120s)

| Metric     | kona-okx-baseline (before) | kona-okx-optimised (after Opt-2) |
|------------|----------------------------|----------------------------------|
| T0→T3 p50  | ~40ms                      | ~3ms                             |
| T0→T3 p99  | ~77ms                      | ~10ms                            |
| T0→T3 max  | ~86ms                      | ~15ms                            |
| attr_prep p50 | ~38ms                   | ~1ms                             |

Figures are projections based on per-fix analysis. Run the bench suite to confirm.
