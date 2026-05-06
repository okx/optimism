# Phase B — Txpool & Receipts Cross-Reference

- **OURS txpool**: `https://github.com/okx/optimism/tree/cf38ca5666/rust/op-reth/crates/txpool/src/`
- **BASE txpool**: `https://github.com/base/base/tree/a33ab4d09/crates/txpool/src/`
- **OURS receipts**: `https://github.com/okx/optimism/tree/cf38ca5666/rust/op-alloy/crates/consensus/src/receipts/`
- **BASE receipts**: `https://github.com/base/base/tree/a33ab4d09/crates/common/consensus/src/receipts/`
- **Generated**: 2026-05-05

## Summary

- **Txpool files (shared concept)**: 10 pairs aligned 1:1; 8 ours-only files are xlayer/upstream-OP-fork features (`pool.rs` = OpPool wrapper, `supervisor/`, `interop.rs`, `maintain.rs`, `error.rs`, `conditional.rs`, `lib.rs` extras for OpPool wiring). Not flagged as "missing in base".
- **Receipt files**: 4 pairs (`mod.rs`, `envelope.rs`, `receipt.rs`, `deposit.rs`). Both implementations treat `Eip8130` as a plain `Receipt<T>` — **no `payer` or `phaseStatuses` extension fields exist in either side**, so the asymmetric encode/decode bug class hypothesised in the prompt is not possible. The user-supplied premise was incorrect.
- **AA branch in `validator.rs`**: 1:1 lift, identical pre-add flow (fork gate → `validate_eip8130_transaction` → L1 fee inclusion → AA pool admission → metadata attach → `propagate: false`).
- **L1 fee inclusion for AA tx**: present in ours, identical to base (`l1_tx_data_fee(...)` invoked at `validator.rs:290–315` ours = `validator.rs:282–307` base).
- **BUG-002 family (eviction paths)**: full enumeration completed. **All four `BasePool` mutation paths in ours forward to `eip8130_pool.remove_transactions`**: `remove_transactions`, `remove_transactions_and_descendants`, `remove_transactions_by_sender`, `prune_transactions`, plus the new `on_canonical_state_change` mined-tx eviction (BUG-002 fix). No further leak vectors found. **Base still lacks the `on_canonical_state_change` fix** (already known per prior-findings).
- **Divergent (suspected new bugs)**: **0**. All apparent diffs are (a) path/type renames `base_alloy_*` → `op_alloy_*`/`reth_optimism_*`, (b) prior-fix integrations (BUG-001 payer-hash binding, BUG-002 mined-AA eviction), (c) ours-only OP fork additions (interop, supervisor, conditional, OpPool reorg state), or (d) the okx fork's `OpTxType::PostExec` receipt arm.

## Per-file findings

### Txpool

| File | OURS lines | BASE lines | Identical-after-rename? | Notes |
|---|---:|---:|---|---|
| `eip8130_invalidation.rs` | 25172 | 25140 | ✅ logic identical | Only differences: import paths, `nonce_slot(sender, U256::from(tx.nonce_key))` (base) vs `nonce_slot(sender, tx.nonce_key)` (ours) — `tx.nonce_key` is already `U256`, so the cast is a no-op; both are equivalent. `lock_slot` / `sequence_base_slot` invocations identical. Doc-comment trivial reformat. |
| `eip8130_pool.rs` | 78822 | 78834 | ✅ byte-identical (modulo rename) | All eviction paths (`try_evict_one`, `update_sequence_nonce`, `sweep_expired`, `remove_transactions`, capacity trim in `add_transaction`) are character-for-character identical after `BasePooledTransaction`→`OpPooledTransaction` rename. |
| `eip8130_validate.rs` | 76196 | 78822 | ⚠️ BUG-001 fix + revm 38 adapt | (1) `payer_signature_hash(tx, sender)` ours vs `payer_signature_hash(tx)` base — ours has BUG-001 fix; base does not (also documented in prior-findings). (2) `verify_custom_via_evm` adapted for revm 38: `CallInputs.reservoir = 0`, `known_bytecode` is now non-Optional `(B256, Bytecode)` populated via `load_account_with_code` (ours adds 12 lines around line 601). (3) `OpSpecId::BASE_V1` → `OpSpecId::NATIVE_AA`. No semantic divergence; the cap-handling, fork gate, and verifier-policy logic match. |
| `validator.rs` | 23319 | 20786 | ⚠️ rename + interop additions | AA branch (the lines `if transaction.ty() == AA_TX_TYPE_ID { ... }`) is **byte-identical** between sides except for the BUG-001-driven `payer_signature_hash` plumbing. Diffs concentrated on (a) `OpHardforks` vs `BaseUpgrades`, (b) `is_native_aa_active_at_timestamp` vs `is_base_v1_active_at_timestamp` (different fork-name registration; both resolve to the AA-active gate), (c) ours adds `supervisor_client` + `OpForkTracker` + `is_valid_cross_tx` cross-tx interop check (post-AA-branch, not affecting AA flow). |
| `base_pool.rs` | 21812 | 20412 | ⚠️ BUG-002 fix + add_transactions adapter | (1) Ours' `BaseTransactionPool::on_canonical_state_change` calls `self.eip8130_pool.remove_transactions(&update.mined_transactions)` **before** delegating to inner — the BUG-002 fix. Base still does only `self.protocol_pool.on_canonical_state_change(update);`. (2) Ours adds an explicit `add_transactions(...)` and changes `add_transactions_with_origins` signature from `impl IntoIterator<...>` to concrete `Vec<...>` to match the upstream-reth `TransactionPool` trait shape. All four mutation paths (`remove_transactions`, `remove_transactions_and_descendants`, `remove_transactions_by_sender`, `prune_transactions`) correctly forward to `eip8130_pool.remove_transactions(...)` on both sides. |
| `transaction.rs` | 14553 | 18121 | ⚠️ rename + xlayer-feature delete | Ours **removes** base's bundle-ish features (`BLOCK_TIME_SECS`, `MAX_BUNDLE_ADVANCE_SECS`, `received_at`, `target_block_number`, `min_timestamp`, `max_timestamp`, `unix_time_millis`, `BundleTransaction`, `TimestampedTransaction`) — these are base-fork bundle/block-builder features absent in upstream OP. Ours **adds** `conditional` + `interop` (OP-fork features). The `OpPooledTx::as_eip8130` impl is semantically identical (both unwrap the sealed `TxEip8130`). `Eip8130Metadata` struct field order/types identical. |
| `best.rs` | 9661 | 9671 | ✅ byte-identical (modulo rename) | Test-only file; `BestTransactions` ordering logic identical. |
| `lib.rs` | 2428 | 3714 | ⚠️ pub-API reorg | Ours doesn't re-export base's bundle/forwarder/consumer/builder/wire modules (those crates don't exist in ours). All EIP-8130-relevant exports preserved (`Eip8130*`, `MAX_AA_TX_ENCODED_BYTES`, `validate_eip8130_transaction`, `compute_account_tier`, `OpTransactionValidator`, `OpPooledTx`, `OpPooledTransaction`). |
| `estimated_da_size.rs` | 384 | 384 | ✅ byte-identical | |
| (ours-only: `pool.rs` 25980, `supervisor/`, `interop.rs` 1519, `maintain.rs` 12246, `error.rs` 1016, `conditional.rs` 1068) | — | — | n/a | xlayer/upstream-OP fork extensions; not part of EIP-8130 port. `OpPool::on_canonical_state_change` (line 353-368) wraps `BaseTransactionPool::on_canonical_state_change`, so the BUG-002 eviction path is reachable through the wrapper. |

### Receipts

| File | OURS lines | BASE lines | Identical-after-rename? | Notes |
|---|---:|---:|---|---|
| `mod.rs` | 568 | 846 | ⚠️ minor reorg | Ours flips `mod {deposit,receipt}` from private to `pub(crate)` and moves the `serde_bincode_compat` re-export from `receipts/mod.rs` to `lib.rs:67-73`. End-user import path (`op_alloy_consensus::serde_bincode_compat::OpReceipt`) unchanged. |
| `envelope.rs` | 17744 | 16464 | ⚠️ +PostExec arm | Ours adds `OpReceiptEnvelope::PostExec` (tx-type 0x7D) — the okx fork's synthetic post-exec receipt. All EIP-8130 paths (`Eip8130` arm in `from_parts`, `tx_type`, `map_logs`, `logs_bloom`, `into_receipt`, `as_receipt`, `inner_length`, encoding length, encode body, decode dispatch) are character-identical between sides. Doc-rebrand `Base chains` → `Optimism / OP Stack chains`. **arbitrary impl** still omits the `Eip8130` arm in both sides — pre-existing shared minor (not a port bug). |
| `receipt.rs` | 33968 | 31851 | ⚠️ +PostExec arm | Same shape as `envelope.rs`: ours adds `OpReceipt::PostExec` and `serde_bincode_compat::OpReceipt::PostExec`. RLP encode/decode for `Eip8130` is identical: same `RlpDecodableReceipt::rlp_decode_with_bloom` dispatch (ours line 193-197, base equivalent), same `rlp_encode_fields_without_bloom` body grouping `Eip8130` with the pre-byzantium-typed receipts. **No `payer` or `phaseStatuses` extension fields exist on either side** (ripgrep confirms). Receipt round-trip is identical between sides. |
| `deposit.rs` | 20031 | 20014 | ✅ byte-identical (modulo doc rename) | Doc-comment changes only. |

## BUG-CANDIDATE list

**None** — no new bugs at this layer. Confirmations / non-issues below.

### Confirmed (already in prior-findings)

| Tag | Severity | File:line | Status |
|---|---|---|---|
| BUG-001 (payer-hash sender binding) | Security/CRIT | `eip8130_validate.rs:996` ours / `:969` base | OURS fixed at `91e7606a6f`; **base still vulnerable** (`payer_signature_hash(tx)`) — already documented as shared-with-base in prior-findings. |
| BUG-002 (mined AA tx eviction) | Liveness/HIGH | `base_pool.rs:560-579` ours / `:550-551` base | OURS fixed at `0b07aa79e1`; **base still leaks** — already documented. |

### Investigated and ruled out

| Topic | Result |
|---|---|
| BUG-002 family — other eviction paths (reorg / replacement / capacity-trim) that don't forward to `eip8130_pool.remove_transactions` | **None found.** All four `BaseTransactionPool` mutation paths (`remove_transactions`, `remove_transactions_and_descendants`, `remove_transactions_by_sender`, `prune_transactions`) explicitly forward to `eip8130_pool.remove_transactions`. Capacity-trim and reorg replacement happen inside `eip8130_pool` itself (`try_evict_one`, `update_sequence_nonce`) which mutates the AA pool's own indices atomically. `update_accounts` is forwarded only to the standard pool — but AA nonce/balance liveness updates flow through `update_sequence_nonce` driven by canon-state storage diffs in `maintain_eip8130_invalidation`, identical on both sides. |
| Validator AA-branch logic match | **Identical.** Diff is purely (a) constant-import path, (b) fork-gate name (`is_native_aa_active_at_timestamp` ↔ `is_base_v1_active_at_timestamp`). The 80+ lines of AA-branch code (validate → l1 fee → eip8130_pool admit → metadata attach) match line-for-line. |
| L1 fee inclusion for AA tx | **Present.** `l1_info.l1_tx_data_fee(...)` called at `validator.rs:290` ours / `:282` base, with identical args (`chain_spec, block_timestamp, &encoded, false`) and identical insufficient-funds check (`total = transaction.cost() + l1_cost`). |
| Receipt extra fields (`payer`, `phaseStatuses`) | **Do not exist on either side.** Both treat AA receipts as plain `Receipt<T>` (status, cumulative_gas_used, logs only). Per spec, AA-specific metadata is conveyed via logs/events, not as receipt fields — both implementations align. The hypothesised asymmetric encode/decode bug class is not possible here. |
| Receipt encode/decode (`to_compact`/`from_compact` round-trip) | **Identical.** Neither side has a separate `Compact` impl in these files. RLP encode (`rlp_encode_fields_without_bloom`, `rlp_encoded_fields_length_without_bloom`) and decode (`rlp_decode_inner`, `rlp_decode_fields_without_bloom`) treat `Eip8130` symmetrically with `Legacy/Eip2930/Eip1559/Eip7702`. Round-trip test for `Eip8130` exists implicitly via the new `post_exec_receipt_roundtrip` test ours added (the same code-path covers `Eip8130`). |
| `arbitrary` impl missing `Eip8130` arm | **Pre-existing on both sides.** Ours' diff only adds the `PostExec` arm (now `0..=5`); the `Eip8130` arm was never added in either codebase. Test-fuzz only — does not affect production correctness. Flag for follow-up if anyone runs receipt-arbitrary tests against AA receipts. |

## Notes & caveats

- The `nonce_slot(sender, U256::from(tx.nonce_key))` → `nonce_slot(sender, tx.nonce_key)` simplification in ours' `eip8130_invalidation.rs:175` is safe (`tx.nonce_key: U256`, the cast was a no-op).
- The 8 ours-only txpool files (`pool.rs`, `supervisor/`, `interop.rs`, `maintain.rs`, `error.rs`, `conditional.rs`) are upstream-OP fork features that wrap `BaseTransactionPool`; verified that the wrapping `OpPool::on_canonical_state_change` delegates to `self.inner.on_canonical_state_change(update)`, so the BUG-002 fix path is reachable.
- The `OpTxType::PostExec` receipt arm is the okx fork's pre-existing extension and is intentional; verified it does not regress any EIP-8130 dispatch arm.
