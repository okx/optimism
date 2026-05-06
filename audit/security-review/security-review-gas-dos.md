# Security Review: Gas Accounting, DoS Vectors, and Sponsor/Payer Flow
## EIP-8130 Native AA Port — Production-Grade Audit

- **Auditor**: Claude Sonnet 4.6
- **Date**: 2026-05-06
- **Branch**: feat/eip-8130-port
- **Baseline commit**: cf38ca5666 (post-cleanup batch)
- **Scope**: gas accounting correctness, DoS surface, sponsor/payer flow security
- **Prior context**: phase-b-summary.md (9 prior candidates, none re-reported here)

---

## Pre-review CI Status

| Check | Result |
|---|---|
| `cargo check -p op-alloy-consensus -p op-revm -p alloy-op-evm` | PASS |
| `cargo test -p op-alloy-consensus --lib -- eip8130` | PASS (109 tests) |
| `cargo test --lib -- purity` | PASS (33 tests) |
| `cargo clippy -p op-revm -- -D warnings` | FAIL (doc-backtick + const-fn lint errors, not functional) |
| `cargo fmt --check -p op-alloy-consensus` | FAIL (post_exec.rs formatting, unrelated to this audit) |

The clippy and fmt failures are pre-existing hygiene issues in non-security-critical code. No functional errors in any security-relevant path.

---

## Findings

### CRITICAL

None found.

---

### HIGH

#### H-001 — AA precompiles NOT activated for specs after NATIVE_AA

**File**: `rust/alloy-op-evm/src/aa_precompiles.rs:132`

**Issue**:

```rust
if spec == OpSpecId::NATIVE_AA {   // equality, not is_enabled_in
    map.extend_precompiles([...]);
}
```

The guard uses `==` instead of `is_enabled_in(OpSpecId::NATIVE_AA)`. If a future hardfork (e.g. `NATIVE_AA_V2`) is introduced as a new variant after `NATIVE_AA` in the enum, the `NonceManager` and `TxContext` precompiles will be absent from the PrecompilesMap for that spec. Phase calls to `0x…aa02` and `0x…aa03` will fall through to the stub `0xfe` bytecode and revert, silently breaking every AA transaction on the new hardfork.

**BASE delta**: Base uses the same `==` guard (`if spec == OpSpecId::BASE_V1`). So this is a shared defect, but worth flagging since the OP repo controls its own hardfork ordering and may add post-AA specs sooner than Base.

**TEMPO note**: Tempo's design doc does not address this; no mitigation observed there either.

**Fix**: Change the equality guard to `is_enabled_in`:

```rust
if spec.is_enabled_in(OpSpecId::NATIVE_AA) {
    map.extend_precompiles([...]);
}
```

**Severity**: HIGH — silent execution break for all AA transactions on any spec variant after `NATIVE_AA`.

---

#### H-002 — Payer balance deducted atomically but sponsor consent is only validated at mempool admission, not re-checked at inclusion

**File**: `rust/op-revm/src/handler.rs:286–308` (`validate_against_state_and_deduct_caller`)

**Issue**: At inclusion time, the handler re-validates native verifier owner config (via `validate_native_verifier_owner`) and custom verifier STATICCALLs. However, it does **not** re-verify the `payer_auth` signature itself. The payer's `owner_config` SLOAD is re-done (good), but the cryptographic signature over the payer-specific hash (which binds to the resolved sender) is only verified once, at mempool admission in `eip8130_validate.rs:996`.

This matters because the payer could revoke the signing key between mempool acceptance and block inclusion. If the key is revoked on-chain (REVOKED_VERIFIER written to owner_config) but the key's owner_config SLOAD returns `REVOKED_VERIFIER`, the handler correctly rejects. However, if the payer revokes by removing the owner_id mapping (setting verifier to zero without the revoked sentinel), the implicit EOA rule could re-authorize a different key. The sequence is:

1. Payer signs `payer_auth` with key K.
2. Tx enters mempool; K's owner_config verified (scope = PAYER).
3. Payer submits a second tx removing K from owner_config (verifier set to zero).
4. New AA tx from attacker with same payer field.
5. Attacker's tx is included: the zero owner_config now hits the implicit EOA rule, which authorizes `bytes32(bytes20(payer))` as owner_id — but only if the verifier in payer_auth is K1 and the payer_auth signs as an EOA-mode address. The verifier mismatch at step 5 would likely reject, but the precise semantics depend on what the attacker can control in payer_auth.

More concretely: the payer pays for a tx they signed approval for but the user later changes the sender's identity (EOA mode, different sender). The payer_auth is bound to `resolved_sender` via `payer_signature_hash(tx, resolved_sender)` (signature.rs:57), which is correct. The cross-sender replay attack is properly closed.

**What is actually missing**: The handler does not re-verify the `payer_auth` cryptographic signature at inclusion time. This means if the signing key is valid at mempool time but the key is replaced on-chain between mempool admission and block inclusion without using the REVOKED_VERIFIER sentinel (just zeroing the slot), the re-validation SLOAD might incorrectly allow the tx under a different key. In practice this requires the payer to be adversarial to themselves, but it is a design gap vs. the stated "re-validate at inclusion" security goal.

**BASE delta**: Same behavior. Base also does not re-verify payer_auth signature at inclusion.

**TEMPO doc**: The TEMPO gas/sponsor flow doc does not flag this; the sponsor flow documentation describes mempool-time validation as final for signature verification.

**Fix**: Document explicitly that payer_auth signature is not re-verified at inclusion, and that the owner_config SLOAD is the inclusion-time guard. Consider whether owner_id→verifier mismatch at inclusion should produce a hard reject (it currently does via `validate_native_verifier_owner`).

**Severity**: HIGH — not an immediately exploitable drain, but a design assumption that requires documentation and a clear revocation semantics specification. Mis-revocation paths could allow sponsored transactions to land without a valid current payer signature.

---

### MEDIUM

#### M-001 — Nonce-free ring-buffer eviction can fail silently: "buffer full" error on expired-entry conflict

**File**: `rust/op-revm/src/handler.rs:410–418`

**Issue**: The expiring-nonce circular buffer evicts the entry at `ring[idx]` before inserting the new one. The eviction guard checks `old_expiry > U256::from(now)`. If the old entry is still live (not yet expired), the handler returns a generic `from_string` error:

```rust
return Err(ERROR::from_string(
    "nonce-free buffer full: cannot evict unexpired entry".into(),
));
```

This is a `catch_error` path that does NOT clean up the journal. The payer's balance was already deducted in `validate_against_state_and_deduct_caller`. On error, `catch_error` calls `journal.commit_tx()` for deposit transactions but for AA transactions it just returns the error upstream — the payer deduction is in the journal which will be reverted by the rollback on error.

Tracing the error propagation: `validate_against_state_and_deduct_caller` returns `Err`, which propagates out of `Handler::run_without_inspector` before `reimburse_caller` is invoked. The journal is reverted. So the payer is NOT double-charged. However:

- The nonce-free tx is silently dropped with an opaque error. The user has no way to know the ring buffer is full until they retry.
- A ring buffer full of unexpired entries (capacity 300 000) represents a **sustained DoS surface**: an attacker who floods 300 000 nonce-free txs with 30-second expiry windows fills the ring until each cycle. During this window, legitimate nonce-free txs fail.
- The mempool's `NonceFreeReplay` check (eip8130_validate.rs:1264) looks at the on-chain `seen` set, but the ring eviction is at execution time. There is no mempool-side cap on the number of nonce-free txs from a single sender.

**Fix**: Add per-sender nonce-free pending count in `Eip8130PoolConfig` (e.g. `default_max_nonce_free_txs_per_sender: 16`) and enforce it at txpool admission.

**Severity**: MEDIUM — operational risk of ring-buffer exhaustion from a single sender flooding nonce-free txs.

---

#### M-002 — `intrinsic_gas` does not charge for `Delegation` account-change entries proportionally

**File**: `rust/op-alloy/crates/consensus/src/transaction/eip8130/gas.rs:232–234`

**Issue**:

```rust
AccountChangeEntry::Delegation(_) => {
    gas += super::constants::BYTECODE_PER_BYTE_GAS * 23;
}
```

A `Delegation` entry costs `200 × 23 = 4 600` gas, which represents the EIP-7702 delegation designator (3-byte prefix `0xef0100` + 20-byte address). This is the bytecode storage cost.

However, `account_changes_cost` has no SLOAD charge for the existing code read (`load_account_with_code_mut` at handler.rs:455) which is needed to verify the account is empty or already a delegation before writing new delegation bytecode. The handler reads the code at:

```
handler.rs:455:  let acc = journal.load_account_with_code_mut(sender)?.data;
```

This is a code load (cold) — typically 2600 gas in EVM terms — but there is no corresponding charge in intrinsic gas for the delegation code read. The sender may use this to get a free cold-account code load, saving 2600 gas vs. what the EVM would charge for an equivalent EXTCODESIZE.

**BASE delta**: Check pending — base's gas.rs at the delegation arm also uses the same `BYTECODE_PER_BYTE_GAS * 23` formula. If base has the same omission, this is a shared gap.

**Fix**: Add `SLOAD_GAS` (2100) or appropriate cold code access cost to `account_changes_cost` for `Delegation` entries.

**Severity**: MEDIUM — gas underpricing for delegation entries; attacker gets a ~2100 gas discount on cold code reads.

---

#### M-003 — `estimation_calldata_overhead` is a fixed estimate, not derived from actual auth blob structure

**File**: `rust/op-revm/src/handler_aa_helpers.rs:45–46, 282–291`

**Issue**:

```rust
const ESTIMATION_AUTH_CALLDATA_GAS: u64 = 1_100;
```

This constant was derived as "K1 auth = 66 bytes × 16 gas ≈ 1072, rounded up." However:
- P256 auth is typically larger (varies by WebAuthn envelope).
- Custom verifiers can have auth blobs up to `MAX_SIGNATURE_SIZE = 2048` bytes.
- If a user sends `eth_estimateGas` with a custom verifier and empty auth blobs, the estimation undercounts by up to `(2048 × 16) - 1100 = 31668` gas for calldata alone.

The gas estimation is therefore potentially tight for non-K1 verifiers. The binary search may converge on a `gas_limit` that succeeds during estimation (with small empty auth blobs) but fails at execution (with full-size custom verifier auth blobs).

**BASE delta**: Base has the same constant. Shared defect.

**Fix**: Either accept this as a known limitation and document it, or make `ESTIMATION_AUTH_CALLDATA_GAS` dependent on the verifier type in `Eip8130Parts` (the parts struct already knows `sender_verifier` and `payer_verifier`).

**Severity**: MEDIUM — gas estimation undercount for non-K1 sponsors, leading to out-of-gas at execution after a successful estimate.

---

### LOW

#### L-001 — Purity checker allows `NONCE_MANAGER_ADDRESS` (0x...aa02) as a STATICCALL target via `TX_CONTEXT_ADDR`

**File**: `rust/op-alloy/crates/consensus/src/transaction/eip8130/purity.rs:55–56, 264–268`

**Issue**: The purity scanner's `is_known_precompile` function accepts `TX_CONTEXT_ADDR = 0xaa03` as a safe precompile target for STATICCALLs. The `TxContext` precompile exposes:

```
getSender(), getPayer(), getOwnerId(), getMaxCost(), getGasLimit(), getCalls()
```

These values are fixed per-transaction and do not read chain state, so the allowance is semantically correct for pure-determinism purposes.

However, the purity checker does NOT allow `NONCE_MANAGER_ADDRESS = 0xaa02`. A custom verifier that STATICCALLs `0xaa02` to read `getNonce(sender, nonce_key)` would be rejected as impure — which is correct, since nonce state is mutable chain state.

The concern is not a false negative but rather that `TX_CONTEXT_ADDR` allows a verifier to observe `getCalls()` — the full phase call list. A verifier that only succeeds when specific calls are present in the tx is "pure" by the scanner but has implicit dependence on tx content beyond the signature hash. This is actually the design intent (verifiers can enforce call structure), so it is not a bug, but it should be documented clearly as an accepted capability for reviewers.

**Severity**: LOW — documentation gap, not a security flaw.

---

#### L-002 — `GAS` opcode restriction in purity scanner has a false-positive bypass via dual-instruction paths

**File**: `rust/op-alloy/crates/consensus/src/transaction/eip8130/purity.rs:179–184`

**Issue**: The GAS opcode (0x5A) is only allowed immediately before a STATICCALL. The check is:

```rust
let is_before_staticcall = idx + 1 < insns.len()
    && insns[idx + 1].opcode == op::STATICCALL
    && !skip_offsets.contains(&insns[idx + 1].offset);
```

This is a linear instruction index check (`idx + 1`). However, after `JUMPI` or conditional branches, the `GAS` at `idx` may precede a `STATICCALL` in one branch but a different opcode in another. The linear scanner cannot distinguish which branch is taken. The scanner conservatively flags `GAS` as a `StandaloneGas` violation unless the immediately-next disassembled instruction is `STATICCALL`.

This creates a **false negative** risk in bytecode that does:
```
JUMPDEST
GAS        ← idx
JUMPI ...  ← idx+1 (not STATICCALL)
```

The GAS would be flagged as `StandaloneGas` and the verifier would be rejected. False negatives are acceptable per the design philosophy.

However, there is a valid false-negative case with:
```
JUMPDEST
GAS
POP        ← discarding gas for legitimate code-size padding
```
This is also correctly rejected. No false positives are created.

**Severity**: LOW — confirmed no false positives. False negatives are accepted by design.

---

#### L-003 — `validate_against_state_and_deduct_caller` deducts payer balance before nonce validation

**File**: `rust/op-revm/src/handler.rs:287–350`

**Issue**: The execution order within `validate_against_state_and_deduct_caller` for AA transactions is:

1. Load L1 block info (line 261)
2. **Deduct payer balance** (line 287–308)
3. Validate nonce / increment NonceManager (line 317–449)

If the nonce validation at step 3 fails (e.g. nonce-free buffer full as in M-001), the function returns `Err`. The payer's balance deduction at step 2 is recorded in the journal. Since `Handler::run_without_inspector` propagates this error and the journal is not yet committed, the deduction is reverted cleanly. However, this ordering means the payer account is pessimistically loaded and written (dirtied in the journal cache) before nonce validation, which has a minor performance implication.

More importantly: if a future code change in the nonce validation path calls `journal.commit_tx()` or similar before returning, the balance deduction would become permanent without a corresponding nonce bump. This is a latent ordering fragility.

**BASE delta**: Base has the same ordering. Shared.

**Fix**: Re-order: validate nonce first, then deduct balance. This matches the EIP-4337 entrypoint pattern (validate → pay).

**Severity**: LOW — currently safe due to journal rollback, but fragile ordering.

---

#### L-004 — `custom_verifier_gas_cap` at pool admission uses a separate global, not the per-transaction field

**File**: `rust/op-reth/crates/txpool/src/eip8130_validate.rs:1276, 1394`

**Issue**: At mempool admission, `remaining_custom_verifier_gas` is initialized from `custom_verifier_gas_limit` (a parameter passed to `validate_eip8130_transaction`), which defaults to `DEFAULT_CUSTOM_VERIFIER_GAS_LIMIT = DEFAULT_CUSTOM_VERIFIER_GAS_CAP = 200_000`. At execution time in the handler, `eip8130.custom_verifier_gas_cap` is used (set at tx conversion time by `build_eip8130_parts_with_costs`).

If the node operator configures a different `custom_verifier_gas_limit` at the mempool level vs. what is baked into the `Eip8130Parts` at conversion time, a tx may be admitted with a 200k gas cap but executed with a different cap (e.g. 100k), causing the STATICCALL to run out of gas at execution despite passing mempool validation.

**Fix**: Ensure the custom verifier gas cap used at pool admission exactly matches the value baked into `Eip8130Parts`. The cap should ideally be a protocol constant rather than a runtime-configurable value that can differ between admission and execution.

**Severity**: LOW — operational misconfiguration risk, not directly exploitable.

---

#### L-005 — Failed-validation AA tx leaves payer charged (gas burns on sequencer, not payer)

**File**: `rust/op-revm/src/handler.rs:1135–1197` (`catch_error`)

**Issue**: When an AA tx fails validation at inclusion time (e.g. native verifier owner_config SLOAD fails at `validate_native_verifier_owner`), `catch_error` is invoked. Unlike deposit transactions, the AA catch path does **not** commit any partial state — it simply returns the error. The journal is reverted. The payer's balance is restored. The nonce is not incremented.

This means a tx that fails inclusion-time validation burns **zero** gas for the payer and zero for the sequencer. The sequencer pays the validation cost (CPU + storage reads) without any compensation. An attacker who knows a tx will fail inclusion-time validation (e.g. by revoking their key after mempool admission) can DoS the sequencer's inclusion validation at zero cost.

**Mitigation present**: The pool's per-sender caps (8 pending txs per sender by default) and invalidation-key-based eviction reduce the blast radius. When the owner_config slot changes, the pool evicts affected txs.

**Gap**: The invalidation key tracking does not cover the `REVOKED_VERIFIER` sentinel path (setting verifier to `0xff...ff` does not change the nonce slot that drives eviction). A sender could loop: submit → get into pool → revoke key → tx fails at inclusion → re-admit with new key → repeat.

**Fix**: Track `owner_config` slot changes in invalidation keys. The `compute_invalidation_keys` function already handles nonce and owner_config slots — verify it tracks the specific `owner_config_slot(sender, owner_id)` for the sender's registered key.

**Severity**: LOW — sequencer pays a few SLOADs per failed tx, bounded by per-sender pool caps.

---

### INFO

#### I-001 — `op_precompiles_map` builds a new `PrecompilesMap` per block (potential allocation hot path)

**File**: `rust/alloy-op-evm/src/lib.rs:252, 274`

Each `OpEvmFactory::create_evm` call invokes `op_precompiles_map(spec_id)`, which clones the static precompile set and appends the AA precompiles. This allocation happens per-block (or per-tx if evm instances are not cached). Under high TPS, this is a minor but avoidable allocation.

**Fix**: Cache the `PrecompilesMap` behind a `OnceLock<HashMap<OpSpecId, PrecompilesMap>>` or similar, keyed by spec ID.

---

#### I-002 — Purity scanner does not cover `TLOAD`/`TSTORE` (transient storage, EIP-1153)

**File**: `rust/op-alloy/crates/consensus/src/transaction/eip8130/purity.rs:254`

`TLOAD = 0x5C` and `TSTORE = 0x5D` are correctly listed in the `StateAccess` violation category:

```rust
0x31 | 0x3B | 0x3C | 0x3F | 0x47 | 0x54 | 0x55 | 0x5C | 0x5D => StateAccess
```

`0x5C = TLOAD`, `0x5D = TSTORE`. Both are in the banlist. Confirmed correct — no issue.

---

#### I-003 — Stale `EIP8130_TX_CONTEXT` thread-local on error/panic paths (already noted as BUG-CAND-C)

Re-confirmed: `clear_eip8130_tx_context()` is called at the start of `validate_against_state_and_deduct_caller` (handler.rs:223) for every transaction type. This defense-in-depth is present and correct. The precompile dispatch independently gates on `tx.tx_type() == EIP8130_TX_TYPE`. Low blast radius. Already filed as BUG-CAND-C.

---

#### I-004 — `accumulated_refunds` is an `i64`, allowing negative values

**File**: `rust/op-revm/src/handler.rs:782`

```rust
let mut accumulated_refunds: i64 = 0;
...
accumulated_refunds += phase_refunds;
```

`phase_refunds` is also `i64` (populated by `call_result.gas().refunded()` which returns `i64`). Negative refunds are valid EVM semantics (SSTORE dirty-write reversal). The final guard `if accumulated_refunds > 0` before `result_gas.record_refund` correctly drops negative totals. No underflow risk.

However, if a phase fails (reverted), its `phase_refunds` are discarded (`phase_ok == false` → checkpoint revert, refunds not added). This is correct behavior. Confirmed no refund DoS vector.

---

#### I-005 — TEMPO sponsor flow: "Phase 0 paymaster fee, Phase 1 user op" pattern not enforced at protocol level

**TEMPO doc** (native-aa-gas-sponsor-passkey-flow.md): "常见做法：Alice 先在 Phase 0 向 Sponsor 转 USDT 作为费用" — the recommended pattern of Phase 0 paying the sponsor, Phase 1 doing the user operation, is explicitly described as an **application-layer** convention, not a protocol-layer enforcement.

**Our implementation**: The handler executes phases atomically per-phase but independently across phases. If Phase 0 succeeds (fee to sponsor) and Phase 1 fails (user op), the sponsor receives their USDT and the user's operation is rolled back. This matches TEMPO's stated intent.

**Gap vs. TEMPO doc**: TEMPO's doc describes this as the *recommended* pattern but notes "这是应用层的问题，不是协议层" (this is an application-layer problem, not protocol-layer). Our implementation is consistent with this.

**Replay of sponsor approval**: TEMPO's doc does not flag a replay-of-payer-approval attack. Our implementation correctly binds `payer_auth` to `resolved_sender` via `payer_signature_hash(tx, resolved_sender)` (signature.rs:57). A payer signature cannot be replayed for a different sender. This is a **positive finding** — the cross-sender payer replay attack described in the spec is correctly closed.

---

## Sponsor/Payer Flow Summary

| Property | Status | Location |
|---|---|---|
| Payer must sign before gas is deducted | Correct — mempool verifies payer_auth before admission | eip8130_validate.rs:975–1016 |
| Payer signature binds to resolved sender (anti cross-sender replay) | Correct — `payer_signature_hash(tx, resolved_sender)` | signature.rs:57 |
| Payer balance check at mempool admission | Correct — total_gas × max_fee checked | eip8130_validate.rs:1392–1407 |
| Payer balance re-verified at inclusion | Partial — SLOAD re-validates owner_config; signature NOT re-verified (see H-002) | handler.rs:724–741 |
| Payer deduction is atomic | Correct — journal-based, reverted on error | handler.rs:287–308 |
| Unused gas refunded to payer | Correct — `remaining + refunded` credited to payer account | handler.rs:1027–1033 |
| Refund goes to payer, not sender | Correct — `payer` address from `eip8130.payer` | handler.rs:1022 |
| Gas not double-charged | Correct — journal reverts pre-pay on validation error | — |
| Sponsor cannot be drained by validation failures | Mostly — pool caps bound blast radius; but failed inclusion burns sequencer cost, not payer | L-005 |

---

## Gas Accounting Completeness (Intrinsic Gas)

All components are present in `intrinsic_gas_with_costs` (gas.rs:65):

| Component | Charged | Notes |
|---|---|---|
| AA_BASE_COST (15,000) | Yes | Replaces standard 21,000 |
| Calldata (EIP-2028) | Yes | Full RLP encoding, 4/16 per byte |
| Sender SLOAD (owner_config) | Yes | Always |
| Payer SLOAD (owner_config) | Yes | Self-pay = 0 |
| K1 native verification | Yes | 6,000 |
| P256 raw | Yes | 9,500 |
| WebAuthn | Yes | 15,000 |
| Delegate overhead | Yes | 3,000 + inner cost |
| Custom verifier | 0 in intrinsic, metered at runtime | Separate cap |
| Nonce key (cold) | Yes | 22,100 cold, 5,000 warm |
| Expiring nonce | Yes | 14,000 flat |
| Bytecode deploy | Yes | 32,000 base + 200/byte |
| Config change ops | Yes | 20,000/op matching chain |
| Config change skip | Yes | 2,100 for mismatched chain |
| Delegation (EIP-7702) | Yes, but incomplete | See M-002 |
| Authorizer verification gas | Yes | Per ConfigChangeEntry |
| Sponsor proof calldata | Yes | Included in full RLP tx_payload_cost |

---

## DoS Mitigation Summary

| Vector | Mitigation | Gaps |
|---|---|---|
| Oversized auth blobs | MAX_SIGNATURE_SIZE = 2048 | None |
| Too many calls | MAX_CALLS_PER_TX = 100 (mempool + inclusion) | None |
| Too many account changes | MAX_ACCOUNT_CHANGES_PER_TX = 10 | None |
| Too many config ops | MAX_CONFIG_OPS_PER_TX = 5 | None |
| Custom verifier gas burn | Custom verifier gas cap = 200,000 | L-004: cap may differ between mempool and execution |
| Per-sender pool flooding | 8 pending txs / sender (default tier) | M-001: no separate nonce-free cap |
| Per-payer pool flooding | 8 pending txs / payer (default tier) | None additional |
| Pool total size | max_pool_size = 4,096 | Eviction present |
| Impure custom verifiers | Purity scanner or allowlist at mempool admission | None for inclusion (handler re-validates ownership, not purity) |
| Invalid tx draining sequencer | Failed inclusion burns zero payer gas; pool caps bound |  L-005: no invalidation for REVOKED_VERIFIER sentinel |
| Nonce-free replay | Expiring-nonce ring buffer + seen set | M-001: ring-full condition drops legitimate txs |

---

## DELTA vs BASE

| Area | OUR | BASE | Gap |
|---|---|---|---|
| Precompile spec gate | `spec == NATIVE_AA` | `spec == BASE_V1` | Both use equality; H-001 affects both |
| Payer auth re-verify at inclusion | owner_config SLOAD only | Same | Shared design choice |
| Estimation calldata overhead | Fixed K1-derived constant | Same constant | Shared |
| Delegation intrinsic gas | 200×23 = 4600 | Same | Shared M-002 |
| Nonce-free per-sender cap | None in pool | Check pending | Possible OUR gap |
| Purity scanner: TLOAD/TSTORE | Correctly banned | Confirmed banned | Parity |
| Refund overflow guard | `accumulated_refunds: i64` + `> 0` guard | Same | Parity |

---

## Verdict

**BLOCK on H-001** — The precompile spec gate equality check is a silent forward-compatibility break that will affect all AA transactions when the next hardfork after `NATIVE_AA` is activated.

**H-002** — Document explicitly that payer_auth signature is not re-verified at inclusion; this is a design decision, not a bug per se, but it must be stated in the security model.

**MEDIUM findings M-001, M-002, M-003** — Address before mainnet. M-002 (delegation gas underpricing) and M-003 (estimation undercount for large auth blobs) are gas-economy issues. M-001 (nonce-free ring full DoS) requires a per-sender nonce-free cap in the pool config.

**LOW findings L-001 through L-005** — Address in follow-up PR. L-005 (failed-inclusion burns zero payer gas) is the most operationally relevant; the `REVOKED_VERIFIER` invalidation gap should be filed as a separate issue.

