# Phase B Cross-Reference: alloy-op-evm + alloy-op-hardforks + rpc-types

Scope: symbol-level diff of EIP-8130-related code paths across three crates
between OURS (`https://github.com/okx/optimism/tree/cf38ca5666/rust/`) and
BASE (`https://github.com/base/base/tree/a33ab4d09/crates/`).

Excluded by directive: BUG-001..008 (especially BUG-008, the
`make_tx_context_precompile` / `make_nonce_manager_precompile` /
`op_precompiles_map` wiring miss in `alloy-op-evm/src/lib.rs:249-275`).

## Summary

The three crates are largely 1:1 ports with the EIP-8130 surface area
intact:

- `OpEvm` / `OpEvmFactory` types, `Evm` trait impl, `BlockExecutor` impl,
  `OpReceiptBuilder`, `ensure_aa_predeploys`, `Eip8130ReceiptFields`,
  `OpTransactionRequest`'s AA fields and `build_eip8130()` are all
  byte-equivalent in fields and signatures.
- `is_native_aa_active_at_timestamp` (ours) ≡ `is_base_v1_active_at_timestamp`
  (base) — same predicate semantics (delegated to `op_fork_activation`
  / `BaseUpgrades`) just renamed.
- `AA_PRECOMPILE_ADDRESSES`, `AA_STUB_BYTECODE`, sentinel-check logic in
  `ensure_aa_predeploys` are byte-identical.
- `make_tx_context_precompile` / `make_nonce_manager_precompile` bodies
  in `alloy-op-evm/src/lib.rs` (ours) ↔ `factory.rs` (base) are
  byte-equivalent (BUG-008 covers the "they exist but aren't wired into
  the EvmFactory" miss — out of scope here).

Structural deltas (intentional, not bugs):

1. Our `alloy-op-evm` keeps the upstream split: `block/{mod,canyon,native_aa,
   receipt_builder}.rs` + `lib.rs` + `tx.rs` + `eip8130_compat.rs` + `env.rs`
   + `error.rs`. Base has flat `evm.rs / factory.rs / executor.rs /
   executor_factory.rs / receipt_builder.rs / canyon.rs / base_v1.rs / tx_env.rs
   / spec_id.rs / ctx.rs / error.rs`. The pre-execution AA stub deploy is
   wired correctly in both: ours in `block/mod.rs:225` (apply_pre_execution_changes),
   base in `executor.rs:168`.
2. Our `alloy-op-hardforks` does NOT contain the BaseV1/Jovian/etc.
   `Hardfork` deposit-tx generators that base puts in
   `crates/consensus/upgrades`. Our equivalent lives in
   `op-node/rollup/derive/native_aa_upgrade_transactions.go` (verified —
   bytecode hex files mirror base 1:1 under `op-node/rollup/derive/native_aa_bytecode/`).
   This is intentional separation of concerns (Go op-node owns deposits)
   and not a port bug.
3. Our `op-alloy/crates/rpc-types` adds `op_gas_refund: Option<u64>` and
   `PostExec` envelope handling not present in base. Both are pre-existing
   OKX features, not EIP-8130 concerns.

## Per-File Cross-Reference

### alloy-op-evm

| OURS file | BASE file | Status |
|---|---|---|
| `lib.rs` (`OpEvm`, `OpEvmFactory`) | `evm.rs` + `factory.rs` | Equivalent for non-AA path; AA precompile wiring miss = BUG-008 (out of scope) |
| `block/mod.rs` (`OpBlockExecutor`, `OpBlockExecutorFactory`, `OpTxEnv`) | `executor.rs` + `executor_factory.rs` | Equivalent. `apply_pre_execution_changes` calls `ensure_aa_predeploys` correctly. |
| `block/native_aa.rs` (`ensure_aa_predeploys`) | `base_v1.rs` (`ensure_aa_predeploys`) | **Byte-identical** body. Only differences: (a) trait bound `OpHardforks` vs `BaseUpgrades`, (b) gating predicate name `is_native_aa_active_at_timestamp` vs `is_base_v1_active_at_timestamp`, (c) doc comment references CREATE2 vs deposit tx. Sentinel check, stub bytecode, address list all match. |
| `block/canyon.rs` (`ensure_create2_deployer`) | `canyon.rs` | Byte-identical bytecode + logic. |
| `block/receipt_builder.rs` (`OpAlloyReceiptBuilder`) | `receipt_builder.rs` | Equivalent. Ours adds `OpTxType::PostExec` arm (pre-existing feature). |
| `eip8130_compat.rs` (`build_eip8130_parts`, `build_eip8130_parts_with_costs`, `derive_sender_owner_id`, `derive_payer_owner_id`, `build_verify_call`, `build_authorizer_validations`) | (lives in `base/crates/common/consensus/src/evm_compat.rs`) | Per file header comment, ported 1:1 from base lines 1-500. Out of immediate scope (consensus crate Phase B agent). |
| `env.rs` (`spec`, `spec_by_timestamp_after_bedrock`, `evm_env_for_op_block/next_block/payload`) | `spec_id.rs` | Equivalent. Ours uses macro-driven match; base uses if/else chain. Ours has additional `Karst`, `Interop`, `NativeAA` arms that map BASE's `BASE_V1` → `NATIVE_AA`. |
| `error.rs` (`OpTxError`, `map_op_err`) | `error.rs` | Different shapes. Base's `BaseBlockExecutionError` covers DA-footprint cases inside `executor.rs`; ours puts the same DA-footprint enum in `block/mod.rs::OpBlockExecutionError`. |
| `tx.rs` (`OpTx` newtype + `From*Tx` impls) | `tx_env.rs` (just trait `OpTxEnv`) | Ours has the full `OpTxEnvelope::Eip8130` dispatch (lines 169-196). Base does the equivalent dispatch in its consensus crate via `FromRecoveredTx` blanket impls — out of scope here. |

### alloy-op-hardforks

| OURS file | BASE file | Status |
|---|---|---|
| `lib.rs` (`OpHardfork` enum, `OpHardforks` trait, `OpChainHardforks`, `is_native_aa_active_at_timestamp`) | `base/crates/consensus/upgrades/src/lib.rs` (different shape: `BaseUpgrades` trait elsewhere) | Both add an "AA" hardfork variant to their respective enum. Activation predicates are equivalent. |
| `optimism/{mainnet,sepolia}.rs` + `base/{mainnet,sepolia}.rs` | n/a (lives in `base-alloy-chains`) | Out of port scope. |
| n/a (lives in op-node Go) | `base_v1.rs`, `jovian.rs`, `fjord.rs`, `ecotone.rs`, `isthmus.rs`, `forks.rs`, `traits.rs`, `utils.rs` (deposit tx generators + `Hardfork` trait) | **Structural** — base puts upgrade-deposit generation in Rust; OURS puts it in Go. Bytecode hex files exist in both repos. |

### rpc-types

| OURS file | BASE file | Status |
|---|---|---|
| `rpc-types/src/receipt.rs` (`OpTransactionReceipt`, `Eip8130ReceiptFields`, `OpTransactionReceiptFields`, `L1BlockInfo`, `From<OpTransactionReceipt> for OpReceiptEnvelope`) | `rpc-types/src/receipt.rs` | AA fields **byte-identical** (struct `Eip8130ReceiptFields { payer, phase_statuses }` with same serde attrs). Ours adds `op_gas_refund` field on receipt + `OpTransactionReceiptFields` and a `PostExec` arm in the `From` impl (pre-existing OKX feature). |
| `rpc-types/src/transaction/request.rs` (`OpTransactionRequest`, AA fields, `build_eip8130`) | `rpc-types/src/transaction/request.rs` | AA struct fields, serde renames (`nonceKey`, `senderAuth`, `payerAuth`, `accountChanges`), `is_eip8130()` predicate, and `build_eip8130()` body **byte-identical**. Ours adds `From<TxPostExec>` and `From<super::Transaction>` impls + `PostExec` arm in `OpTypedTransaction`/`OpTxEnvelope` matches. Base adds two builder methods we lack: `chain_id(self, ChainId)` and `deploy_code(self, code)`. |

## BUG-CANDIDATES

### BUG-CAND-009: `OpTransactionReceiptFields::From` panics on serde failure

**Severity: LOW** (no current trigger path, but lurking)

**File**: `op-alloy/crates/rpc-types/src/receipt.rs:155-159`

```rust
impl From<OpTransactionReceiptFields> for OtherFields {
    fn from(value: OpTransactionReceiptFields) -> Self {
        serde_json::to_value(value).unwrap().try_into().unwrap()
    }
}
```

Base uses `TryFrom<OpTransactionReceiptFields> for OtherFields` returning
`Result<Self, serde_json::Error>` (lines 148-154). Ours uses an
infallible `From` with `.unwrap().try_into().unwrap()`. With current
struct fields it always succeeds, but if a future field with a serde
type that produces a non-object value is added, the conversion will
panic in production. Base has already migrated to the fallible form;
ours has not.

**Recommendation**: port base's `TryFrom` to maintain symmetry and
eliminate the latent panic. This is a divergence, not a port bug — the
unwrap form predates the AA work — but fixing it as part of this audit
keeps the surface aligned with upstream.

### BUG-CAND-010: doc comment in `block/native_aa.rs` references CREATE2 deployment of AccountConfiguration, but our deployment path is via op-node deposit transactions

**Severity: INFO / DOC**

**File**: `alloy-op-evm/src/block/native_aa.rs:13-17`

```rust
/// `AccountConfiguration` is NOT included — it is a real Solidity contract
/// deployed via deposit transactions at NativeAA activation (see
/// `op-node/rollup/derive/native_aa_upgrade_transactions.go`). The node gates
/// AA validation on its presence: before deployment, only the implicit EOA
/// rule applies.
```

This doc is correct, but it inverts BASE's wording (base/base_v1.rs:13-17
talks about `deploy-8130.sh` for devnet and "upgrade deposit transactions
on mainnet"). The substantive logic matches — both rely on a separate
deployment flow + sentinel check. No code bug, but the comment in our
copy is more permissive ("only the implicit EOA rule applies") than
base ("the node gates AA validation on its presence"). Worth verifying
that `op-reth/crates/txpool/src/eip8130_validate.rs:336` (which reports
`"AccountConfiguration contract not deployed"`) actually enforces the
same gating. Searching the txpool, it does — see line 263 ("contract
has not been deployed yet") returning a validation error. So semantics
match base. **No fix required**, but recommend tightening the doc.

### BUG-CAND-011: `OpTransactionRequest` lacks `chain_id` / `deploy_code` builder methods

**Severity: LOW** (API parity gap, not correctness)

**File**: `op-alloy/crates/rpc-types/src/transaction/request.rs:91-152`

Base provides:
```rust
pub const fn chain_id(mut self, chain_id: ChainId) -> Self { ... }
pub fn deploy_code(mut self, code: impl Into<Bytes>) -> Self { ... }
```

Ours does not. Downstream callers porting from base that build
`OpTransactionRequest::new(...).chain_id(...)` will fail to compile.
Pre-existing op-alloy ergonomic gap, surfaced by base's larger builder
API. Not an EIP-8130 correctness bug.

**Recommendation**: cherry-pick the two builder methods.

### BUG-CAND-012 (FYI / NON-BUG): `is_native_aa_active_at_timestamp` vs `is_base_v1_active_at_timestamp` rename consistency

**Severity: NONE** (semantically equivalent)

**Files**:
- `alloy-op-evm/src/env.rs:38` — `is_native_aa_active_at_timestamp => NATIVE_AA`
- `alloy-op-evm/src/block/native_aa.rs:45` — `if !chain_spec.is_native_aa_active_at_timestamp(timestamp)`
- `alloy-op-hardforks/src/lib.rs:252` — defines the predicate

Base uses `is_base_v1_active_at_timestamp` everywhere. The rename is
consistent across the three call sites in our crates. The activation
timestamp comparison logic
(`self.op_fork_activation(...).active_at_timestamp(...)`) is identical
to base (`active_at_timestamp` from `alloy-hardforks` `ForkCondition`).
**No action needed.**

### BUG-CAND-013: Test coverage gap — no test exercises `ensure_aa_predeploys` in the OURS tree

**Severity: LOW**

**Files**:
- OURS: `alloy-op-evm/src/block/native_aa.rs` — **zero tests** in this file
- BASE: `base/crates/common/evm/src/base_v1.rs:74-129` — three tests
  (`deploys_precompile_stubs_on_activation`, `idempotent_when_already_deployed`,
  `no_op_when_fork_inactive`)

Base ships unit tests for the equivalent `ensure_aa_predeploys` but
ours does not. Logic is byte-identical so risk is low, but a regression
in our trait-bound or activation gate would slip past CI.

**Recommendation**: port the three tests into
`alloy-op-evm/src/block/native_aa.rs`, adapting `BaseChainUpgrades` to
`OpChainHardforks` and using
`is_native_aa_active_at_timestamp`-activated config.

### BUG-CAND-014 (POSITIVE): EIP-8130 RPC receipt + request types are tight

**Severity: NONE**

`Eip8130ReceiptFields { payer, phase_statuses }` in both repos: same
fields, same serde attributes (camelCase, `skip_serializing_if`,
`flatten` on the parent `OpTransactionReceipt`). `OpTransactionRequest`
AA-field serde renames (`nonceKey`, `senderAuth`, `payerAuth`,
`accountChanges`) match exactly. `build_eip8130()` body is byte-equal.
JSON wire format will be compatible across base and OKX clients for
type-0x7B receipts and `eth_call`/`eth_estimateGas` requests. **No
action needed.**

## Confirmed-Equivalent Surface (no findings)

- `OpEvm::transact_raw`, `transact_system_call`, `finish`,
  `set_inspector_enabled`, `components`, `components_mut` — same
  bodies modulo type-name parameterization.
- `OpBlockExecutor::apply_pre_execution_changes` — calls
  `ensure_create2_deployer` then `ensure_aa_predeploys` in the same
  order as base.
- `OpAlloyReceiptBuilder::build_receipt` — `OpTxType::Eip8130 =>
  OpReceiptEnvelope::Eip8130(receipt)` arm matches base byte-for-byte.
- `OpHardfork::NativeAA` enum variant has correct `idx()` ordering
  (last) — `OpChainHardforks::Index<OpHardfork>` impl handles it.
