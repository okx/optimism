# EIP-8130 Port Specification (AI-Executable)

**Audience**: An AI coding agent tasked with porting EIP-8130 (Account
Abstraction by Account Configuration) from `base` to this monorepo from
scratch, autonomously.

**Reading guarantee**: An AI that reads only this document plus the linked
files should be able to complete the full port, byte-aligned with base,
without further user input. If you find yourself needing a clarification
that is not in this doc, the doc is wrong — fix it (PR back) before
proceeding.

---

## 0. Mission

Port the complete EIP-8130 implementation from base's `eip-8130-v2` branch
to this monorepo's Rust execution stack (op-revm, op-alloy, op-reth,
alloy-op-evm, alloy-op-hardforks). Target output: byte-compatible state
roots with base when both run the same EIP-8130 transaction.

**Non-goals (explicit)**:

- Re-design EIP-8130. Treat the spec as fixed.
- Improve on base. The user's policy is "**align with base**". Deviations
  must be justified by upstream version drift, by op-reth-only features
  (interop/supervisor), or by file-size constraints — never by personal
  taste.
- Port the Go op-node derivation path. That is an explicitly separate task
  the user defers.
- Port base's bundle/MEV pool features (`Forwarder`, `Consumer`,
  `BuilderApi`, `SendBundleApi`). Op-reth has different MEV infrastructure;
  porting these creates dead code.

---

## 1. References

| Resource | Location | Notes |
|---|---|---|
| Base reference impl | `/Users/xzavieryuan/workspace/reth-projects/base` | Branch `eip-8130-v2`, currently frozen at `a33ab4d` |
| Base alloy 2.0 rebase (parallel, no EIP-8130 yet) | same repo, branch `hh/reth-v2-rebased` | Reference only — when base eventually merges these two, re-sync our port |
| Spec | EIP-8130 (Account Abstraction by Account Configuration) | tx type 0x7B |
| Our monorepo root | `/Users/xzavieryuan/workspace/op-dev/optimism` | Work happens in `rust/` |

---

## 2. Architectural ground rules

### 2.1 Byte-alignment principle

For every file we port from base, we maintain a **normalized diff line
count** as our primary correctness metric. Normalization strips:

- Crate renames: `base_revm`→`op_revm`, `base_alloy_consensus`→
  `op_alloy_consensus`, `base_alloy_evm`→`alloy_op_evm`, `base-revm`→
  `op-revm`, `base-alloy-consensus`→`op-alloy-consensus`
- Hardfork name renames: `BASE_V1`→`NATIVE_AA`, `base_v1`→`native_aa`,
  `BaseV1`→`NativeAA`
- Type renames: `BasePrecompiles`→`OpPrecompiles`, `BasePooledTransaction`→
  `OpPooledTransaction`
- Branding in doc comments: `Base ` (trailing space) → `Optimism `,
  `` `base `` → `` `Optimism ``

After normalization, target divergence per file:

- **0 lines** — file ports without API drift (`constants.rs`,
  `eip8130_policy.rs`, `transaction/abstraction.rs`)
- **<10 lines** — minor crate path differences (`eip8130_pool.rs`,
  `best.rs`, `transaction/eip8130.rs`)
- **10-30 lines** — known structural differences
  (`base_pool.rs` — TransactionPool API drift; `eip8130_invalidation.rs`
   — `lock_slot` path qualification)
- **100+ lines is acceptable** when entirely justified by:
  1. revm 34→38 API adaptations (concentrated in `precompiles.rs`,
     `handler.rs`, `eip8130_validate.rs::verify_custom_via_evm`)
  2. op-only modules (interop, supervisor, conditional)
  3. base-only modules (bundle, forwarder, builder, consumer)

If a file's normalized divergence exceeds these tiers, you have an
unjustified deviation. Find and remove it.

### 2.2 No unjustified divergence rules

- **Don't** add `#[allow(dead_code)]` or `#![allow(...)]`. Use
  `pub(crate)` to plumb visibility instead.
- **Don't** add `let _ = x;` to suppress unused warnings. Either use the
  variable or remove the suppression after verifying it isn't needed.
- **Don't** add `TODO` comments unless tracked in this doc's task list
  AND the value is non-obvious from the code.
- **Don't** add aliases (e.g. `is_eip8130_active_at_timestamp` aliasing
  `is_native_aa_active_at_timestamp`). One name per concept.
- **Don't** rename base's gas-schedule constants (`VerifierGasCosts::
  BASE_V1`). That constant identifies the schedule's version, not a
  fork; renaming breaks future `BASE_V2` rollout alignment.
- **Don't** wrap fields in `Arc<>` "for performance" if base doesn't
  (e.g. `trusted_payer_bytecodes: HashSet<B256>`, not
  `Arc<HashSet<B256>>`).

### 2.3 Hardfork naming

Our chain's EIP-8130 fork is **NativeAA** (PascalCase) /
`NATIVE_AA` (SCREAMING) / `native_aa` (snake_case).

The single trait method that gates EIP-8130 activation is
`is_native_aa_active_at_timestamp`. Don't add an `is_eip8130_active_*`
alias.

`OpSpecId::NATIVE_AA` maps to `SpecId::OSAKA` (CLZ opcode + OSAKA-priced
MODEXP/P256VERIFY are required). Do **not** map to PRAGUE — it diverges
gas accounting from base.

---

## 3. File map (base → ours)

### 3.1 Consensus layer (op-alloy)

| Base | Ours | Notes |
|---|---|---|
| `crates/common/consensus/src/transaction/eip8130/` (whole dir) | `rust/op-alloy/crates/consensus/src/transaction/eip8130/` | 1:1 copy, only `base_alloy_consensus`→`op_alloy_consensus` rename. Tests in `tests.rs` are disabled (upstream fixtures broken). |
| `OpTxEnvelope::BaseV1` variant | `OpTxEnvelope::Eip8130` | Add to `transaction/envelope.rs` |
| `OpTxType::BaseV1` | `OpTxType::Eip8130` (=0x7B) | Add to `transaction/tx_type.rs` |
| `OpTypedTransaction` | Add `Eip8130` arm | `transaction/typed.rs` |
| Receipt envelope `Eip8130` arm | Same | `receipts/envelope.rs`, `receipts/receipt.rs` |

### 3.2 EVM execution (op-revm + alloy-op-evm)

| Base | Ours | Notes |
|---|---|---|
| `crates/execution/revm/src/handler.rs` (3500 LOC monolithic) | `rust/op-revm/src/handler.rs` + `handler_aa_helpers.rs` | **Split into 2 files** because of size. handler_aa_helpers.rs holds AA-specific constants and helpers that base puts at top of handler.rs. Keep nonce-free path in handler.rs verbatim from base lines 921-985. |
| `crates/execution/revm/src/precompiles.rs` | `rust/op-revm/src/precompiles.rs` | Includes both NonceManager (0x...aa02) and TxContext (0x...aa03) precompiles. Keep `EIP8130_TX_TYPE` private here (not in constants.rs). |
| `crates/execution/revm/src/constants.rs` | `rust/op-revm/src/constants.rs` | Byte-aligned. Do NOT add `EIP8130_TX_TYPE` here — it lives in handler_aa_helpers.rs and precompiles.rs separately, mirroring base's pattern. |
| `crates/execution/revm/src/spec.rs` | `rust/op-revm/src/spec.rs` | Add `OpSpecId::NATIVE_AA` (mapped to `SpecId::OSAKA`). KARST/INTEROP variants are X-Layer-specific upstream additions, keep them. Default = JOVIAN. |
| `crates/execution/revm/src/eip8130_policy.rs` | `rust/op-revm/src/eip8130_policy.rs` | Byte-aligned. |
| `crates/execution/revm/src/transaction/eip8130.rs` | `rust/op-revm/src/transaction/eip8130.rs` | Byte-aligned. |
| `crates/execution/revm/src/transaction/abstraction.rs` | `rust/op-revm/src/transaction/abstraction.rs` | Byte-aligned. Note: `OpTransaction::tx_type()` checks deposit FIRST: `if source_hash != B256::ZERO { DEPOSIT_TRANSACTION_TYPE } else { self.base.tx_type() }`. |
| `base_alloy_consensus::build_eip8130_parts_with_costs` | `alloy_op_evm::build_eip8130_parts_with_costs` | Lives in alloy-op-evm in our monorepo (different crate). |

### 3.3 Txpool (op-reth)

| Base | Ours | Notes |
|---|---|---|
| `crates/txpool/src/eip8130_pool.rs` (2148 LOC) | `rust/op-reth/crates/txpool/src/eip8130_pool.rs` | Mechanical copy. Replace `base_alloy_consensus::lock_slot`→`op_alloy_consensus::transaction::eip8130::lock_slot`. `OpTransactionSigned` lives in `reth_optimism_primitives`, not `op_alloy_consensus`. `OpTypedTransaction` lives in `op_alloy_consensus`. |
| `crates/txpool/src/base_pool.rs` (569) | `rust/op-reth/crates/txpool/src/base_pool.rs` | Mechanical copy. Our reth pin requires both `add_transactions(origin, Vec<...>)` AND `add_transactions_with_origins(Vec<(origin, ...)>)` — base only has the latter, with `impl IntoIterator` arg. Implement both. |
| `crates/txpool/src/best.rs` (281) | `rust/op-reth/crates/txpool/src/best.rs` | Mechanical copy. |
| `crates/txpool/src/eip8130_invalidation.rs` | `rust/op-reth/crates/txpool/src/eip8130_invalidation.rs` | Imports `lock_slot, sequence_base_slot` from full path; base uses unqualified path access (their re-export reaches further). Keep test block. |
| `crates/txpool/src/eip8130_validate.rs` | `rust/op-reth/crates/txpool/src/eip8130_validate.rs` | revm 38 adaptations in `verify_custom_via_evm` (see §4). `compute_account_tier` ports straight. |
| `crates/txpool/src/validator.rs` | `rust/op-reth/crates/txpool/src/validator.rs` | Routes ALL AA txs to `Eip8130Pool`. Op-only fields: `supervisor_client`, `fork_tracker` (interop). Don't wrap `trusted_payer_bytecodes` in Arc. |
| `crates/txpool/src/transaction.rs` | `rust/op-reth/crates/txpool/src/transaction.rs` | Add `Eip8130Metadata` struct, `aa_metadata` field on `OpPooledTransaction`, `attach_aa_metadata`/`get_aa_metadata` on `OpPooledTx` trait. Skip base's bundle/MEV fields (`received_at`, `min_timestamp`, etc.) — we don't have that infrastructure. |
| `crates/txpool/src/lib.rs` | `rust/op-reth/crates/txpool/src/lib.rs` | Use `mod xxx; pub use xxx::{...};` pattern matching base. Re-export: `Eip8130InvalidationIndex`, `InvalidationKey`, `compute_invalidation_keys`, `maintain_eip8130_invalidation`, `process_fal`, all of `eip8130_pool::*`, `BaseTransactionPool`, `MergedBestTransactions`, `Eip8130ValidationError`, `Eip8130ValidationOutcome`, `MAX_AA_TX_ENCODED_BYTES`, `VerifierAdmissionPolicy`, `VerifierAllowlist`, `VerifierPurityCache`, `compute_account_tier`, `validate_eip8130_transaction`, `Eip8130Metadata`, `OpPooledTransaction`, `OpPooledTx`. |

### 3.4 Hardforks + EVM env (alloy-op-hardforks + alloy-op-evm)

| Base | Ours | Notes |
|---|---|---|
| `BaseUpgrade::V1` | `OpHardfork::NativeAA` | `rust/alloy-op-hardforks/src/lib.rs` |
| `is_base_v1_active_at_timestamp` | `is_native_aa_active_at_timestamp` | Single name only — no alias |
| `crates/common/evm/src/spec_id.rs` | `rust/alloy-op-evm/src/env.rs` | Add `is_native_aa_active_at_timestamp => NATIVE_AA` arm at TOP of resolution macro (before older forks). |

### 3.5 Payload + Node (op-reth)

| Base | Ours | Notes |
|---|---|---|
| `crates/execution/payload/src/builder.rs` `Eip8130PayloadTransactions` | `rust/op-reth/crates/payload/src/builder.rs` | Wraps `SharedEip8130Pool<T>` + emits `MergedBestTransactions::new(standard, eip8130)` for `OpPayloadTransactions` impl. |
| `crates/execution/node/src/node.rs` pool wire | `rust/op-reth/crates/node/src/node.rs` | Layering: `Pool → BaseTransactionPool → OpPool` (interop filter wraps outermost). Spawn `maintain_eip8130_invalidation` task with `eip8130_pool` from `inner_pool.validator().validator().eip8130_pool()`. |

---

## 4. revm 34→38 API cookbook

Base is on revm 34 / alloy 1.6. We are on revm 38 / alloy 2.0.

When you encounter base code that doesn't compile against our deps, apply
these rules. They're the only differences (modulo upstream module reorg).

### 4.1 Renames

| revm 34 (base) | revm 38 (ours) |
|---|---|
| `PrecompileError` enum | `PrecompileHalt` |
| `PrecompileResult` | `EthPrecompileResult` |
| `Gas::record_cost(n)` | `Gas::record_regular_cost(n)` |
| `context::ContextError::take_error(...)` (function) | `context::take_error(...)` (free function) |

### 4.2 Type / field changes

```rust
// CallInputs gained `reservoir` field (EIP-8037 — set 0 for sub-EVM
// STATICCALLs that don't inherit parent reservoir).
let inputs = CallInputs {
    // ... base fields ...
    reservoir: 0,                         // <-- NEW in revm 38
    known_bytecode: (B256, Bytecode),     // <-- was Option<...> in revm 34
    // ...
};
```

To populate the now-non-Optional `known_bytecode`:

```rust
let known_bytecode = {
    let info = &evm.ctx().journal_mut()
        .load_account_with_code(addr)?  // <-- replaces load_account
        .data
        .info;
    (info.code_hash(), info.code.clone().unwrap_or_default())
};
```

### 4.3 Precompile macro requirement

revm 38 requires `eth_precompile_fn!` wrapping for raw precompile fns:

```rust
// base (revm 34): direct registration
.with_precompile(addr, Precompile::new(g, run_pair_granite));

// ours (revm 38): macro-wrapped
eth_precompile_fn!(granite_precompile, run_pair_granite);
.with_precompile(addr, Precompile::new(g, granite_precompile));
```

### 4.4 Result types

```rust
// base
ResultGas         // doesn't exist in revm 34
ExecutionResult   // same name, different fields

// ours
result::{EVMError, ExecutionResult, FromStringError, ResultGas}  // ResultGas added
```

---

## 5. Hardcoded port snippets

These bits are non-obvious and copy-paste-ready.

### 5.1 `OpHardfork` variant addition

```rust
// alloy-op-hardforks/src/lib.rs — inside hardfork! macro:
        /// Native AA: introduces EIP-8130 (Account Abstraction by Account Configuration).
        ///
        /// Adds transaction type 0x7B with multi-phase calls, dual-domain signing
        /// (sender/payer), 2D nonces, and account-config predeploys. Byte-compatible
        /// with base's BASE_V1 hardfork.
        NativeAA,
```

### 5.2 Spec resolution arm (alloy-op-evm/src/env.rs)

```rust
// MUST be the FIRST arm (before older forks) so it wins when active.
is_native_aa_active_at_timestamp => NATIVE_AA,
```

### 5.3 OpSpecId → SpecId mapping (op-revm/src/spec.rs)

```rust
Self::ISTHMUS | Self::JOVIAN | Self::INTEROP => SpecId::PRAGUE,
Self::KARST | Self::NATIVE_AA => SpecId::OSAKA,
```

### 5.4 Validator AA routing (txpool/src/validator.rs)

```rust
if transaction.ty() == op_alloy_consensus::transaction::eip8130::AA_TX_TYPE_ID {
    if !self.chain_spec().is_native_aa_active_at_timestamp(self.block_timestamp()) {
        return TransactionValidationOutcome::Invalid(
            transaction,
            InvalidTransactionError::TxTypeNotSupported.into(),
        );
    }
    // ... validate via crate::validate_eip8130_transaction ...
    // ... route to self.eip8130_pool.add_transaction ...
    // ... attach_aa_metadata ...
    // Always return Valid { propagate: false, ... }
}
```

### 5.5 Node wire-up (op-reth/crates/node/src/node.rs)

```rust
let inner_pool = TxPoolBuilder::new(ctx)
    .with_validator(validator)
    .build(blob_store, final_pool_config.clone());

let eip8130_pool = inner_pool.validator().validator().eip8130_pool();
let invalidation_index = inner_pool.validator().validator().invalidation_index();

let combined_pool = reth_optimism_txpool::BaseTransactionPool::new(
    inner_pool,
    eip8130_pool.clone(),
);
let transaction_pool = OpPool::new(combined_pool, interop_filter_enabled);

ctx.task_executor().spawn_critical_task(
    "eip8130-maintenance",
    reth_optimism_txpool::maintain_eip8130_invalidation(
        transaction_pool.clone(),
        eip8130_pool,
        tokio_stream::wrappers::BroadcastStream::new(
            ctx.provider().subscribe_to_canonical_state(),
        ),
        invalidation_index,
    ),
);
```

### 5.6 OpTransactionPool type alias (txpool/src/lib.rs)

```rust
pub type OpTransactionPool<Client, S, Evm, T = OpPooledTransaction> = OpPool<
    BaseTransactionPool<
        Pool<
            TransactionValidationTaskExecutor<OpTransactionValidator<Client, T, Evm>>,
            CoinbaseTipOrdering<T>,
            S,
        >,
        T,
    >,
>;
```

### 5.7 Cargo.toml feature propagation

When `reth-optimism-txpool` consumes op-revm/alloy-op-evm via workspace
deps with `default-features = false`, you must explicitly opt into `std`:

```toml
# rust/op-reth/crates/txpool/Cargo.toml
op-revm = { workspace = true, features = ["std"] }
alloy-op-evm = { workspace = true, features = ["native-verifier", "std"] }
```

Without this, op-revm's `to_string()` calls fail because `ToString` isn't
in the `no_std` prelude.

### 5.8 op-reth node Cargo.toml needs tokio-stream

```toml
# rust/op-reth/crates/node/Cargo.toml
tokio-stream.workspace = true
```

---

## 6. Workflow

### 6.1 Verification loop

After every meaningful edit:

```bash
cd /Users/xzavieryuan/workspace/op-dev/optimism/rust && cargo check --workspace 2>&1 | tail -8
```

Single-crate iteration when fast feedback wanted:

```bash
cargo check -p op-revm
cargo check -p reth-optimism-txpool
cargo check -p reth-optimism-node
```

For tests:

```bash
cargo test -p reth-optimism-txpool --lib
```

Target: 92+ tests passing, 0 failures.

### 6.2 Normalized diff comparison

Run this script after a file is "done" to check alignment:

```bash
BASE=/Users/xzavieryuan/workspace/reth-projects/base
OURS=/Users/xzavieryuan/workspace/op-dev/optimism/rust

normalize() {
  sed -E '
    s/base_revm/op_revm/g
    s/base_alloy_consensus/op_alloy_consensus/g
    s/BasePrecompiles/OpPrecompiles/g
    s/BasePooledTransaction/OpPooledTransaction/g
    s/BASE_V1/NATIVE_AA/g
    s/base_v1/native_aa/g
    s/BaseV1/NativeAA/g
    s/Base /Optimism /g
    s/`base/`Optimism/g
    s/base-revm/op-revm/g
    s/base-alloy-consensus/op-alloy-consensus/g
    s/base_alloy_evm/alloy_op_evm/g
  '
}

# Example for one file:
normalize < "$OURS/op-revm/src/constants.rs" > /tmp/o.rs
normalize < "$BASE/crates/execution/revm/src/constants.rs" > /tmp/b.rs
diff -u /tmp/o.rs /tmp/b.rs
echo "Divergence: $(diff /tmp/o.rs /tmp/b.rs | grep -c '^[<>]') lines"
```

For `handler.rs` which is split into 2 files in our port, concatenate:

```bash
cat $OURS/op-revm/src/handler.rs $OURS/op-revm/src/handler_aa_helpers.rs | normalize > /tmp/o.rs
normalize < $BASE/crates/execution/revm/src/handler.rs > /tmp/b.rs
diff -u /tmp/o.rs /tmp/b.rs
```

### 6.3 Suspicious-marker scan

Before declaring done, scan for stale markers:

```bash
grep -rnE "#\[allow\(dead_code|#\[allow\(unused_imports|#!\[allow" \
  rust/op-revm/src rust/op-reth/crates/txpool/src \
  rust/op-alloy/crates/consensus/src/transaction/eip8130 \
  rust/alloy-op-evm/src
# Should output: empty.

grep -rnE "TODO\b|EIP8130_POOL_TODO|EIP8130_METADATA_TODO|FIXME|NOT YET PORTED|stub" \
  rust/op-revm/src rust/op-reth/crates/txpool/src
# Allowed: TODOs that exist in base verbatim (e.g. handler.rs "TODO FrameResult should
#         be a generic trait" mirrors base/handler.rs:657). Anything else = bug.
```

---

## 7. Task breakdown (in order)

Execute these in order. Each is independently testable.

| # | Task | Verification |
|---|---|---|
| 1 | Port `op-alloy/crates/consensus/src/transaction/eip8130/` (whole dir, ~17 files) | `cargo check -p op-alloy-consensus` clean. Run `git diff base ↔ ours` per file: 0 lines divergence except crate rename. |
| 2 | Add `Eip8130` to `OpTxType`, `OpTxEnvelope`, `OpTypedTransaction`. Add receipt envelope arm. | `cargo check -p op-alloy` clean. |
| 3 | Add `OpHardfork::NativeAA` + `is_native_aa_active_at_timestamp` (no alias). | `cargo check -p alloy-op-hardforks` clean. |
| 4 | Add `OpSpecId::NATIVE_AA` → `SpecId::OSAKA` mapping in `op-revm/src/spec.rs`. | `cargo check -p op-revm` clean. |
| 5 | Port `op-revm/src/eip8130_policy.rs` byte-for-byte. | 0 normalized divergence. |
| 6 | Port `op-revm/src/constants.rs` byte-for-byte. NO `EIP8130_TX_TYPE` const. | 0 normalized divergence. |
| 7 | Port `op-revm/src/transaction/eip8130.rs` (Eip8130Parts data carrier). | <5 normalized divergence. |
| 8 | Port `op-revm/src/transaction/abstraction.rs` (OpTransaction wrapper with `eip8130: Eip8130Parts` field). | 0 normalized divergence. |
| 9 | Port `op-revm/src/precompiles.rs` (NonceManager + TxContext predeploys). Apply revm 38 cookbook (§4). | `cargo check -p op-revm`. ~600 lines normalized divergence acceptable. |
| 10 | Port handler logic (split into `handler.rs` + `handler_aa_helpers.rs` due to size). Apply revm 38 cookbook in `validate_against_state_and_deduct_caller`, `execution`, `pre_execution`, `reimburse_caller`. Include nonce-free path in `validate_against_state_and_deduct_caller`. | `cargo test -p op-revm` clean. |
| 11 | Port `alloy-op-evm/src/eip8130_compat.rs` and `build_eip8130_parts_with_costs`. | `cargo check -p alloy-op-evm` clean. |
| 12 | Port `alloy-op-evm/src/tx.rs` Eip8130 dispatch (uses `op_alloy::consensus::transaction::eip8130::AA_TX_TYPE_ID`). | `cargo check -p alloy-op-evm` clean. |
| 13 | Port txpool `eip8130_invalidation.rs` (InvalidationKey, Eip8130InvalidationIndex, compute_invalidation_keys, process_fal, maintain_eip8130_invalidation) + tests. | 13 unit tests pass. |
| 14 | Port txpool `eip8130_validate.rs` (validate_eip8130_transaction, verify_custom_via_evm, compute_account_tier) + tests. Apply revm 38 cookbook in `verify_custom_via_evm`. | 17 unit tests pass. |
| 15 | Port `eip8130_pool.rs`, `base_pool.rs`, `best.rs` (~3000 lines mechanical). | `cargo check -p reth-optimism-txpool` clean. |
| 16 | Wire validator: add `eip8130_pool` field, `with_eip8130_pool_config`/`with_verifier_admission_policy`/`with_verifier_allowlist`/`with_trusted_payer_bytecodes`/`eip8130_pool()`/`invalidation_index()` methods. AA tx branch routes via `eip8130_pool.add_transaction(...)` + `attach_aa_metadata`. | All txpool tests pass. |
| 17 | Add `Eip8130Metadata` struct to `transaction.rs`. Add `aa_metadata` field, `with_aa_metadata`/`aa_metadata` methods. Add `attach_aa_metadata`/`get_aa_metadata` to `OpPooledTx` trait + impl. | Compiles. |
| 18 | Update `lib.rs` re-exports. Type alias `OpTransactionPool` = `OpPool<BaseTransactionPool<Pool<...>, T>>`. | Compiles. |
| 19 | Wire `payload/src/builder.rs`: add `Eip8130PayloadTransactions` impl using `MergedBestTransactions`. | `cargo check -p reth-optimism-payload-builder` clean. |
| 20 | Wire `node/src/node.rs`: build `BaseTransactionPool` over `inner_pool`, spawn `maintain_eip8130_invalidation` task. Add `tokio-stream` to Cargo.toml. | `cargo check -p reth-optimism-node` clean. |
| 21 | Add Eip8130 receipt arm in `op-reth/crates/primitives/src/receipt.rs`, RPC receipt translation, codec, file codec. | Workspace `cargo check` clean. |
| 22 | Final pass: run §6.2 normalized diff on EVERY file. Confirm divergence within tier limits. | All tier limits met. |
| 23 | Final pass: run §6.3 suspicious-marker scan. Empty output. | Clean. |
| 24 | Run `cargo test --workspace`. | All tests pass (≥92 in reth-optimism-txpool). |

---

## 8. Definition of done

The port is complete when ALL of these hold simultaneously:

```bash
# 1. Workspace compiles cleanly.
cargo check --workspace 2>&1 | grep -E "^error|warning: unused" | grep -v "alloy_evm.*unused"
# Expected: empty output (the alloy_evm warning in op-alloy-consensus is pre-existing).

# 2. All unit tests pass.
cargo test -p reth-optimism-txpool --lib 2>&1 | tail -3
# Expected: "test result: ok. 92 passed; 0 failed; 0 ignored"

cargo test -p op-revm --lib 2>&1 | tail -3
# Expected: pre-existing test count, no regression.

# 3. Per-file normalized divergence within §2.1 tiers.

# 4. No suspicious markers (§6.3 scan returns empty).

# 5. cargo test --workspace returns 0.
```

---

## 9. Anti-patterns (things prior attempts got wrong)

These are real mistakes the previous AI made before correction. Don't repeat:

### 9.1 Three copies of `EIP8130_TX_TYPE: u8 = 0x7B`

**Wrong**: defined `pub const EIP8130_TX_TYPE` in `op-revm/src/constants.rs`,
then imported it from `precompiles.rs` and `handler.rs`.

**Right**: base has 2 private copies (in `handler.rs` and `precompiles.rs`)
to avoid cross-file dep cycle. Mirror exactly:

- `op-revm/src/handler_aa_helpers.rs`: `pub(crate) const EIP8130_TX_TYPE: u8 = 0x7B;`
- `op-revm/src/precompiles.rs`: `const EIP8130_TX_TYPE: u8 = 0x7B;` (private)
- Don't add to `constants.rs`. Don't share between the two.
- External callers use `op_alloy_consensus::transaction::eip8130::AA_TX_TYPE_ID`.

### 9.2 Stale "NOT YET PORTED" comments after porting

**Wrong**: kept the file header "NOT YET PORTED: verify_custom_via_evm" after
actually porting it.

**Right**: when you port a deferred function, delete the deferral note.

### 9.3 `XLayerNativeAA` interim naming

**Wrong**: used `XLayerNativeAA` as the fork name "to brand it for X Layer".

**Right**: final name is **`NativeAA`**. Don't add `X Layer` prefix anywhere
(enum variants, snake_case ids, doc comments). The fork is X-Layer-specific
because of where it's enabled in chain config, not because of its name.

### 9.4 Aliased trait methods

**Wrong**: define both `is_native_aa_active_at_timestamp` and
`is_eip8130_active_at_timestamp` (alias). Caller picks one.

**Right**: one name only. Pick the fork name (`is_native_aa_active_at_timestamp`)
to match base's `is_base_v1_active_at_timestamp` style.

### 9.5 File-level `#![allow(...)]`

**Wrong**: `#![allow(dead_code, unused_imports)]` at top of
`eip8130_validate.rs` to suppress warnings during partial port.

**Right**: warnings flag real issues. Either:
- The symbol IS used → fix the visibility/path so the compiler sees it
- The symbol ISN'T used → delete it, or `pub(crate)` it where the user is

### 9.6 `let _ = state;` warning suppression

**Wrong**: drop a parameter into the void to silence "unused variable".

**Right**: either use it, prefix with `_state`, or remove the parameter.

### 9.7 Wrapping fields in `Arc` "for cheap clones"

**Wrong**: `trusted_payer_bytecodes: Arc<HashSet<B256>>` because validator is
`Clone`.

**Right**: match base. `HashSet<B256>` directly. Cloning a small set is fine.

### 9.8 Renaming `VerifierGasCosts::BASE_V1`

**Wrong**: rename to `VerifierGasCosts::NATIVE_AA` for consistency with our
fork name.

**Right**: keep `BASE_V1`. It identifies the gas schedule version (base shipped
v1; future v2 will be `BASE_V2`), not the fork. The schedule travels independently
of the fork name.

### 9.9 Adding TODO comments not in base

**Wrong**: `// TODO(eip-8130): the actual handler integration ... is NOT yet ported`
left in code AFTER porting it.

**Right**: don't add TODOs. If something is genuinely deferred, track it in
this doc's task list (§7) and don't pollute code. Always sweep for stale
TODOs before declaring done.

---

## 10. Self-test for the AI agent

Before submitting work as "done", answer these questions in your reasoning:

1. Did I run `cargo check --workspace` and `cargo test -p reth-optimism-txpool`
   and confirm both pass?
2. Did I run the normalized diff on the files I touched, and is each within
   the tier limits in §2.1?
3. Did I run the suspicious-marker grep and confirm empty output?
4. Did I introduce any of the anti-patterns in §9?
5. For every deviation from base I introduced, can I cite which §4 cookbook
   rule, §3 file-map note, or §5.x snippet justifies it?

If the answer to (4) is yes or to (5) is no for any deviation, the work is
not done.

---

## 11. Coordination with future base updates

Base may at some point merge `eip-8130-v2` with `hh/reth-v2-rebased` (alloy 2.0
upgrade), at which point base's revm 34→38 divergence from ours collapses.

To re-align:

```bash
cd /Users/xzavieryuan/workspace/reth-projects/base
git fetch origin
git log a33ab4d..origin/eip-8130-v2 --oneline   # any new commits?
git log a33ab4d..origin/hh/reth-v2-rebased --oneline -- crates/txpool crates/execution/revm crates/common/consensus
```

When a new merge of these two branches lands on base, re-run §6.2 against the
new commit. The expected effect: per-file divergence should drop because the
revm 38 noise we currently absorb becomes alignment.
