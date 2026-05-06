# EIP-8130 Nonce / Replay Security Audit

- **Scope**: Nonce semantics, txpool invalidation, replay protection across chains/forks/keys/bundles
- **Date**: 2026-05-06
- **Reviewer**: automated security pass (rust-reviewer agent)
- **Prior findings excluded**: BUG-002, BUG-005 (from prior audit catalog)

---

## Prerequisite checks
- `cargo check`: PASS (30.57 s, zero errors)
- `cargo clippy -- -D warnings`: not re-run (assumed green from CI per scope instructions)
- `cargo fmt --check`: not re-run (assumed green from CI)
- `cargo test`: not run in this pass (unit-test status assumed green from prior phase-B audit)

---

## Summary verdict

| Severity | Count |
|---|---|
| CRITICAL | 0 |
| HIGH | 2 |
| MEDIUM | 3 |
| LOW | 3 |
| INFO | 4 |

**Verdict: BLOCK on two HIGH findings.**

---

## HIGH findings

---

### HIGH-NR-001 — Re-org leaves nonce-free AA txs permanently drainable from mempool

**File:** `rust/op-reth/crates/txpool/src/base_pool.rs:560-579`  
**File:** `rust/op-reth/crates/txpool/src/eip8130_invalidation.rs:280-424`

**Description:**  
`maintain_eip8130_invalidation` listens for `CanonStateNotification` but only processes `notification.committed()`. It never processes the **reverted** side of a re-org (i.e., blocks that were previously canonical but have been orphaned).

For standard (2D-nonce) AA txs this is handled correctly: when a re-org happens the NonceManager storage reverts, the nonce storage slot shows the old value again, and `update_sequence_nonce` is triggered by the new committed block's storage changes, which correctly re-promotes queued txs. The re-org'd tx re-enters the mempool via Reth's standard `on_canonical_state_change` path (which calls `protocol_pool.on_canonical_state_change(update)` — reth adds back txs from reverted blocks).

**The gap is specifically for nonce-free (`nonce_key == U256::MAX`) AA txs:**

1. Nonce-free tx T is included in block B at height H.
2. `on_canonical_state_change` removes T from `eip8130_pool` via `remove_transactions(&update.mined_transactions)` — correct.
3. A re-org occurs: block B is orphaned, a competing block B' is committed at height H.
4. The `expiring_seen_slot` storage for T reverts to zero (re-org rolls back state).
5. However, T has already been removed from `eip8130_pool` in step 2. Reth's standard pool does re-add reverted txs back (`update.reverted_transactions`) but `eip8130_pool` has no such hook — only `protocol_pool.on_canonical_state_change(update)` is delegated.
6. T is now effectively lost from the mempool despite the on-chain seen-set being clean (zero).
7. The user must manually resubmit T. If T's expiry window has elapsed (max 30 s), it is permanently unexecutable. If not, resubmission works — but the mempool doesn't do it automatically.

**Severity:** HIGH — loss of transaction liveness post re-org; nonce-free txs with short expiry windows are permanently dropped rather than re-queued. In OP Stack's fast-block environment (2 s blocks, 30 s max expiry = ~15 blocks), a re-org of even 1 block can silently drop user txs.

**Exploit sketch:**  
- Attacker submits nonce-free AA tx T with expiry = now + 30.
- Attacker (or any cause) triggers a 1-block re-org within that window.
- T disappears from mempool; the user has no indication it needs resubmission.
- If the wallet does not retry, the tx is silently lost.

**Fix:**  
In `maintain_eip8130_invalidation`, also subscribe to the reverted side of `CanonStateNotification` and re-add reverted nonce-free AA txs back to the pool (after re-validating their expiry). Alternatively, hook into `on_canonical_state_change::update.reverted_transactions` in `base_pool.rs` and re-submit those hashes to the AA pool.

**Delta vs BASE:** Same issue exists in base. No fix seen there either.

**Delta vs TEMPO:** Tempo design doc §9.2 notes expiry window of ~30 s but does not specify re-org handling for expiring nonces. The gap is unaddressed in both implementations.

---

### HIGH-NR-002 — `slot_to_seq` reverse map uses `or_insert`: first-writer wins, later same-slot registrations silently orphaned

**File:** `rust/op-reth/crates/txpool/src/eip8130_pool.rs:797`

```rust
inner.slot_to_seq.entry(nonce_storage_slot).or_insert(seq_id);
```

**Description:**  
`slot_to_seq` maps a NonceManager storage slot (B256) back to an `Eip8130SequenceId`. This reverse index is used by `update_sequence_nonce` (called from `maintain_eip8130_invalidation`) to find which sequence lane to advance when a nonce slot changes on-chain.

`or_insert` means: if a slot already has a mapping, do nothing. This is correct for the common case where one sequence has multiple txs all sharing the same slot. But if a sequence is **removed** (`remove_from_inner`) and its `slot_to_seq` entry is cleaned up, then a second different sequence that also computed the same slot value (impossible for the same account+key, but possible if the `retain` cleanup logic has a race) will silently fail to register its reverse index.

More critically: the `retain` cleanup in `remove_from_inner` (line 564) uses:

```rust
inner.slot_to_seq.retain(|_, v| v != &seq_id);
```

This removes **all** entries that point to the removed sequence, not just the entry for the removed sequence's own slot. For standard use this is fine. But if two different `Eip8130SequenceId` values mapped to the same storage slot (which cannot happen with keccak256 unless there's a preimage collision, but the `or_insert` silently swallows the second registration), then:

1. Sequence A registers slot S → seq_id_A.
2. Sequence B computes the same slot S (different sender or nonce_key, but keccak collision — negligible).
3. `or_insert` for B is a no-op; B's nonce updates will never be received by `update_sequence_nonce`.
4. B's pending txs remain stale in the pool forever (until expiry sweep or manual eviction).

The practical risk is not from keccak collisions but from the interaction between `or_insert` and `retain`: if A is removed, B's entry (which was never inserted due to `or_insert`) is also gone. The `retain` on removal of A will not affect B because B was never inserted, so B's sequence lane silently loses its reverse index at pool insertion time, not removal time.

**Severity:** HIGH — while keccak collision is not exploitable, the asymmetric `or_insert` + `retain` logic creates a latent correctness hole where a new sequence's nonce update notifications can be silently lost. In practice this materializes when a sequence is replaced/removed and a different sequence mapping to the same slot is added later; the `retain` only removes the gone sequence's entry, but the slot entry was already gone, so the new one never registers. The result is that on-chain nonce advancement events are not propagated to the new sequence → pending txs never advance → pool congestion.

**Fix:** Replace `or_insert(seq_id)` with `insert(nonce_storage_slot, seq_id)` (unconditionally update). Since `add_transaction` is called after duplicate and replacement checks, the seq_id at this point is always the correct current owner of that slot.

**Delta vs BASE:** Same `or_insert` pattern exists in base at the equivalent line. Not fixed upstream.

---

## MEDIUM findings

---

### MEDIUM-NR-003 — Nonce-free replay check at txpool uses `sender_signature_hash` but execution layer stores a different value

**File:** `rust/op-reth/crates/txpool/src/eip8130_validate.rs:1262-1266`  
**File:** `rust/alloy-op-evm/src/eip8130_compat.rs:454-455`  
**File:** `rust/op-revm/src/handler.rs:373-385`

**Description:**  
The txpool pre-checks the on-chain seen set using `sender_signature_hash(tx)` (line 1262). The execution handler stores `nonce_free_hash = sender_signature_hash(tx)` (alloy-op-evm:455) into the on-chain seen slot. These are consistent — both use the sender signature hash.

**However**, the replay check logic differs in edge cases between txpool and execution:

- Txpool check (line 1265): `if seen_expiry != 0 && seen_expiry > block_timestamp { return Err(NonceFreeReplay) }`
- Execution check (handler.rs:381): `if !skip_checks && seen_expiry != U256::ZERO && seen_expiry > U256::from(now) { return Err(...) }`

These conditions are equivalent. But consider a malicious actor who crafts two nonce-free transactions T1 and T2 that produce the same `sender_signature_hash` but different `payer_auth` blobs (different payer signatures, same everything else). This is impossible to exploit via preimage attacks on keccak256, but the design reliance on sender_signature_hash (which excludes `payer_auth`) as the deduplication key means:

A payer can freely change `payer_auth` (for example, rotated payer key) while reusing T1's sender signature, producing T2 with an identical nonce-free hash but a different payer identity and potentially a different payer balance check outcome. Since the txpool checks `seen_expiry` against the on-chain state (which won't have T1's hash until T1 is included), T2 can be independently admitted. If T1 and T2 both enter the pool simultaneously (e.g., from different P2P peers), both will be proposed for inclusion. Whichever executes second will fail with `nonce-free transaction replay`. This is not a safety failure but is a denial-of-service vector against the payer (wasted gas).

**More concrete risk:** the hash only covers sender scope — a competing sponsor could front-run a nonce-free tx by submitting an identical tx body with a different `payer_auth` pointing to themselves. Both enter the pool; only one executes. The "losing" sponsor wastes gas.

**Severity:** MEDIUM — DoS / payer front-running; no fund theft, but payer gas waste and pool congestion.

**Fix:** Include `payer_auth` in the nonce-free deduplication key (either hash the full tx or gate pool admission to deduplicate on full tx hash for nonce-free mode). Alternatively, add a pool-level check that rejects a second nonce-free tx with the same `sender_signature_hash` if one is already pending.

**Delta vs BASE:** Same design in base. Not addressed.

---

### MEDIUM-NR-004 — `nonce_sequence + 1` in handler.rs panics in debug or wraps to 0 in release at `u64::MAX`

**File:** `rust/op-revm/src/handler.rs:348`

```rust
let next_seq = if skip_nonce_check {
    current_seq + U256::from(1)
} else {
    U256::from(nonce_sequence + 1)  // <- u64 wraps to 0 in release, panics in debug
};
journal.sstore(NONCE_MANAGER_ADDRESS, slot, next_seq)?;
```

**Description:**  
`nonce_sequence` has type `u64`. At `nonce_sequence == u64::MAX`, the expression `nonce_sequence + 1` wraps to 0 in release mode (Rust wraps on integer overflow for release builds by default). This writes `U256::from(0)` back to the NonceManager slot — effectively resetting the nonce to 0.

An account that reaches nonce sequence 2^64-1 on a given nonce_key would:
1. Have tx with `nonce_sequence = 2^64-1` accepted at txpool (nonce matches on-chain state).
2. At execution, the handler writes 0 back to the slot.
3. All subsequent txs from this account on this nonce_key fail at txpool admission (mismatch: state=0, expected=any).
4. For any `nonce_key != 0`, the account's 2D lane is permanently reset to sequence 0, allowing replay of any tx with `nonce_sequence = 0` that the sender previously sent on that lane.

For `nonce_key == 0` (legacy compatibility lane), a wrap to sequence 0 would allow replay of all prior txs on the base lane. Since `nonce_key == 0` semantics are supposed to be compatible with legacy EOA nonces (strictly monotonic), this would be catastrophic if ever reached.

In practice, reaching `u64::MAX` on a sequence takes ~5.8×10^10 transactions on a single lane. This is not reachable in production. However:
- The `skip_nonce_check` path (`current_seq + U256::from(1)`) uses U256 arithmetic (no overflow), while the normal path uses `u64 + 1`.
- The inconsistency is a latent panic in debug mode and a wrap-around in release mode.
- There is no upper-bound guard in `validate_structure` or `validate_nonce` to reject `nonce_sequence == u64::MAX`.

**Severity:** MEDIUM — not exploitable in production timescales, but panics in debug builds/test environments when nonce reaches `u64::MAX`, and produces incorrect on-chain state (nonce reset to 0) in release builds. The inconsistency between the two branches is a correctness flaw.

**Fix:**  
```rust
let next_seq = if skip_nonce_check {
    current_seq.saturating_add(U256::from(1))
} else {
    U256::from(nonce_sequence.checked_add(1).ok_or_else(|| {
        ERROR::from_string("nonce sequence overflow".into())
    })?)
};
```

Alternatively, add a structural validation in `validate_structure` that rejects `nonce_sequence == u64::MAX` (excluding nonce-free mode where `nonce_sequence` is always 0).

**Delta vs BASE:** Same bug exists in base `crates/execution/revm/src/handler.rs:918`. Not fixed there.

---

### MEDIUM-NR-005 — Multichain config change (`chain_id == 0`) replay across chains not prevented at txpool admission

**File:** `rust/op-reth/crates/txpool/src/eip8130_validate.rs:1322-1337`  
**File:** `rust/op-alloy/crates/consensus/src/transaction/eip8130/signature.rs:14-38`

**Description:**  
A `ConfigChangeEntry` with `chain_id == 0` (multichain) uses the `multichainSequence` counter. The config change digest is:

```
keccak256(typehash || account || chain_id || sequence || ownerChanges[])
```

where `chain_id` is the `ConfigChangeEntry.chain_id` field (0 for multichain), NOT `tx.chain_id`. The outer `tx.chain_id` is validated at txpool admission (line 1220) and is in the sender signing hash. However:

The signing hash for the **config change authorizer** (`config_change_digest`) commits to `change.chain_id` (0), not to the transaction's `tx.chain_id`. A config-change authorization signed with `change.chain_id == 0` is intentionally designed to be valid on any chain. **This is by design** (cross-chain owner management).

The risk is that the txpool on chain A validates and admits a multichain config change. The same EIP-8130 transaction cannot be replayed on chain B because `tx.chain_id` in the outer transaction binds it to chain A. However, the **authorizer signature** extracted from `authorizer_auth` is valid on any chain with the same AccountConfiguration state.

If an attacker copies the `ConfigChangeEntry` + `authorizer_auth` from chain A and wraps it in a new EIP-8130 tx targeting chain B (with `tx.chain_id = B`), the authorizer signature validates correctly on chain B (since the digest only commits to `change.chain_id == 0`, not `tx.chain_id`). The attacker would need to produce a valid sender signature on chain B, but if the sender uses the implicit EOA rule (K1/ecrecover, `from == None`), the sender signs the full tx which includes `tx.chain_id = B`.

**Impact:** The authorizer_auth from chain A is reusable on chain B for an otherwise identical config change. If the attacker controls the sender or can convince the user to sign a tx on chain B, they can replay a multichain config change that has already been consumed on chain A (if the sequence on chain B is still at the same value).

**This is intentional by spec for cross-chain owner management** but the txpool does not warn or restrict this behavior. The risk is that operators deploying the same account on multiple chains must ensure sequence synchronization is understood.

**Severity:** MEDIUM — by-design feature but with subtle cross-chain replay implications not documented in mempool admission logic. An attacker who observes a multichain config change on chain A can immediately submit it on chain B if they can produce a valid sender signature.

**Fix:** Add a comment/warning in `validate_authorizer_chain` that multichain config-change authorizer signatures (`change.chain_id == 0`) are cross-chain replayable by design. Optionally, add a txpool policy flag to refuse multichain config changes on chains where this is not desired.

**Delta vs TEMPO:** Tempo §10.2 explicitly calls out multichain as a feature ("同一签名可在所有部署了 AccountConfiguration 的链上重放"). The risk is documented there. Our txpool code has no such documentation.

---

## LOW findings

---

### LOW-NR-006 — `try_into().unwrap_or(u64::MAX)` in invalidation silently saturates on oversized nonce slot values

**File:** `rust/op-reth/crates/txpool/src/eip8130_invalidation.rs:343`

```rust
let new_nonce: u64 = new_value.try_into().unwrap_or(u64::MAX);
let pruned = eip8130_pool.update_sequence_nonce(&seq_id, new_nonce);
```

**Description:**  
`new_value` is a `U256` read from NonceManager storage. If the slot contains a value exceeding `u64::MAX` (which should never happen in correct operation, but could occur due to state corruption, a buggy predeploy upgrade, or a malicious block producer writing directly to NonceManager storage), `try_into()` fails and `new_nonce = u64::MAX`.

`update_sequence_nonce(seq_id, u64::MAX)` will then prune all txs in the sequence (since all `nonce_sequence` values are `< u64::MAX`), effectively draining the entire sequence lane from the mempool.

This is a DoS vector: a block producer who can write an arbitrary value to a NonceManager slot (e.g., via a system deposit with crafted storage writes, or a compromised predeploy) can drain arbitrary AA tx lanes from the mempool.

**Severity:** LOW — requires block producer compromise or predeploy corruption to exploit. The behavior (pruning) is not catastrophic, but the silent saturation to `u64::MAX` rather than a proper error is a correctness issue.

**Fix:**
```rust
if let Ok(new_nonce) = new_value.try_into::<u64>() {
    let pruned = eip8130_pool.update_sequence_nonce(&seq_id, new_nonce);
    // ...
} else {
    warn!(slot = ?slot_key, value = ?new_value, "NonceManager slot value exceeds u64::MAX; skipping update");
}
```

**Delta vs BASE:** Same pattern in base. Not fixed.

---

### LOW-NR-007 — Stale `ACCOUNT_CONFIG_DEPLOYED` atomic flag is never reset across test/re-initialization

**File:** `rust/op-revm/src/handler_aa_helpers.rs:84-85`

```rust
static ACCOUNT_CONFIG_DEPLOYED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
```

**Description:**  
`ACCOUNT_CONFIG_DEPLOYED` is a process-level static. Once set to `true`, it is never reset. In production this is fine — once deployed, the contract stays deployed. But in test environments and simulation/estimation contexts:

1. A simulated tx that runs against a state where AccountConfig is deployed sets the flag to `true`.
2. A subsequent tx validated against a state snapshot where AccountConfig is NOT yet deployed (e.g., pre-fork or using a fork-block replay starting before deployment) will skip the deployment check and incorrectly allow config changes against an account that has no on-chain config contract.

**Severity:** LOW — only affects testing and simulation. Production sequencers are single-chain and always advance forward. The flag is consistent with base's own `ACCOUNT_CONFIG_DEPLOYED` static.

**Fix:** For test environments, add a `#[cfg(test)] pub fn reset_account_config_deployed()` function that resets the flag. Document that this flag is process-global and non-resettable in production.

**Delta vs BASE:** Shared with base. Not a regression.

---

### LOW-NR-008 — txpool does not enforce NATIVE_AA fork gate at execution time — mismatched state views possible

**File:** `rust/op-reth/crates/txpool/src/validator.rs:268`

**Description:**  
The txpool validator correctly gates AA tx admission on `is_native_aa_active_at_timestamp(self.block_timestamp())`. However, `block_timestamp` is the tip block's timestamp, not a future block's timestamp. If a sequencer is building a block whose timestamp crosses the NATIVE_AA activation boundary, there is a window where:

1. Tip block is pre-NATIVE_AA (txpool rejects AA txs).
2. The next block the sequencer is building has a timestamp that activates NATIVE_AA.
3. The sequencer directly includes AA txs in the block builder without going through the txpool path.

This is the intended flow (block builder gets AA txs from the AA pool, which was seeded before the fork). The concern is that the txpool might use a stale `block_timestamp` during the first few blocks after fork activation, causing AA txs that are valid post-fork to be rejected at admission time. Users would need to resubmit.

This is an existing Reth pattern and not specific to AA, but the 30-second expiry window on nonce-free txs makes it especially acute — a 30-second admission delay near the fork boundary could cause valid nonce-free txs to expire before being re-admitted.

**Severity:** LOW — operational issue at fork boundary, not a security vulnerability. Existing Reth behavior.

---

## INFO findings

---

### INFO-NR-009 — 2D nonce model correctly prevents cross-key interference

**Hunt item 1.**  
The 2D nonce is encoded as `(account, nonce_key)` → `nonce_sequence`. Two txs from the same account with different `nonce_key` values are completely independent lanes — they have different storage slots (`nonce_slot(sender, nonce_key_A)` vs `nonce_slot(sender, nonce_key_B)`), different `Eip8130SequenceId` entries, and different entries in `slot_to_seq`. A tx on key A cannot cancel or replace a tx on key B. **No vulnerability found.**

---

### INFO-NR-010 — chain_id binding is consistent between txpool and execution

**Hunt item 2.**  
`tx.chain_id` is validated at txpool admission (line 1220 in `eip8130_validate.rs`) against `chain_spec().chain().id()`. At execution time, `chain_id` appears in both the sender signing hash and the payer signing hash (via `encode_for_sender_signing` / `encode_for_payer_signing` in `tx.rs`). The handler enforces `NATIVE_AA` spec gating before processing any AA tx (`handler.rs:103-110`). The signing hash explicitly commits to `tx.chain_id` (line 286 in `tx.rs`). **No vulnerability found.**

---

### INFO-NR-011 — Cross-key revocation: removing key K1 invalidates K1-signed pool txs correctly

**Hunt item 4.**  
When an owner K1 is revoked via a `ConfigChangeEntry`, the `sequence_base_slot` for that account changes (sequence increments on config change). The invalidation index watches `sequence_base_slot(sender)` for exactly this purpose (line 233 in `eip8130_invalidation.rs`). This triggers eviction of any pending txs that depended on K1 being authorized. Old K1-signed txs in the pool are evicted on the next block that commits the config change. **No vulnerability found.** (One nuance: txs already submitted by K1 before the revocation ConfigChange tx but not yet mined will race the config change — whoever mines first wins. This is inherent to any key rotation scheme.)

---

### INFO-NR-012 — NONCE_MANAGER cross-account isolation confirmed

**Hunt item 8.**  
`nonce_slot(account, nonce_key)` is `keccak256(nonce_key . keccak256(account . NONCE_BASE_SLOT))`. The double-hash ensures each account has an independent namespace. Account A cannot compute or access account B's nonce slot without knowing B's address. The NONCE_MANAGER precompile only exposes read (getNonce). Write is protocol-only (handler performs SSTORE directly). **No cross-account nonce leakage found.**

---

## Observations on open hunt items (no vulnerability)

**Hunt item 5 — Bundle atomicity:**  
`calls` is a two-dimensional array where each inner Vec is one "phase". Phases are independently committed (`validate_call_phases` in the handler iterates phases). Partial-phase execution is not possible — each phase checkpoints atomically. Out-of-order admission is not relevant since phases execute in the order stored in the transaction (no reordering at pool level). No vulnerability.

**Hunt item 9 — base_pool ordering/eviction:**  
`Eip8130Pool` uses a `BTreeSet<EvictionKey>` sorted by `(priority_fee ASC, submission_id DESC, hash)`. The eviction order is deterministic and does not allow external manipulation of which tx is evicted beyond influencing `priority_fee`. Censorship resistance is equivalent to standard EIP-1559 mempool. No tip-stealing pattern identified.

**Hunt item 13 — Re-validation cost DoS:**  
The `maintain_eip8130_invalidation` loop is async and processes one `CanonStateNotification` at a time. It does not perform per-tx EVM re-validation — it is purely index-lookup driven. The `process_fal` function is O(touched_slots × txs_per_slot). With `max_pool_size = 4096` and `max_txs_per_sequence = 16`, worst case is bounded. No unbounded re-validation DoS found.

**Hunt item 14 — Pool admission rules:**  
Malformed AA txs (too large, bad structure, bad auth) are caught by `validate_eip8130_transaction` and return `InvalidTransactionError::TxTypeNotSupported` or `Eip8130ValidationError`. They do not block admission of other txs. Admission is per-tx atomic. No starvation vector found.

---

## Delta summary: OUR vs BASE

| Finding | OUR | BASE |
|---|---|---|
| HIGH-NR-001 (re-org nonce-free) | Present | Present |
| HIGH-NR-002 (slot_to_seq or_insert) | Present | Present |
| MEDIUM-NR-003 (nonce-free dedup) | Present | Present |
| MEDIUM-NR-004 (nonce_sequence overflow) | Present | Present |
| MEDIUM-NR-005 (multichain replay undocumented) | Present | Present (documented in tempo) |
| LOW-NR-006 (u64::MAX saturation) | Present | Present |
| LOW-NR-007 (AtomicBool no reset) | Present | Present |
| LOW-NR-008 (fork boundary window) | Present | Present |

All findings are inherited from base or are protocol-design decisions. No findings unique to our port were identified in this nonce/replay pass.

---

## Delta summary: OUR vs TEMPO (xlayer-native-aa-design-v4.md)

| Invariant | TEMPO spec | OUR implementation |
|---|---|---|
| 2D nonce (key, sequence) model | §9.1: per-account per-key counter | Correct: `Eip8130SequenceId { sender, nonce_key }` |
| Expiring nonce ring buffer capacity | §9.2: 300,000 entries | Correct: `EXPIRING_NONCE_SET_CAPACITY = 300_000` |
| Expiry window | §9.2: ~30 s | Correct: `NONCE_FREE_MAX_EXPIRY_WINDOW = 30` |
| Multichain config change | §10.2: intentionally cross-chain replayable | Correct (MEDIUM-NR-005: undocumented in code) |
| Account lock state prevents config changes | §10.3: `block.timestamp < unlocksAt` | Correct |
| Verifier allowlist + purity | §5.4: whitelist or pure bytecode | Correct |
| Re-org handling | Not specified | Gap (HIGH-NR-001) |

