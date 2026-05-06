# Phase B — Consensus Types Cross-Reference

- **OURS**: `https://github.com/okx/optimism/tree/cf38ca5666/rust/op-alloy/crates/consensus/src/transaction/eip8130/`
- **BASE**: `https://github.com/base/base/tree/a33ab4d09/crates/common/consensus/src/transaction/eip8130/`
- **Generated**: 2026-05-05

## Summary
- **Files compared**: 18 file pairs (1:1 aligned by name)
- **Byte-identical files**: 11 (61%)
- **Functionally diverged files**: 7 — all changes accounted for by prior bug fixes (BUG-001/005/006) plus the global `from: Option<Address>` migration
- **Equivalent public symbols**: ~20 public fns + 5 const + 1 struct verified equal/equivalent
- **Divergent (suspected new bugs)**: **0** — no new bug-class divergences found at consensus-types level
- **Test-coverage gaps**: **1** — `tests.rs` (39 #[test] fns) is fully disabled in OURS pending Option-migration; partial replacement coverage exists in inline mod tests
- **Intentional fork extensions**: 0 in this layer (PostExec extension lives in op-revm + types layer, not here)

## Per-file findings

### File-level shasum check

| File | OURS lines | BASE lines | Identical? | Notes |
|---|---:|---:|---|---|
| abi.rs | 112 | 112 | ✅ byte-identical | |
| accessors.rs | 130 | 130 | ✅ byte-identical | |
| address.rs | 175 | 175 | ✅ byte-identical | |
| constants.rs | 190 | 190 | ✅ byte-identical | AA_TX_TYPE / AA_PAYER_TYPE / AA_SENDER_TYPE typehashes match |
| execution.rs | 261 | 261 | ✅ byte-identical | |
| gas.rs | 686 | 686 | ⚠️ tests-only diff | Test fixtures updated for `from: Option<Address>` / `payer: Option<Address>` (BUG-006-related migration). Production code unchanged. |
| mod.rs | 118 | 113 | ⚠️ test gating | `mod tests;` disabled in OURS (5 extra comment lines). See test-coverage gap below. |
| native_verifier.rs | 964 | 964 | ✅ byte-identical | All K1/P256/WebAuthn/Delegate verifier paths identical |
| precompiles.rs | 158 | 158 | ✅ byte-identical | |
| predeploys.rs | 142 | 126 | ⚠️ BUG-006 fix | OURS has post-fix CREATE-derived addresses (0xAb4eE49E…, 0xf946601D…, 0x6751c7ED…, 0x3572bb3F…, 0xc758A89C…). All cross-verified against `op-node/rollup/derive/native_aa_upgrade_transactions.go` `NativeAA*Deployer @ 0x4210…0008..000d` + `crypto.CreateAddress(deployer, 0)`. Match. |
| purity.rs | 971 | 971 | ✅ byte-identical | |
| signature.rs | 190 | 182 | ⚠️ BUG-001 fix | `payer_signature_hash` signature gained `resolved_sender: Address` parameter. Tests updated. All 3 production callers in OURS pass resolved sender. |
| storage.rs | 450 | 450 | ✅ byte-identical | |
| tests.rs | 636 | 635 | ⚠️ disabled | OURS file present but un-`mod`-ed; uses stale `from: Address` / `payer: Address` literals throughout `simple_tx` helper. |
| tx.rs | 797 | 790 | ⚠️ BUG-001 fix | `encode_for_payer_signing(&self, from: Option<Address>, out)` signature change; binds payer hash to resolved sender. Field order in `TxEip8130` struct is byte-identical with BASE. Caller in `tx.rs:664` passes `tx.from`. |
| types.rs | 470 | 470 | ✅ byte-identical | `AccountChangeEntry::{Create, Owner, Delegation}` enum + RLP order identical |
| validation.rs | 707 | 692 | ⚠️ BUG-005 fix | OURS adds `seen_delegation` short-circuit in `validate_account_changes` per EIP-8130 "at most one delegation per tx". Otherwise tests-only Option migration. |
| verifier.rs | 114 | 114 | ✅ byte-identical | |

### Public-symbol verification (key types/functions)

| Symbol | OURS | BASE | Status |
|---|---|---|---|
| `TxEip8130` struct fields + RLP order | tx.rs:28-66 | tx.rs:28-66 | ✅ identical (Option<Address> for both `from` and `payer` on both sides) |
| `AccountChangeEntry` enum | types.rs | types.rs | ✅ byte-identical |
| `AA_TX_TYPE` / `AA_PAYER_TYPE` / `AA_SENDER_TYPE` | constants.rs | constants.rs | ✅ byte-identical typehash values |
| `tx_hash`, `effective_sender`, `effective_payer` | tx.rs | tx.rs | ✅ identical bodies |
| `rlp_encode_fields` / `rlp_decode` (round-trip) | tx.rs:209/249 | tx.rs:209/249 | ✅ identical encode + decode field order |
| `encode_for_sender_signing` | tx.rs:270 | tx.rs:270 | ✅ identical |
| `encode_for_payer_signing` | tx.rs:310 (extra `from` param) | tx.rs:303 | ⚠️ intentional BUG-001 hardening; semantics: OURS forces caller to pass resolved sender |
| `payer_signature_hash` | signature.rs:57 (2 args) | signature.rs:49 (1 arg) | ⚠️ intentional BUG-001 hardening |
| `validate_account_changes` | validation.rs | validation.rs | ⚠️ BUG-005 hardening (multiple-delegation rejection) |
| `K1_VERIFIER_ADDRESS` | predeploys.rs:90 = `0x…0001` | predeploys.rs:90 = `0x…0001` | ✅ identical |
| `TX_CONTEXT_ADDRESS` / `NONCE_MANAGER_ADDRESS` | predeploys.rs | predeploys.rs | ✅ identical (`0x…aa03` / `0x…aa02`) |
| `DEFAULT_ACCOUNT_ADDRESS` etc. (5 deployed contracts) | predeploys.rs (BUG-006 fixed) | predeploys.rs (still OLD spec doc values) | ⚠️ already filed under BUG-006 |
| `try_native_verify` | native_verifier.rs | native_verifier.rs | ✅ byte-identical |
| `verify_account_purity` (purity.rs) | purity.rs | purity.rs | ✅ byte-identical |

## Suspected bugs

**No new bug candidates** at the consensus-types layer. All deltas are explicable by previously-filed BUG-001 / BUG-005 / BUG-006 fixes that base has not yet adopted (or, per BUG-006, base still has the wrong values — that's BASE's problem, not ours).

## Test-coverage gap (not a runtime bug, but worth flagging)

### GAP-CONSENSUS-TESTS — `tests.rs` 39 functions disabled
**File**: `op-alloy/crates/consensus/src/transaction/eip8130/mod.rs:113-118`
**Symptom**: `mod tests;` is commented out; 39 `#[test]` functions in `tests.rs` are not run by `cargo test`. The disablement comment claims "tests reference non-existent benches/fixtures/*.hex" — verified false: `tests.rs` has zero `fixtures` / `.hex` / `include_bytes` references. The real reason is unmigrated `from: Address::ZERO` / `from: Address::repeat_byte(0x42)` / `payer: Address::*` literals in the `simple_tx` helpers (lines 20-39 and 499-518) and several test bodies (e.g. lines 35, 281, 472, 514, 608, 315, 436), incompatible with the post-BUG-001 `Option<Address>` schema.
**Lost coverage** (representative sample):
- `eoa_k1_self_pay_roundtrip`, `eoa_k1_self_pay_eip2718_roundtrip` (RLP round-trip)
- `predeploy_addresses_unique` (would have caught BUG-006 regressions)
- `aa_tx_type_distinct` (0x7b sentinel uniqueness)
- `storage_slots_deterministic`
- `owner_config_pack_unpack`
- `validate_structure_oversized_sender_auth`
- `tx_hash_uniqueness`
- `complex_tx_with_all_features`
- `account_creation_entry_roundtrip` + `create2_address_derivation`
**Partial mitigation**: each module still has its own inline `#[cfg(test)] mod tests` block — gas.rs (26), validation.rs (18), signature.rs (5), tx.rs (9) = 58 tests still run. But these don't replace the integration-style coverage in tests.rs.
**Severity**: low (no runtime impact — pure test-coverage erosion)
**Fix path**: 1 hour mechanical sed: `s/from: \(Address::[A-Z_a-z]\+([^)]*)\)/from: Some(\1)/`, `s/payer: Address::ZERO/payer: None/`, `s/payer: Address::repeat_byte(0x\([0-9A-Fa-f]\+\))/payer: Some(Address::repeat_byte(0x\1))/`, then re-enable `mod tests;`. Also update the two `simple_tx` helper signatures.
**Test coverage**: GAP itself; once fixed, restores 39 tests.

## Reaffirmed equivalences

- All 11 byte-identical files re-confirmed (shasum match)
- `TxEip8130` struct field declaration order matches byte-for-byte → RLP encode/decode round-trip is wire-compatible with BASE
- `AccountChangeEntry` enum discriminant order in `types.rs` matches → BASE-encoded txs decode correctly in OURS and vice versa
- `AA_TX_TYPE_ID` / `AA_PAYER_TYPE` / `AA_SENDER_TYPE` typehash bytes match
- `K1_VERIFIER_ADDRESS = 0x…01` (ECRECOVER precompile sentinel) matches
- All 4 native verifier dispatch paths (K1, P256-raw, WebAuthn, Delegate) byte-identical
- 5 production callers of `payer_signature_hash` in OURS all pass the resolved sender (BUG-001 fix complete)

## Known divergences (intentional, not bugs)

- BUG-001 hardening: extra `resolved_sender` parameter in `payer_signature_hash` + `encode_for_payer_signing`
- BUG-005 hardening: multiple-delegation rejection in `validate_account_changes`
- BUG-006 fix: 5 predeploy addresses replaced with CREATE-derived values matching the Go deployer
- `mod tests;` disablement in mod.rs (test-only; tracked as GAP-CONSENSUS-TESTS above)

## Punch list

1. **No new BUG-CANDIDATE entries** — consensus-types layer is clean.
2. **GAP-CONSENSUS-TESTS** (low severity, hygiene): re-enable `tests.rs` after migrating ~40 stale `Address` literals to `Option<Address>` constructors. Restores 39 tests.
3. Suggest filing this as a follow-up tracking issue rather than a "bug".
