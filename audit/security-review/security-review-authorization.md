# EIP-8130 Security Review — Authorization, Ownership Transitions, and Predeploy State Mutations

- **Date**: 2026-05-06
- **Scope**: OUR port vs BASE upstream vs TEMPO design
- **Reviewer**: claude-sonnet-4-6 (rust-reviewer agent)
- **CI gate at review time**: cargo check PASS, cargo test PASS (170 + 81 cases), clippy -D warnings FAILS (hygiene-level doc/style lints only — no logic errors; see CI note below)
- **Prior findings NOT re-reported**: BUG-001..008, BUG-CAND-A..014 from phase-b-summary.md

---

## CI Note

`cargo clippy -p op-revm -- -D warnings` and `cargo clippy -p op-alloy-consensus -- -D warnings` both fail with **style and documentation lints only** (missing backticks in doc comments, `map_or` simplification, `const fn` suggestions, `div_ceil` idiom). Zero logic or safety warnings were emitted. All `cargo check` and `cargo test` passes are clean. The style failures are pre-existing hygiene debt, not findings from this review.

---

## Summary

| ID | Severity | Title | File:Line | Delta |
|---|---|---|---|---|
| AUTH-001 | HIGH | `is_owner_authorized()` returns `true` for `REVOKED_VERIFIER` | accessors.rs:58-65 | OUR-only public API trap |
| AUTH-002 | HIGH | Dual `ACCOUNT_CONFIG_DEPLOYED` atomics diverge between txpool and execution layers | predeploys.rs:50 / handler_aa_helpers.rs:84 | OUR-ONLY (porting artifact) |
| AUTH-003 | HIGH | Pre-execution `pre_writes` applied without runtime authorization check at inclusion time | handler.rs:489-500 | SHARED with BASE |
| AUTH-004 | MEDIUM | `is_estimation` allows config_writes to apply without authorizer-chain validation | handler.rs:744-766 (outside `if !is_estimation`) | SHARED with BASE |
| AUTH-005 | MEDIUM | DELEGATE verifier inner-slot re-reads same slot as outer check — tautological scope validation | handler_aa_helpers.rs:362-374 | SHARED with BASE |
| AUTH-006 | MEDIUM | No minimum-owner guard: last-key self-revocation bricks an account permanently | (all of handler flow) | SHARED with BASE |
| AUTH-007 | LOW | TX_CONTEXT precompile active for ALL txs when NATIVE_AA spec active; non-AA callers receive ZERO values, not a revert | precompiles.rs:493-494 | OUR-ONLY (spec divergence) |
| AUTH-008 | LOW | `NONCE_BASE_SLOT` and `LOCK_BASE_SLOT` are equal (`U256::from_limbs([1,0,0,0])`) — named aliasing obscures layout collision risk | handler_aa_helpers.rs:92-94 | SHARED with BASE |
| AUTH-009 | INFO | Clippy -D warnings fails (style/doc only) — CI would block merges | multiple files | OUR-ONLY |
| AUTH-010 | INFO | `clear_eip8130_tx_context()` is the only defense against stale thread-local on non-AA txs; panic path can skip it | handler.rs:223 | SHARED with BASE (known as BUG-CAND-C) |

---

## CRITICAL Findings

*No new CRITICAL findings beyond BUG-001..008 were identified in the authorization/ownership/predeploy mutation surface.*

---

## HIGH Findings

### AUTH-001 — `is_owner_authorized()` Returns `true` for `REVOKED_VERIFIER`

**File:** `rust/op-alloy/crates/consensus/src/transaction/eip8130/accessors.rs:58-65`

**Code:**
```rust
pub fn is_owner_authorized<DB: Database>(
    db: &mut DB,
    account: Address,
    owner_id: B256,
) -> Result<bool, DB::Error> {
    let (verifier, _) = read_owner_config(db, account, owner_id)?;
    Ok(verifier != Address::ZERO)   // BUG: REVOKED_VERIFIER (0xffff..ff) is != ZERO
}
```

**Attacker Model:** Any caller using `is_owner_authorized()` as the sole gate for owner validation on a revoked owner entry.

**Exploit:** `REVOKED_VERIFIER = address(type(uint160).max) = 0xffffffff...ff`. The check `verifier != Address::ZERO` passes for this sentinel value. If any downstream call site uses this function to decide whether to permit an operation (e.g., a future RPC handler or helper library), a revoked owner would appear authorized.

**Current Impact:** Limited. No production call site in this repository calls `is_owner_authorized()` — the function is exported but not consumed by the handler (which uses `validate_owner_against_effective_config()` which correctly checks `REVOKED_VERIFIER` explicitly) or the txpool (which checks `REVOKED_VERIFIER` directly at line 1034 of `eip8130_validate.rs`). The risk is a **latent API trap** for future consumers of the public crate API.

**Fix:**
```rust
pub fn is_owner_authorized<DB: Database>(
    db: &mut DB,
    account: Address,
    owner_id: B256,
) -> Result<bool, DB::Error> {
    let (verifier, _) = read_owner_config(db, account, owner_id)?;
    // Both Address::ZERO (empty) and REVOKED_VERIFIER are unauthorized states.
    Ok(verifier != Address::ZERO && verifier != REVOKED_VERIFIER)
}
```

**Delta vs BASE:** BASE upstream has the same function with the same defect. This is a shared bug. Recommend fixing in our port and upstreaming to BASE.

---

### AUTH-002 — Dual `ACCOUNT_CONFIG_DEPLOYED` Atomics Can Diverge

**Files:**
- `rust/op-alloy/crates/consensus/src/transaction/eip8130/predeploys.rs:50` — `static ACCOUNT_CONFIG_DEPLOYED: AtomicBool`
- `rust/op-revm/src/handler_aa_helpers.rs:84` — `static ACCOUNT_CONFIG_DEPLOYED: std::sync::atomic::AtomicBool`

**Attacker Model:** An operator restarting the node during the window between NATIVE_AA fork activation and the first config-change transaction.

**Exploit:** These are two distinct `static` instances in separate crates. The txpool path calls `op_alloy_consensus::mark_account_config_deployed()` (the first static). The execution handler calls `ACCOUNT_CONFIG_DEPLOYED.load()` in `handler_aa_helpers` (the second static). After a node restart both are `false`. The txpool's `mark_account_config_deployed()` call does not propagate to the handler's static.

Concrete scenario:
1. Node boots at block N+1, post-fork. `AccountConfiguration` is already deployed.
2. Txpool receives an AA config-change tx. `eip8130_validate.rs:1278` detects code exists, marks `op_alloy_consensus::ACCOUNT_CONFIG_DEPLOYED = true`.
3. Execution receives the same tx. `handler_aa_helpers.rs:399` checks its OWN static (still `false`), performs the DB code-existence check, and correctly marks its own static to `true`. **This path is safe on first tx.**
4. The divergence means **one extra DB round-trip per block-builder session**, not a security failure.

**Real risk is different:** If someone can construct a scenario where the txpool's fast-path check and the handler's check disagree on *whether* the contract is deployed (rather than just caching it differently), an attacker could bypass the config-change rejection guard. In practice this is not exploitable today because both checks do a DB lookup when their respective cached flag is `false`, and the DB lookup result is the ground truth. However the architectural duplication creates maintenance risk.

**Fix:** Consolidate into a single caching mechanism, or have `handler_aa_helpers` call the function from `op_alloy_consensus::predeploys` rather than keeping a private copy.

**Delta vs BASE:** BASE has a single codebase where both paths share the same static. This is an OUR-ONLY porting artifact from separating `op-alloy-consensus` and `op-revm` crates.

---

### AUTH-003 — `pre_writes` Applied Without Runtime Authorization at Inclusion

**File:** `rust/op-revm/src/handler.rs:489-500`

```rust
// --- Apply pre-execution storage writes (account creation only) ---
for w in &eip8130.pre_writes {
    journal.load_account(w.address)?;
    journal.sstore(w.address, w.slot, w.value)?;
}
```

**Attacker Model:** Adversary who has crafted an AA tx with a `Create` entry where `derive_account_address` produces an address that collides with an attacker-controlled contract address or — in a future deployment — a to-be-deployed system contract.

**Exploit:** `pre_writes` are computed at tx-conversion time in `eip8130_compat.rs` from `AccountChangeEntry::Create` entries. They are applied in `validate_against_state_and_deduct_caller` **unconditionally** with no check that:
1. The target account has no existing code.
2. The target account is not a system contract.

The `derive_account_address` function uses `ACCOUNT_CONFIG_ADDRESS` as the CREATE2 deployer, so a preimage collision against an *arbitrary* target address is computationally infeasible. However, an attacker **can construct a Create entry that deliberately targets an address they already know will be empty** and then race the deployment against a legitimate user attempting to deploy to the same address.

More concretely: no code-existence check occurs before `code_placements` overwrites the target address bytecode. If two users race to create an account at the same (salt, bytecode, owners) → same address, the second CREATE will silently overwrite the first account's code with identical bytecode (no harm), but also re-write the owner_config slots with potentially different initial owners (potentially harmful if `initial_owners` are different between the two tx submissions — though they produce the same address via `effective_salt` which commits to owner_id values).

**Assessment:** The computational hardness of CREATE2 preimage finding provides strong practical protection. The race scenario resolves safely for identical owner sets. The architectural absence of a guard is a latent weakness but not currently exploitable.

**Fix:** Add a code-existence check before `code_placements`:
```rust
for placement in &eip8130.code_placements {
    let existing = journal.load_account_with_code(placement.address)?;
    if existing.data.info.code_hash != keccak256([]) {
        return Err(eip8130_invalid_tx::<ERROR>("account already exists at CREATE2 address"));
    }
    // ... then place code
}
```

**Delta vs BASE:** Shared. BASE has the identical pattern.

---

## MEDIUM Findings

### AUTH-004 — Config Writes Applied Without Authorizer-Chain Validation in Estimation Mode

**File:** `rust/op-revm/src/handler.rs:625-766`

The `if !is_estimation { validate_config_change_preconditions...; validate_authorizer_chain...; }` block (lines 625-742) is skipped when `is_base_fee_check_disabled() == true`. However the `config_writes` and `sequence_updates` application at lines 744-768 lies **outside** this guard and executes in both modes.

This means in `eth_estimateGas` and `eth_call` contexts:
- Owner config changes are written to the journaled state without verifying the authorizer's signature or scope.
- Sequence counters are bumped without validation.

**Risk:** `eth_estimateGas` and `eth_call` always run on forked/temporary state that is never committed to the canonical DB. The state mutations are discarded after the call returns. Under standard node operation this is safe. If a node operator ever configures state-persistent simulation (e.g., `debug_traceCall` with `stateOverrides` that get committed, a non-standard extension), this becomes a privilege escalation vector.

**Fix (defense in depth):** Gate the `config_writes` block on `!is_estimation` as well, or skip sequence_updates in estimation mode. The missing authorizer validation already prevents config_writes from containing attacker-controlled data (they originate from the tx fields), so the risk is bounded. However, the pattern should be unified.

**Delta vs BASE:** Shared.

---

### AUTH-005 — DELEGATE Verifier Inner-Slot Reads Tautologically Same Data as Outer Check

**File:** `rust/op-revm/src/handler_aa_helpers.rs:358-374`

```rust
if verifier == crate::constants::DELEGATE_VERIFIER_ADDRESS && !has_pending_override {
    let inner_slot = aa_owner_config_slot(account, owner_id_uint);  // SAME slot as outer
    let inner_word = evm.ctx().journal_mut().sload(ACCOUNT_CONFIG_ADDRESS, inner_slot)?.data;
    let (inner_verifier, inner_scope) = parse_owner_config_word(inner_word);

    if inner_verifier == Address::ZERO { ... }  // outer already checked this
    if inner_scope != 0 && (inner_scope & required_scope) == 0 { ... }  // partially redundant
}
```

`aa_owner_config_slot(account, owner_id_uint)` computes `keccak256(account || keccak256(owner_id || 0))`. This **does not include the verifier address** in the slot key. The `_ownerConfig` mapping is `[ownerId][account]`, not `[ownerId][verifier][account]`.

The inner check reads the exact same storage slot as the outer `validate_owner_against_effective_config` call that just completed. At that point `on_chain_verifier == DELEGATE_VERIFIER_ADDRESS` (the outer check confirmed non-zero, non-REVOKED). The inner check then:
- `inner_verifier == Address::ZERO` → false (it's DELEGATE_VERIFIER_ADDRESS), so this never triggers.
- `inner_scope` check is equivalent to re-running the same scope gate.

**Effect:** The "delegation target" described in the comment is never actually looked up. The inner check provides no additional security beyond a second identical read of the same slot. The DELEGATE verifier's contract-level delegation chain is not verified at the protocol layer; it is presumably enforced by the DELEGATE verifier contract itself during the STATICCALL.

**Impact:** The inner check is a dead code path masquerading as a security gate. No known exploit, but the misleading comment creates a false security assumption for future maintainers.

**Fix:** Either remove the inner check (document that DELEGATE sub-chain validation happens in the verifier STATICCALL) or implement a correct two-slot lookup that actually resolves the delegation target account's owner_config.

**Delta vs BASE:** Shared. Identical code and comment in BASE's handler.

---

### AUTH-006 — No Minimum-Owner Guard: Last-Key Self-Revocation Bricks Account

**File:** `rust/op-revm/src/handler_aa_helpers.rs:554-631` (`validate_authorizer_chain`), `eip8130_compat.rs` (config write building)

There is no check that after applying `config_writes`, the target account retains at least one authorized owner. An account holder can:
1. Submit an AA tx with a `ConfigChange` that revokes their last owner key.
2. The authorizer chain is valid (they still have the key at validation time).
3. `config_writes` are applied, deleting the last owner entry.
4. The account now has no owner and no way to submit further changes (all AA txs require at least one valid owner auth).

**Self-revocation sentinel:** When the implicit EOA owner is revoked, `REVOKED_VERIFIER` is written to prevent re-entry via the implicit rule. For non-implicit owners, the slot is zeroed. Either way, a last-key revocation produces an account with `all slots == Address::ZERO` or the implicit slot `== REVOKED_VERIFIER`. Both states block all future AA transactions for that sender.

**Impact:** User-inflicted permanent account brick. An attacker who tricks a user into signing a config change that revokes their last key achieves the same result. There is no recovery mechanism described in the protocol.

**Fix (spec-level):** The `AccountConfiguration` Solidity contract should enforce `remaining_owners >= 1` as an invariant, and the handler's `validate_authorizer_chain` should count net owners and reject if any config change would result in zero owners (accounting for adds and removes in the same tx).

**Delta vs BASE:** Shared. BASE does not implement a minimum-owner guard either. This is a spec-level gap that should be escalated.

---

## LOW Findings

### AUTH-007 — TX_CONTEXT Precompile Active for ALL Txs When NATIVE_AA Spec Is Active

**File:** `rust/op-revm/src/precompiles.rs:493-494`, `rust/alloy-op-evm/src/aa_precompiles.rs`

```rust
let aa_context = context.tx().tx_type() == EIP8130_TX_TYPE;
if eip8130_precompiles_enabled(self.spec) || aa_context {
```

With `OpSpecId::NATIVE_AA` active, `eip8130_precompiles_enabled()` returns `true`. The condition `true || X` means the `if` branch always executes for all transactions, including regular L2 txs (type 0x00, 0x02) and deposits. A call to `TX_CONTEXT_ADDRESS` from a non-AA tx returns ZERO values (safe defaults), and a call to `NONCE_MANAGER_ADDRESS` returns the stored nonce for any queried (account, key) pair.

**Concern:** The `contains()` method at line 530 registers both addresses as "warm" precompile addresses in NATIVE_AA spec. Any contract can CALL these addresses. For `TX_CONTEXT`, the response is safely zeroed for non-AA callers. For `NONCE_MANAGER`, any contract can read any account's 2D nonce value — this is intentional (read-only view), but the addresses are not listed in `warm_addresses` unless NATIVE_AA is active, creating a gas accounting difference.

**Delta vs BASE:** BASE's comment on `getNonce` states it is intentionally read-unrestricted. Our port matches this. However, the spec divergence from standard precompile behavior (precompiles typically revert for non-AA callers in some designs) may differ from TEMPO design expectations.

---

### AUTH-008 — `NONCE_BASE_SLOT` and `LOCK_BASE_SLOT` Are Aliased to the Same Value

**File:** `rust/op-revm/src/handler_aa_helpers.rs:92-94`

```rust
const NONCE_BASE_SLOT: U256 = U256::from_limbs([1, 0, 0, 0]);
const LOCK_BASE_SLOT: U256 = U256::from_limbs([1, 0, 0, 0]);
```

Both constants share value `1`. `NONCE_BASE_SLOT` is used to derive nonce storage slots in the `NonceManager` precompile (address `0x..aa02`). `LOCK_BASE_SLOT` is used to derive lock state slots in `AccountConfiguration` (address `0xf946...fee`). These are **different contracts** at different addresses, so the slot collision is irrelevant at the storage level. However, the naming creates cognitive confusion: a future maintainer could mistakenly believe these map to the same contract's storage.

**Delta vs BASE:** Shared. BASE has the same dual-constant pattern.

---

## INFO Findings

### AUTH-009 — Clippy -D warnings Fails on Multiple Crates

`cargo clippy -p op-revm -- -D warnings` fails with 61 errors, all style/documentation (`doc_markdown`, `map_or_default`, `bool_then`, `const_fn`). `cargo clippy -p op-alloy-consensus -- -D warnings` fails with 45+ errors. These are not logic or safety issues, but would block CI in a project that enforces `-D warnings`. If the CI pipeline does not enforce clippy-clean builds, this should be noted.

---

### AUTH-010 — Stale Thread-Local on Panic Path (known BUG-CAND-C, repeated for completeness)

**File:** `rust/op-revm/src/handler.rs:223`

`clear_eip8130_tx_context()` is called at the start of `validate_against_state_and_deduct_caller`. If a previous tx's EVM execution panicked before reaching the clear call, the thread-local `EIP8130_TX_CONTEXT` retains stale AA data. The precompile dispatch independently gates on `tx.tx_type() == EIP8130_TX_TYPE`, so a stale context for a non-AA tx is benign (the context is never read by the non-AA tx's precompile calls). Risk is theoretical; shared with BASE.

---

## Hunt List Disposition

| Hunt item | Verdict | Finding |
|---|---|---|
| 1. Owner config mutation gating | SECURE | Writes happen only in system-call context (journal.sstore from handler). User contracts cannot SSTORE to `ACCOUNT_CONFIG_ADDRESS` storage; config_writes target `sender`'s own slots from tx-time. |
| 2. Unbounded key set | CONCERN (LOW) | `_ownerConfig[ownerId][account]` can grow if unlimited owners are added. No cap enforced at handler level. Gas cost for each SLOAD/SSTORE provides economic deterrent. Bounded by `MAX_ACCOUNT_CHANGES_PER_TX` per tx, but cumulative unbounded. |
| 3. Initial owner setup race / front-run | SECURE | CREATE2 address is committed by (user_salt, bytecode, initial_owners sorted). Initial owners are baked into the salt via `effective_salt`. Front-running with different owners produces a different address. |
| 4. TX_CONTEXT READ who / WRITE | SECURE READ; N/A WRITE | Read: any contract can call; returns AA context for AA txs, zeros otherwise. Write: not possible (precompile, no SSTORE path). Context set by handler before execution, cleared before each tx. |
| 5. NONCE_MANAGER scoping | SECURE | Read: any contract can call `getNonce` (read-only view). Write: only handler SSTOREs directly, gated to AA tx type. |
| 6. Predeploy bytecode immutability | LARGELY SECURE | `0x..aa02` and `0x..aa03` addresses are below standard contract address range. No evidence of CREATE-at-precompile protection in revm; but these addresses cannot receive bytecode from user transactions without first holding `SELFDESTRUCT`-then-`CREATE`, which requires code to exist there first. AA predeploy contracts at `0xf946..fee` etc. are deployed via TxDeposit at fork activation, not user-facing CREATE. |
| 7. Predeploy CALL from user contracts | SECURE (read-only exposure) | User contracts can call `NONCE_MANAGER` (gets read-only nonce view) and `TX_CONTEXT` (gets zeroed data or current AA tx context). No sensitive key material is exposed. |
| 8. Self-revocation / last key | OPEN — AUTH-006 | No minimum-owner guard. Account can be bricked by revoking last key. |
| 9. Hardfork activation race | SECURE | Pre-fork txs cannot write to `NONCE_MANAGER_ADDRESS` or `ACCOUNT_CONFIG_ADDRESS` storage because: (a) NONCE_MANAGER requires being that account, (b) ACCOUNT_CONFIG is only deployed at fork activation via TxDeposit. Post-fork state starts clean. |
| 10. system_call_one paths | VERIFIED CORRECT | `ACCOUNT_CONFIG_ADDRESS` constant in `handler_aa_helpers.rs` matches the one in `op_alloy_consensus::predeploys::ACCOUNT_CONFIG_ADDRESS` (both derive from `create(0x4210...000b, 0)`). BUG-006/007 previously corrected. |
| 11. TX_CONTEXT field schema | NO SENSITIVE LEAK | Fields exposed: sender, payer, owner_id, max_cost, gas_limit, calls. No private key material, no other account data. owner_id is a public identifier. |
| 12. Cross-account contamination / TX_CONTEXT in sub-calls | SECURE | Thread-local `EIP8130_TX_CONTEXT` is set once per AA tx and read-only during execution. Sub-calls to TX_CONTEXT address always return the OUTER AA tx's context (by design). No mutation of context during execution. |
| 13. Reentrancy into predeploys | SECURE | `NONCE_MANAGER` and `TX_CONTEXT` are native precompiles — no EVM code runs at those addresses. `ACCOUNT_CONFIG` is a Solidity contract that can be re-entered, but the handler writes all state changes BEFORE executing call phases. Reentrancy would see the already-updated state (correct). No reentrancy lock is needed or present. |
| 14. TX type 0x7B in EXTCODECOPY of predeploys | SECURE | Precompile addresses (`0x..aa02`, `0x..aa03`) have no bytecode; `EXTCODECOPY` returns empty. AA-deployed contracts (`0xf946..fee` etc.) have Solidity bytecode and are CALL-able; no special `EXTCODECOPY` restriction needed. |

---

## Delta vs BASE

| Area | OUR port | BASE upstream | Risk |
|---|---|---|---|
| `ACCOUNT_CONFIG_DEPLOYED` | Two independent `AtomicBool` statics in separate crates (AUTH-002) | Single shared static | LOW — no security failure, extra DB check on restart |
| `is_owner_authorized()` REVOKED check | Missing (AUTH-001) | Same missing check | LOW public API trap — not used in production paths on either side |
| DELEGATE inner-slot check | Tautological re-read (AUTH-005) | Same tautological code | MEDIUM — shared design gap, not an OUR-only delta |
| Clippy status | Fails -D warnings (AUTH-009) | Not checked | hygiene |

## Delta vs TEMPO Design

The TEMPO v2/v3/v4 design documents were not available at file paths specified in the request. The following TEMPO-relevant observations are based on our code and BASE comparison:

- TEMPO v4 design specifies a `REVOKED_VERIFIER` sentinel for implicit EOA revocation. Our implementation correctly uses `0xffffffff...ff`. CONCORDANT.
- TEMPO specifies owner scope bitmask `SENDER=0x02, PAYER=0x04, CONFIG=0x08`. Our constants match (`constants.rs:77-83`). CONCORDANT.
- No TEMPO design document was checked for minimum-owner enforcement. AUTH-006 may be a TEMPO spec gap rather than an implementation gap.

---

## Verdict: Authorization Path

**AMBER**

- The primary authorization path (sender/payer verification via native or custom verifier + owner_config scope check + config change authorizer chain) is **functionally correct** and **cryptographically sound** as implemented.
- No currently exploitable authorization bypass exists in production transaction processing.
- Three issues elevate the verdict from GREEN to AMBER:
  1. **AUTH-001** (is_owner_authorized REVOKED bypass): public API trap that future consumers of `op-alloy-consensus` could trigger.
  2. **AUTH-002** (dual AtomicBool): architectural debt that could produce divergent behavior under edge cases.
  3. **AUTH-006** (no minimum-owner guard): user can permanently brick their own account, and a socially-engineered attacker can brick a victim's account if they can get a co-signed "rogue config change."

**Required before mainnet:**
- AUTH-001: Fix `is_owner_authorized` to exclude `REVOKED_VERIFIER`. Low-effort, high-safety-value.
- AUTH-006: Escalate to spec committee; decide whether minimum-owner enforcement belongs in the protocol handler or the `AccountConfiguration` Solidity contract.

**Recommended before mainnet:**
- AUTH-002: Consolidate `ACCOUNT_CONFIG_DEPLOYED` into a single static or cross-crate call.
- AUTH-005: Remove misleading inner-check comment; document that DELEGATE sub-chain is verifier-enforced.
