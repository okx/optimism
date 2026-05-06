# Phase B — revm Extension Layer Cross-Reference

- **OURS**: `https://github.com/okx/optimism/tree/cf38ca5666/rust/op-revm/src/`
- **BASE**: `https://github.com/base/base/tree/a33ab4d09/crates/execution/revm/src/`
- **Generated**: 2026-05-05

## Summary

- **Files compared**: 12 functional files (ours has `handler_aa_helpers.rs` carved out of base's monolithic `handler.rs`; ours has `fast_lz.rs`; base has `compat.rs` + `rollup_config.rs` not needed in ours)
- **Byte-identical (modulo `Optimism`/`Base` namespace renames)**: `eip8130_policy.rs`, `result.rs`, `evm.rs`, `api.rs`, `api/builder.rs`, `api/default_ctx.rs`
- **Equivalent public symbols**: ~30 fns / 8 consts / 4 structs verified
- **Behavioral divergences from BASE found**: **3 not in BUG-001..008**
- **Intentional fork extensions** (don't flag): `eip8130_policy.rs` (shared, but identical anyway), KARST/INTEROP spec variants, `OpPrecompiles` struct name, revm-38 reservoir/state-gas-spent additions
- **Pre-existing fixes already deployed** (don't re-report): BUG-003 (empty calls), BUG-004 (any vs all + short-circuit), BUG-006/007 (ACCOUNT_CONFIG_ADDRESS post-fix value)

## Per-file findings

| File | OURS lines | BASE lines | Identical? | Notes |
|---|---:|---:|---|---|
| `api.rs` | 9 | 10 | ✅ same | mod-statement reorganization only |
| `api/builder.rs` | — | — | ✅ identical | only namespace rename |
| `api/default_ctx.rs` | — | — | ✅ identical | only namespace rename |
| `api/exec.rs` | 170 | 230 | ⚠️ **MISSING workaround** | See BUG-CANDIDATE-A below — base has `load_account_with_code_mut(caller)` calls in `system_call_one_with_caller` and `inspect_one_system_call_with_caller` to satisfy revm bluealloy/revm#3484; ours does not. |
| `constants.rs` | 117 | 116 | ✅ equivalent | `DELEGATE_VERIFIER_ADDRESS` differs (`0xc758A89C…` vs `0x30A76831…`) but is the **chain-specific** CREATE-derived value; ours matches op-alloy `predeploys.rs` line 124. Not a bug. |
| `eip8130_policy.rs` | 83 | 83 | ✅ byte-identical | comment-only diff (header) — `PendingOwnerState`, `PendingOwnerValidationError`, `owner_scope_allows`, `validate_pending_owner_state`, `pending_owner_state_for_change` all byte-equal |
| `evm.rs` | 163 | 164 | ✅ identical | only namespace rename + 1 import-order line |
| `handler.rs` + `handler_aa_helpers.rs` | 2097 + 632 | 3511 (combined) | ⚠️ multiple known fixes + ours ahead of base on 2 protocol semantics | Phase loop, gas math, and slot derivations identical. Notable port-side gains: BUG-003/004 fixes (already in skill), revm-38 reservoir + `state_gas_spent` fields. See deltas below. |
| `l1block.rs` | 670 | 667 | ⚠️ revm-38 reservoir | Ours subtracts reservoir from used gas in fee computation; base does not (revm-version diff, not AA-specific). |
| `lib.rs` | 31 | 56 | ✅ same surface | mod-statement reorganization only |
| `precompiles.rs` | 949 | 1133 | ⚠️ slightly different KARST construction | See note below. AA precompile dispatch is **byte-identical** in `run`/`warm_addresses`/`contains`. |
| `result.rs` | 43 | 44 | ✅ identical | only namespace rename |
| `spec.rs` | 263 | 266 | ⚠️ extra fork variants | Ours adds `KARST`, `INTEROP`, `NATIVE_AA`. Base has `BASE_V1`. See finding C. |
| `transaction.rs` | 30 | 79 | informational | Mostly re-export reorg. |

### Public-symbol verification — key fns

| Symbol | OURS | BASE | Status |
|---|---|---|---|
| `EIP8130_TX_TYPE = 0x7B` | precompiles.rs:23 + handler_aa_helpers.rs:38 | precompiles.rs:208 + handler.rs:43 | ✅ value match |
| `NONCE_MANAGER_ADDRESS / TX_CONTEXT_ADDRESS` | precompiles.rs:26-31 | precompiles.rs:212-216 | ✅ byte-equal (`0x…aa02`, `0x…aa03`) |
| `TX_CONTEXT_GAS = 100`, `NONCE_MANAGER_GAS = 2_100` | precompiles.rs:37,40 | precompiles.rs:222,225 | ✅ match |
| `NONCE_BASE_SLOT = 1`, `LOCK_BASE_SLOT = 1`, `EXPIRING_*_BASE_SLOT = 2/3/4` | handler_aa_helpers.rs:92-106 | handler.rs:87-101 | ✅ all match |
| `EXPIRING_NONCE_SET_CAPACITY = 300_000`, `NONCE_FREE_MAX_EXPIRY_WINDOW = 30` | handler_aa_helpers.rs:108-110 | handler.rs:103-105 | ✅ match |
| `NONCE_COLD_WARM_DELTA = 17_100`, `ESTIMATION_AUTH_CALLDATA_GAS = 1_100` | handler_aa_helpers.rs:45,52 | handler.rs:50,57 | ✅ match |
| `K1_VERIFIER_ADDRESS` (=0x…01), `REVOKED_VERIFIER` (=0xff..) | handler_aa_helpers.rs:71-79 | handler.rs:70-78 | ✅ match |
| `aa_nonce_slot`, `aa_lock_slot`, `aa_owner_config_slot`, `aa_expiring_seen_slot`, `aa_expiring_ring_slot` | handler_aa_helpers.rs | handler.rs | ✅ byte-equal bodies (only privacy diff: ours marks `pub(crate)` so precompiles.rs and txpool can reuse) |
| `validate_owner_against_effective_config` | handler_aa_helpers.rs:203 | handler.rs:196 | ✅ byte-equal body |
| `validate_owner_config` | handler_aa_helpers.rs:305 | handler.rs:298 | ✅ byte-equal body |
| `validate_native_verifier_owner` | handler_aa_helpers.rs:332 | handler.rs:325 | ✅ byte-equal body |
| `validate_config_change_preconditions` | handler_aa_helpers.rs:386 | handler.rs:379 | ✅ byte-equal body (incl. lock-window check, sequence chaining) |
| `run_custom_verifier_staticcall` | handler_aa_helpers.rs:467 | handler.rs:460 | ✅ semantically equal (ours adds revm-38 `reservoir: 0` + `known_bytecode` fields to `CallInputs`; gas accounting unchanged) |
| `validate_authorizer_chain` | handler_aa_helpers.rs:554 | handler.rs:538 | ✅ byte-equal body |
| `run_nonce_manager_precompile`, `run_tx_context_precompile`, `encode_calls_abi`, `selector` | precompiles.rs | precompiles.rs | ✅ byte-equal bodies (only `record_regular_cost` vs `record_cost` rename, revm-38) |
| `Eip8130TxContext::new` | precompiles.rs:95 | precompiles.rs:70 | ✅ byte-equal body (`max_cost = (gas_limit + known_intrinsic + custom_verifier_gas_cap) * max_fee_per_gas`) |
| `validate_initial_tx_gas` (AA branch) | handler.rs:180 | handler.rs:763 | ✅ byte-equal logic |
| `validate_against_state_and_deduct_caller` (AA branch) | handler.rs:208 | handler.rs:789 | ✅ byte-equal AA body (signature differs by revm-38 extra `_init_and_floor_gas` parameter) |
| `execution` (AA phase loop) | handler.rs:551 | handler.rs:1018+ | ⚠️ ours has BUG-003/004 fixes; base does not (skill-known) |
| `last_frame_result` | handler.rs:931 | handler.rs:1389 | ⚠️ ours adds revm-38 reservoir + `state_gas_spent` reset; AA path constructs `result_gas` with both = 0, so net behavior unchanged for AA |
| `reimburse_caller` (AA payer-refund branch) | handler.rs:1007 | handler.rs:1421 | ✅ byte-equal AA body — payer (not caller) gets refund + operator-fee refund |

### KARST / NATIVE_AA precompile construction (precompiles.rs)

Both end states equivalent, but the *construction path* differs:

- **Ours `karst()`** (lines 428–442): `let mut p = jovian().clone(); p.difference(&{BERLIN, P256VERIFY}); p.extend([modexp::OSAKA, P256VERIFY_OSAKA]);`
- **Base `base_v1()`** (lines 520–527): `let mut p = jovian().clone(); p.extend([modexp::OSAKA, P256VERIFY_OSAKA]);`

`Precompiles::extend` is keyed by address (verified via `revm-precompile-28.1.1/src/secp256r1.rs`: both `P256VERIFY` and `P256VERIFY_OSAKA` resolve to address 0x100; both MODEXP variants share address 0x05). Since `extend` overwrites by address, base's path produces the same final set as ours' explicit-difference path. **Equivalent — not a bug.**

`new_with_spec` dispatch (ours lines 360-376, base 105-115): ours maps `KARST | INTEROP | NATIVE_AA → karst()`. Base maps `BASE_V1 → base_v1()`. Both gate the AA precompile dispatch on `eip8130_precompiles_enabled(spec)` which returns `true` only for `OpSpecId::NATIVE_AA` / `OpSpecId::BASE_V1`. **Equivalent — INTEROP/KARST do not enable AA precompiles, by design.**

## BUG-CANDIDATE list

### BUG-CANDIDATE-A — Missing system-call witness load (Geth proofs compatibility)
- **Severity**: MEDIUM (proof-witness divergence, not consensus-critical at execution layer; could break Geth-style witness consumers)
- **Wiring**: `op-revm/src/api/exec.rs:137-145` (`system_call_one_with_caller`) and `:155-168` (`inspect_one_system_call_with_caller`)
- **Symptom**: For non-AA system transactions (e.g., the `L1Block` setter, beacon-root setter, etc.), the caller account is not pre-loaded into the journal. Base added `self.0.ctx.journal_mut().load_account_with_code_mut(caller)?;` after `set_tx(...)` and before `run_system_call(self)` to keep the system-call caller present in `State.cache.accounts`, which `ExecutionWitnessRecord` uses to build `hashed_state` for `state_provider.witness(...)`. Without it, the caller is missing from the witness, breaking Geth proof consumers (revm bluealloy/revm#3484).
- **Effect on EIP-8130**: indirect. The L1Block update is a system tx that runs every block; missing caller in the witness can cause witness/proof divergence between ours and base on every block. AA-flow itself (regular AA txs) is **not** routed through `system_call_one_with_caller`, so AA execution is unaffected, but execution-witness equality vs base is.
- **Shared with base**: ❌ — base has the workaround; ours does not.
- **Suggested fix**: in both methods, after `self.0.ctx.set_tx(...)` and before instantiating `OpHandler`, call `self.0.ctx.journal_mut().load_account_with_code_mut(caller)?;`. Mirror base's two-line patch.
- **Test**: would surface in execution-witness regression tests if such exist; not currently part of the 8130 SDK suite.

### BUG-CANDIDATE-B — `OpSpecId::INTEROP` ordering inverts EVM-rule precedence vs `KARST`
- **Severity**: LOW (no consensus harm in current codebase; latent correctness trap if someone wires INTEROP into a hardfork timeline)
- **Wiring**: `op-revm/src/spec.rs:48-53`
- **Symptom**: ours' enum order is `JOVIAN(108) < KARST(109) < INTEROP(110) < NATIVE_AA(111)`. Therefore `OpSpecId::INTEROP.is_enabled_in(KARST) == true` (numerically `>=`), implying INTEROP "enables" all KARST features. But `into_eth_spec()` maps `KARST | NATIVE_AA → SpecId::OSAKA` while `INTEROP → SpecId::PRAGUE`. So a tx running on INTEROP gets OSAKA-not-yet-active EVM rules even though KARST features are claimed enabled. This contradicts the standard `is_enabled_in` ⇒ `into_eth_spec` ordering invariant maintained for all other variants (e.g., JOVIAN→PRAGUE → ISTHMUS→PRAGUE: monotone non-decreasing).
- **Effect on EIP-8130**: AA txs are gated by `is_enabled_in(NATIVE_AA)`, which still rejects INTEROP correctly. But any other call path that uses `is_enabled_in(KARST)` to gate OSAKA-only features (CLZ opcode, OSAKA-priced MODEXP/P256) on an INTEROP chain would falsely succeed and then fail at EVM-rule evaluation time.
- **Shared with base**: ❌ — base does not have INTEROP/KARST variants.
- **Reason this isn't auto-classified as a "fork-extension don't flag"**: ours' own `is_enabled_in`/`into_eth_spec` pair is internally inconsistent. The skill's "don't flag fork extensions" applies to additive variants; an additive variant that **subverts an established invariant on existing variants** is a defect.
- **Suggested fix**: either reorder `INTEROP` to `< KARST` (so INTEROP enables only Jovian and earlier), or have INTEROP also map to `OSAKA` if it is intended to inherit KARST EVM rules.
- **Test**: T-? — not currently exercised; would need a unit test asserting `OpSpecId::INTEROP.into_eth_spec().is_enabled_in(SpecId::OSAKA) == false` while `OpSpecId::KARST.into_eth_spec().is_enabled_in(SpecId::OSAKA) == true` AND `INTEROP.is_enabled_in(KARST) == true`, which today all pass simultaneously and document the inconsistency.

### BUG-CANDIDATE-C — `clear_eip8130_tx_context()` placement: defensive in ours, but check ordering still leaves a leak window on exception paths
- **Severity**: INFO / hardening (no exploit path identified)
- **Wiring**: ours `handler.rs:222-223`; base `handler.rs:835-836`.
- **Symptom**: Both implementations clear the EIP-8130 thread-local **only** at the start of `validate_against_state_and_deduct_caller`. Neither clears at the end of execution / on error paths. Ours places the clear *before* the deposit branch (so deposits also clear), base places it *after* the deposit branch (so deposits do **not** clear). However, neither implementation clears in `catch_error` / `execution_result` / panic-unwind paths, so if a thread is reused and an `Err(...)` aborts before the next tx reaches `validate_against_state_and_deduct_caller`, a subsequent read via `get_eip8130_tx_context()` would return stale data.
- **Effect on EIP-8130**: practically benign because the precompile dispatch in `OpPrecompiles::run` independently gates on `aa_context = (tx.tx_type() == EIP8130_TX_TYPE)` — so non-AA txs never reach the thread-local. But the defense-in-depth posture is incomplete.
- **Shared with base**: ✅ — both have the same gap.
- **Suggested fix**: also call `clear_eip8130_tx_context()` in `execution_result` (after `commit_tx`) and in `catch_error`, mirroring how `chain_mut().clear_tx_l1_cost()` is called there for L1 cost.

## Items NOT flagged (already covered or fork-extension)

- `OpSpecId::NATIVE_AA / KARST / INTEROP` extra variants — fork extensions per skill.
- `ACCOUNT_CONFIG_ADDRESS = 0xf946601d…` — already covered by BUG-006/007 (ours' value is post-fix).
- Phase loop "all phases succeed" + "break on revert" — already covered by BUG-003/004.
- `record_regular_cost` vs `record_cost`, revm-38 `CallInputs::reservoir` / `known_bytecode` fields, `state_gas_spent` accounting in `last_frame_result`, reservoir-aware fee math in `l1block.rs` — revm version difference; ours is on revm-38, base on older.
- `DELEGATE_VERIFIER_ADDRESS` differing between ours/base — chain-specific CREATE-derived value; ours matches op-alloy. Not a bug.
- KARST `difference + extend` construction vs base's `extend`-only — equivalent end state because `Precompiles::extend` overwrites by address.

## Verification commands

```text
diff op-revm/src/eip8130_policy.rs base/.../revm/src/eip8130_policy.rs
# only header comment differs

grep -n "load_account_with_code_mut(caller)" op-revm/src/api/exec.rs
# returns nothing  ← BUG-CANDIDATE-A

grep -n "load_account_with_code_mut(caller)" base/.../revm/src/api/exec.rs
# returns 2 hits in system_call_one_with_caller / inspect_one_system_call_with_caller

# INTEROP inversion (BUG-CANDIDATE-B)
grep -n "INTEROP\|KARST" op-revm/src/spec.rs
# enum: JOVIAN < KARST < INTEROP < NATIVE_AA;
# into_eth_spec: KARST→OSAKA, INTEROP→PRAGUE  ← invariant break
```
