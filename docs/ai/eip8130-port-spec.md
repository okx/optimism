# EIP-8130 Port Specification (AI-Executable)

**Audience**: an AI coding agent tasked with porting EIP-8130 (Account
Abstraction by Account Configuration) from `base` to this monorepo from
scratch.

**Goal**: byte-compatible state roots with base on the same EIP-8130
transaction.

---

## 1. References

| Resource | Path | Status |
|---|---|---|
| Base reference impl | `/Users/xzavieryuan/workspace/reth-projects/base` | Branch `eip-8130-v2`, frozen at `a33ab4d` |
| Base alloy-2.0 rebase (no EIP-8130 yet) | same repo, branch `hh/reth-v2-rebased` | Reference only |
| Our root | `/Users/xzavieryuan/workspace/op-dev/optimism` | Work in `rust/` |

---

## 2. Five binding principles

Every concrete rule below derives from one of these. When a situation
isn't covered by a specific section, fall back here.

### P1. Code state matches reality

Comments, suppressions, and markers describe what the code IS, not what
it was during some earlier phase. Stale = lie. After porting a deferred
function, the deferral note goes. After fixing a warning's cause, the
suppression goes.

### P2. One name per concept

A single semantic identity gets a single identifier across the codebase.
No aliases, no decorative prefixes/suffixes that encode no new constraint.

### P3. Mirror base's structure

If base has 2 private copies of a constant to dodge a cyclic dep, mirror
that. Don't consolidate, don't refactor, don't add abstractions base
lacks. The byte-alignment metric (§3) is your judge.

### P4. Compiler feedback is signal

Warnings flag real problems. The fix is the cause (visibility, dead code,
actual usage), never the suppression. No `#[allow(...)]`, no `let _ = x`,
no `_unused` prefixes used as escape hatches.

### P5. Understand a name before changing it

Two similar names may identify different things. Trace the domain (fork
name? gas-schedule version? trait method? contract version?) before
renaming. Rename only within one domain.

These principles compose. Most anti-patterns violate two or more.

---

## 3. Byte-alignment metric

For each ported file, normalize trivial differences with sed and count
remaining divergent lines:

```bash
normalize() { sed -E '
  s/base_revm/op_revm/g
  s/base_alloy_consensus/op_alloy_consensus/g
  s/base_alloy_evm/alloy_op_evm/g
  s/BasePrecompiles/OpPrecompiles/g
  s/BasePooledTransaction/OpPooledTransaction/g
  s/BASE_V1/NATIVE_AA/g
  s/base_v1/native_aa/g
  s/BaseV1/NativeAA/g
  s/Base /Optimism /g
  s/`base/`Optimism/g
  s/base-revm/op-revm/g
  s/base-alloy-consensus/op-alloy-consensus/g
'; }

normalize < $OURS/path > /tmp/o.rs
normalize < $BASE/path > /tmp/b.rs
diff /tmp/o.rs /tmp/b.rs | grep -c '^[<>]'   # divergent lines
```

Tier limits per file (see §5 file map for which tier each file targets):

| Tier | Target | Allowed cause |
|---|---|---|
| 0 | 0 lines | Pure data carrier, no API drift |
| A | <10 | Trivial path qualification |
| B | 10-30 | Known structural difference (TransactionPool API drift, file split) |
| C | 100+ | revm 34→38 API adaptation, op-only modules, base-only modules |

Anything outside these tiers is an unjustified deviation. Find and remove.

---

## 4. revm 34→38 / alloy 1.6→2.0 cookbook

Base is on revm 34 / alloy 1.6. We are on revm 38 / alloy 2.0. Apply
these mechanical adaptations whenever base code doesn't compile against
our deps.

### 4.1 Renames

| revm 34 (base) | revm 38 (ours) |
|---|---|
| `PrecompileError` enum | `PrecompileHalt` |
| `PrecompileResult` | `EthPrecompileResult` |
| `Gas::record_cost(n)` | `Gas::record_regular_cost(n)` |
| `ContextError::take_error(...)` | `context::take_error(...)` (free fn) |

### 4.2 `CallInputs` field changes

```rust
// revm 38 added `reservoir` (EIP-8037; 0 for sub-EVM STATICCALLs that
// don't inherit a parent reservoir) and made `known_bytecode` non-Optional.
let known_bytecode = {
    let info = &evm.ctx().journal_mut()
        .load_account_with_code(addr)?      // replaces load_account
        .data
        .info;
    (info.code_hash(), info.code.clone().unwrap_or_default())
};
let inputs = CallInputs {
    // ... base fields ...
    reservoir: 0,                            // NEW
    known_bytecode,                          // was Option<...>
    // ...
};
```

### 4.3 Precompile macro

revm 38 requires `eth_precompile_fn!` to wrap raw precompile fns before
registration:

```rust
eth_precompile_fn!(granite_precompile, run_pair_granite);
.with_precompile(addr, Precompile::new(g, granite_precompile));
```

### 4.4 `OpSpecId::NATIVE_AA` maps to `SpecId::OSAKA`

CLZ + OSAKA-priced MODEXP/P256VERIFY are required. Mapping to PRAGUE
diverges gas accounting — don't.

---

## 5. File map (base → ours)

### 5.1 Consensus (op-alloy)

| Base | Ours | Tier |
|---|---|---|
| `crates/common/consensus/src/transaction/eip8130/` | `rust/op-alloy/crates/consensus/src/transaction/eip8130/` | 0 |
| `OpTxEnvelope::BaseV1` | `OpTxEnvelope::Eip8130` | — |
| `OpTxType::BaseV1` (=0x7B) | `OpTxType::Eip8130` | — |

### 5.2 EVM (op-revm + alloy-op-evm)

| Base | Ours | Tier |
|---|---|---|
| `crates/execution/revm/src/handler.rs` (3500 LOC) | `rust/op-revm/src/handler.rs` + `handler_aa_helpers.rs` (size split) | C |
| `crates/execution/revm/src/precompiles.rs` | `rust/op-revm/src/precompiles.rs` | C |
| `crates/execution/revm/src/constants.rs` | `rust/op-revm/src/constants.rs` (no `EIP8130_TX_TYPE` here — see §6.1) | 0 |
| `crates/execution/revm/src/spec.rs` | `rust/op-revm/src/spec.rs` | C |
| `crates/execution/revm/src/eip8130_policy.rs` | `rust/op-revm/src/eip8130_policy.rs` | 0 |
| `crates/execution/revm/src/transaction/eip8130.rs` | `rust/op-revm/src/transaction/eip8130.rs` | A |
| `crates/execution/revm/src/transaction/abstraction.rs` | `rust/op-revm/src/transaction/abstraction.rs` | 0 |
| `base_alloy_consensus::build_eip8130_parts_with_costs` | `alloy_op_evm::build_eip8130_parts_with_costs` | — |

### 5.3 Txpool (op-reth)

| Base | Ours | Tier |
|---|---|---|
| `crates/txpool/src/eip8130_pool.rs` (2148 LOC) | `rust/op-reth/crates/txpool/src/eip8130_pool.rs` | A |
| `crates/txpool/src/base_pool.rs` (569) | `rust/op-reth/crates/txpool/src/base_pool.rs` | B |
| `crates/txpool/src/best.rs` (281) | `rust/op-reth/crates/txpool/src/best.rs` | A |
| `crates/txpool/src/eip8130_invalidation.rs` | `rust/op-reth/crates/txpool/src/eip8130_invalidation.rs` | B |
| `crates/txpool/src/eip8130_validate.rs` | `rust/op-reth/crates/txpool/src/eip8130_validate.rs` | C |
| `crates/txpool/src/validator.rs` | `rust/op-reth/crates/txpool/src/validator.rs` | C |
| `crates/txpool/src/transaction.rs` | `rust/op-reth/crates/txpool/src/transaction.rs` | C |
| `crates/txpool/src/lib.rs` | `rust/op-reth/crates/txpool/src/lib.rs` | C |

### 5.4 Hardforks + Payload + Node

| Base | Ours |
|---|---|
| `BaseUpgrade::V1` | `OpHardfork::NativeAA` (no `XLayer` prefix; no `is_eip8130_*` alias) |
| `is_base_v1_active_at_timestamp` | `is_native_aa_active_at_timestamp` |
| `crates/common/evm/src/spec_id.rs` | `rust/alloy-op-evm/src/env.rs` (NATIVE_AA arm goes FIRST in resolution macro) |
| `crates/execution/payload/src/builder.rs` `Eip8130PayloadTransactions` | `rust/op-reth/crates/payload/src/builder.rs` |
| `crates/execution/node/src/node.rs` pool wire | `rust/op-reth/crates/node/src/node.rs` (layering: `Pool → BaseTransactionPool → OpPool`) |

---

## 6. Non-obvious port points

These are the spots where mechanical translation isn't enough.

### 6.1 `EIP8130_TX_TYPE` constant placement

Base has 2 private copies (`handler.rs:43` and `precompiles.rs:208`) to
avoid a cyclic intra-crate dep. Mirror exactly:

- `op-revm/src/handler_aa_helpers.rs`: `pub(crate) const EIP8130_TX_TYPE: u8 = 0x7B;`
- `op-revm/src/precompiles.rs`: `const EIP8130_TX_TYPE: u8 = 0x7B;` (private)

External callers use `op_alloy_consensus::transaction::eip8130::AA_TX_TYPE_ID`
(canonical).

### 6.2 Validator AA routing

```rust
if transaction.ty() == op_alloy_consensus::transaction::eip8130::AA_TX_TYPE_ID {
    if !self.chain_spec().is_native_aa_active_at_timestamp(self.block_timestamp()) {
        return Invalid(transaction, TxTypeNotSupported.into());
    }
    // validate via crate::validate_eip8130_transaction(...)
    // route to self.eip8130_pool.add_transaction(...)
    // attach_aa_metadata
    // return Valid { propagate: false, ... }
}
```

### 6.3 Node pool layering

```rust
let inner_pool = TxPoolBuilder::new(ctx).with_validator(validator).build(...);
let eip8130_pool = inner_pool.validator().validator().eip8130_pool();
let invalidation_index = inner_pool.validator().validator().invalidation_index();
let combined = BaseTransactionPool::new(inner_pool, eip8130_pool.clone());
let transaction_pool = OpPool::new(combined, interop_filter_enabled);

ctx.task_executor().spawn_critical_task(
    "eip8130-maintenance",
    maintain_eip8130_invalidation(
        transaction_pool.clone(),
        eip8130_pool,
        BroadcastStream::new(ctx.provider().subscribe_to_canonical_state()),
        invalidation_index,
    ),
);
```

### 6.4 Type alias

```rust
pub type OpTransactionPool<Client, S, Evm, T = OpPooledTransaction> = OpPool<
    BaseTransactionPool<
        Pool<TransactionValidationTaskExecutor<OpTransactionValidator<Client, T, Evm>>,
             CoinbaseTipOrdering<T>, S>,
        T,
    >,
>;
```

### 6.5 Cargo.toml feature propagation

When `reth-optimism-txpool` consumes `op-revm`/`alloy-op-evm` via workspace
deps with `default-features = false`, opt into `std` explicitly — otherwise
`ToString` isn't in the `no_std` prelude and `to_string()` calls fail:

```toml
op-revm = { workspace = true, features = ["std"] }
alloy-op-evm = { workspace = true, features = ["native-verifier", "std"] }
```

`reth-optimism-node` needs `tokio-stream.workspace = true` for the
`BroadcastStream` used in the maintenance task.

---

## 7. Workflow

### 7.1 Compile loop

```bash
cargo check --workspace 2>&1 | tail -8
cargo check -p <crate>           # single-crate iteration
```

### 7.2 Diff loop

After a file is "done", run §3's normalize+diff and verify the count
matches the file's tier (§5).

### 7.3 Suspicious-marker scan

Before declaring any file done:

```bash
grep -rnE "#\[allow\(dead_code|#\[allow\(unused_imports|#!\[allow" \
  rust/op-revm/src rust/op-reth/crates/txpool/src \
  rust/op-alloy/crates/consensus/src/transaction/eip8130 rust/alloy-op-evm/src
# Expected: empty.

grep -rnE "TODO\b|EIP8130_POOL_TODO|EIP8130_METADATA_TODO|FIXME|NOT YET PORTED|stub\b" \
  rust/op-revm/src rust/op-reth/crates/txpool/src
# Expected: only TODOs that exist in base verbatim. Anything else = P1 violation.
```

---

## 8. Task breakdown

Execute in order. Each step is independently verifiable.

| # | Task | Verification |
|---|---|---|
| 1 | Port `op-alloy/.../transaction/eip8130/` directory | tier 0 per file |
| 2 | Add `Eip8130` arm to `OpTxType`, `OpTxEnvelope`, `OpTypedTransaction`, receipt envelope | `cargo check -p op-alloy` |
| 3 | Add `OpHardfork::NativeAA` + `is_native_aa_active_at_timestamp` (single name) | `cargo check -p alloy-op-hardforks` |
| 4 | Add `OpSpecId::NATIVE_AA → SpecId::OSAKA` | `cargo check -p op-revm` |
| 5 | Port `eip8130_policy.rs` | tier 0 |
| 6 | Port `constants.rs` (no `EIP8130_TX_TYPE`) | tier 0 |
| 7 | Port `transaction/eip8130.rs` (Eip8130Parts) | tier A |
| 8 | Port `transaction/abstraction.rs` | tier 0 |
| 9 | Port `precompiles.rs` (apply §4 cookbook) | compile clean |
| 10 | Port handler logic — split into `handler.rs` + `handler_aa_helpers.rs`. Apply §4. Include nonce-free path. `EIP8130_TX_TYPE` per §6.1. | compile clean |
| 11 | Port `alloy-op-evm/src/eip8130_compat.rs` + `build_eip8130_parts_with_costs` | `cargo check -p alloy-op-evm` |
| 12 | Port `alloy-op-evm/src/tx.rs` Eip8130 dispatch | compile clean |
| 13 | Port `eip8130_invalidation.rs` + tests | 13 tests pass |
| 14 | Port `eip8130_validate.rs` (incl. `verify_custom_via_evm`, `compute_account_tier`) + tests | 17 tests pass |
| 15 | Port `eip8130_pool.rs`, `base_pool.rs`, `best.rs` (~3000 lines mechanical) | compile clean |
| 16 | Wire validator (§6.2) | tests pass |
| 17 | Add `Eip8130Metadata` + `attach_aa_metadata`/`get_aa_metadata` on `OpPooledTx` | compile clean |
| 18 | Update `lib.rs` re-exports + type alias (§6.4) | compile clean |
| 19 | Wire `payload/builder.rs` `Eip8130PayloadTransactions` | compile clean |
| 20 | Wire `node/node.rs` (§6.3) | compile clean |
| 21 | Add Eip8130 receipt arm in primitives, RPC, codec | workspace clean |
| 22 | Run §3 diff per file. Confirm tier limits met. | all tiers met |
| 23 | Run §7.3 marker scan. Empty. | clean |
| 24 | `cargo test --workspace` | ≥92 tests, 0 failures |

---

## 9. Definition of done

All five must hold:

```bash
# 1. Workspace compiles cleanly (only the pre-existing alloy_evm warning).
cargo check --workspace 2>&1 | grep -E "^error" | head
# Expected: empty.

# 2. Tests pass.
cargo test -p reth-optimism-txpool --lib 2>&1 | tail -3
# Expected: "test result: ok. 92 passed; 0 failed"

# 3. Per-file diff within tier limits (§3, §5).

# 4. Marker scan empty (§7.3).

# 5. cargo test --workspace returns 0.
```

Before submission, the AI must answer:

1. Did I run all five checks above? Did they pass?
2. For every divergence I introduced, can I cite which §4 cookbook rule,
   §5 file-map note, or §6 non-obvious port point justifies it?
3. Did I introduce any P1–P5 violations?

If (3) is yes, or (2) is no for any divergence, the work is not done.

---

## 10. Future re-alignment

Base may eventually merge `eip-8130-v2` with `hh/reth-v2-rebased` (alloy
2.0 upgrade), at which point base's revm 34→38 divergence from us
collapses.

To detect:

```bash
cd /Users/xzavieryuan/workspace/reth-projects/base
git fetch origin
git log a33ab4d..origin/eip-8130-v2 --oneline   # any new commits?
git log a33ab4d..origin/hh/reth-v2-rebased --oneline -- \
    crates/txpool crates/execution/revm crates/common/consensus
```

When new merge lands, re-run §3 against the new commit. The expected
effect: per-file divergence drops as the revm 38 noise we currently
absorb becomes alignment.
