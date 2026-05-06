# Security Review — revm EIP-8130 Native AA Port

- **Reviewer**: Claude Sonnet 4.6 (production-grade audit)
- **Date**: 2026-05-06
- **Branch**: `feat/eip-8130-port`
- **Scope**: handler lifecycle, thread-local TX_CONTEXT, system calls, journal semantics, error/panic paths, AA-precompile integration into PrecompilesMap.
- **Methodology**: Cargo check + clippy + test run; git diff HEAD~5; cross-reference with https://github.com/base/base crates/execution/revm and crates/common/evm.

---

## Tooling Results

| Tool | Result |
|---|---|
| `cargo check -p op-revm` | PASS |
| `cargo check -p alloy-op-evm` | PASS |
| `cargo test -p op-revm` | PASS — 81 tests |
| `cargo fmt --check -p op-revm` | FAIL — 1 style diff in `fast_lz.rs:91` (pre-existing, unrelated to AA) |
| `cargo clippy -p op-revm -- -D warnings` | FAIL — 61 warnings promoted to errors; see HIGH-001 |

---

## Previous Findings — Resolution Status

| ID | Prior verdict | Current status |
|---|---|---|
| BUG-001..007 | Fixed | Confirmed resolved — not visible in this diff |
| BUG-008 | Fixed (PrecompilesMap) | CONFIRMED RESOLVED — `aa_precompiles.rs` wires both precompiles; `op_precompiles_map` tested |
| BUG-CAND-A | Medium — missing `load_account_with_code_mut` | CONFIRMED RESOLVED — `api/exec.rs:148,177` has both calls with comment citing bluealloy/revm#3484 |
| BUG-CAND-B | Low — INTEROP ordering | CONFIRMED — enum ordering still: `KARST=110, INTEROP=111, NATIVE_AA=112`. Latent semantic incoherence but no AA execution impact (NATIVE_AA gating uses `is_enabled_in` directly). |
| BUG-CAND-C | Info — thread-local not cleared on error/panic | PARTIALLY IMPROVED — our code clears BEFORE the deposit branch (line 223), so deposit txs now DO trigger the clear, unlike base (which clears only after the deposit early-return). Still not cleared in `catch_error`. Documented below as NEW-004. |
| BUG-CAND-009..014 | Low/hygiene | Not revisited (out of scope for this diff) |

---

## New Findings

---

### HIGH-001 — Clippy -D warnings fails (61 violations); CI gate broken

**Severity**: HIGH  
**File**: `rust/op-revm/src/handler_aa_helpers.rs`, `rust/op-revm/src/transaction/eip8130.rs` (and others)  
**Location**: project-wide  

`cargo clippy -p op-revm -- -D warnings` produces 61 errors and refuses to compile.
Key violations:

- `handler_aa_helpers.rs:467` — `run_custom_verifier_staticcall` has 9 arguments (limit is 7).
- Multiple `doc_markdown` errors (missing backticks around type names in doc comments).
- `const fn` opportunities, `map_or` simplifications, `if`-collapse suggestions.

If CI enforces `-D warnings`, every PR touches to this crate will block. The fmt check also
has a pre-existing diff in `fast_lz.rs:91`.

**Delta vs base**: base's handler is a single file and the clippy flag state is unknown, but
the AA-specific additions (handler_aa_helpers.rs, eip8130.rs) are new code that should be
clean before merge.

**Action**: Fix doc comment backticks mechanically. Extract a builder struct for
`run_custom_verifier_staticcall` params to reduce arg count to ≤7.

---

### HIGH-002 — `op_precompiles_map` uses exact equality for `NATIVE_AA`; breaks any future fork

**Severity**: HIGH  
**File**: `rust/alloy-op-evm/src/aa_precompiles.rs:132`  
**File**: `rust/op-revm/src/precompiles.rs:117` (`eip8130_precompiles_enabled`)  

```rust
// aa_precompiles.rs:132
if spec == OpSpecId::NATIVE_AA {          // exact equality
    map.extend_precompiles([...]);
}

// precompiles.rs:117
fn eip8130_precompiles_enabled(spec: OpSpecId) -> bool {
    matches!(spec, OpSpecId::NATIVE_AA)   // exact equality
}
```

But `handler.rs:104` correctly uses `is_enabled_in`:
```rust
if !evm.ctx().cfg().spec().is_enabled_in(OpSpecId::NATIVE_AA) {
    return Err(...);
}
```

If a future fork (e.g., `NATIVE_AA_V2`) is added to `OpSpecId` with a higher discriminant,
any EVM running under that spec will:
1. Gate AA txs correctly (via `is_enabled_in`) — allowed.
2. NOT dispatch calls to `TX_CONTEXT_ADDRESS` or `NONCE_MANAGER_ADDRESS` through the
   `PrecompilesMap` — they fall through to the stub `0xFE` bytecode and **revert**.
3. NOT include the AA addresses in `warm_addresses` — cold access penalty.

This is a **silent consensus divergence**: the handler allows AA txs but precompile calls
inside them revert. Payer cannot call `getMaxCost()`.

**Delta vs base**: base has the same pattern (`spec == OpSpecId::BASE_V1`), so this is
shared. However, base does not yet have a fork after BASE_V1; adding one without fixing
this pattern first would silently break AA on that fork.

**Action**: Change both to `spec.is_enabled_in(OpSpecId::NATIVE_AA)`.

---

### MEDIUM-001 — `u64` integer overflow in nonce increment: `nonce_sequence + 1`

**Severity**: MEDIUM  
**File**: `rust/op-revm/src/handler.rs:348`  

```rust
// handler.rs:268
let nonce_sequence = tx.nonce();   // returns u64

// handler.rs:348
U256::from(nonce_sequence + 1)     // u64 arithmetic: wraps to 0 if nonce_sequence == u64::MAX
```

In Rust, integer arithmetic on primitive types wraps in release mode (no panic). If
`nonce_sequence == u64::MAX`, `nonce_sequence + 1` silently wraps to `0`. The
`U256::from(0)` is then stored as the new nonce, meaning the nonce channel rolls back to
`0` instead of overflowing gracefully.

The preceeding check at lines 331-343 only validates the current value against `expected`;
it does not guard against `u64::MAX`. A specially crafted nonce sequence of `u64::MAX` would
pass validation (if the on-chain slot also holds `u64::MAX`) and then write `0` as the next
nonce, effectively resetting replay protection.

**Exploitability**: An attacker who has exhausted 2^64-1 nonce slots for a given `nonce_key`
can trigger this. In practice this is infeasible today, but it is a spec violation (nonces
should never wrap) and a correctness bug.

**Delta vs base**: Same pattern at base `handler.rs:918` — shared defect. Both should use
`nonce_sequence.checked_add(1).ok_or(...)` and propagate as an error.

**Action**: Replace `U256::from(nonce_sequence + 1)` with:
```rust
let next_seq = nonce_sequence.checked_add(1)
    .ok_or_else(|| ERROR::from_string("AA nonce overflow".into()))?;
U256::from(next_seq)
```

---

### MEDIUM-002 — `ACCOUNT_CONFIG_DEPLOYED` AtomicBool never reset: test pollution and devnet risk

**Severity**: MEDIUM  
**File**: `rust/op-revm/src/handler_aa_helpers.rs:84-85,399-404`  

```rust
static ACCOUNT_CONFIG_DEPLOYED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
```

This process-global static is set to `true` on first detection of the
`AccountConfiguration` contract being deployed. It is never reset.

**Test pollution**: Any test that triggers `validate_config_change_preconditions` with a DB
that has `ACCOUNT_CONFIG_ADDRESS` code will permanently set the flag. All subsequent tests
in the same process that rely on the flag being `false` will skip the code-existence check,
potentially accepting config-change txs against an address with no code.

**Devnet risk**: If a devnet resets its state (e.g., re-genesis after a botched deployment)
without restarting the node process, the flag remains `true`. Config-change txs would be
accepted even though `AccountConfiguration` is not yet deployed, and their writes would go
to a plain EOA address, silently corrupting state.

**Delta vs base**: Same pattern in `base/crates/execution/revm/src/handler.rs:83-84`.
Shared defect.

**Action**: Convert to a per-block or per-journal-context check, or add a process-level
reset hook. Simplest fix: make the check non-cached (always SLOAD on first config-change tx
per block), adding ~2100 gas overhead per block where this check is needed.

---

### MEDIUM-003 — `clear_eip8130_tx_context` not called in `catch_error`; stale context persists on non-deposit error paths

**Severity**: MEDIUM  
**File**: `rust/op-revm/src/handler.rs:1135-1196`  

The `catch_error` handler is called when any non-recoverable error propagates out of `run()`.
For AA transactions, `set_eip8130_tx_context()` is called at line 275 during
`validate_against_state_and_deduct_caller`. If an error occurs after that point (e.g., payer
insufficient balance at line 296-301, nonce mismatch at line 331-338), the context is set
but the tx never reaches `execution_result()`.

`catch_error` performs cleanup:
```rust
evm.ctx().chain_mut().clear_tx_l1_cost();
evm.ctx().local_mut().clear();
evm.frame_stack().clear();
// *** clear_eip8130_tx_context() is NOT called here ***
```

The stale context persists until the NEXT transaction's
`validate_against_state_and_deduct_caller` is called (line 223). During the window between
`catch_error` and the next tx's validation, the thread-local holds the failed AA tx's
sender, payer, owner_id, max_cost, and call_phases.

**Exploitability**: Not directly exploitable. The `PrecompilesMap` AA precompiles only
dispatch when called from within an EVM frame (during `run_exec_loop`). `catch_error` is
invoked outside of frame processing, so no call can reach the precompile. The stale state
cannot be read by external code in this gap.

**However**: If future code adds a step between `catch_error` and the next tx's
`validate_against_state` that reads the thread-local (e.g., receipts processing that calls
a hook using `get_eip8130_tx_context()`), this would be a leak.

**Delta vs base**: Same omission in `base/crates/execution/revm/src/handler.rs:1590-1594`.
Shared defect. Our clearing at line 223 BEFORE the deposit branch is actually an
improvement over base (deposit txs now clear the context in ours; they do not in base).

**Action**: Add `crate::precompiles::clear_eip8130_tx_context()` to `catch_error` before
the final cleanup block. Low blast-radius change.

---

### LOW-001 — `eip8130_precompiles_enabled` and `op_precompiles_map` admit `aa_context` bypass for any spec

**Severity**: LOW  
**File**: `rust/op-revm/src/precompiles.rs:493-513`  

```rust
let aa_context = context.tx().tx_type() == EIP8130_TX_TYPE;
if eip8130_precompiles_enabled(self.spec) || aa_context {
    // ... dispatch to NONCE_MANAGER and TX_CONTEXT precompiles
}
```

The `|| aa_context` condition means: even when `NATIVE_AA` is not the active spec, an AA
tx type (`0x7B`) will cause the AA precompile dispatch to run. This is intentional as a
defense-in-depth gate (ensuring the precompiles are reachable from AA txs even if the
static map has not been built for this spec). However:

1. `validate_env` (line 104) already rejects AA txs if `!spec.is_enabled_in(NATIVE_AA)`.
   So in practice, no AA tx can reach `OpPrecompiles::run` on a pre-NATIVE_AA spec.
2. The `aa_context` branch is still reachable via maliciously crafted `CallInputs` that set
   `bytecode_address = TX_CONTEXT_ADDRESS` in a non-AA tx if any frame is constructed
   directly (e.g., inspector hooks, `transact_raw` with a modified tx type field).

**Practical risk**: Low. The precompile bodies themselves gate on `tx.tx_type()` for most
fields, returning zero values for non-AA txs. But the `aa_context` bypass is surprising
behavior that could confuse future auditors.

**Delta vs base**: Same pattern.

---

### LOW-002 — `nonce_free_hash.unwrap_or_default()` uses `B256::ZERO` if field is `None`

**Severity**: LOW  
**File**: `rust/op-revm/src/handler.rs:373`  

```rust
let nf_hash = eip8130.nonce_free_hash.unwrap_or_default();
```

In the nonce-free branch (`nonce_key == NONCE_KEY_MAX`), `nonce_free_hash` is populated by
`eip8130_compat.rs:454-455` as `Some(sender_signature_hash(tx))`. The `unwrap_or_default`
falling to `B256::ZERO` is therefore unreachable in production.

However, if `Eip8130Parts` is constructed manually (e.g., in tests or via deserialization)
with `nonce_key == NONCE_KEY_MAX` but `nonce_free_hash == None`, the handler would use
`B256::ZERO` as the replay-protection hash. Multiple such transactions would all collide
in `aa_expiring_seen_slot(B256::ZERO)`:
- First tx: stores its expiry at the ZERO slot.
- Second tx: replay check fires (ZERO hash + non-expired = duplicate detection) — rejects.

This effectively makes all manually-constructed nonce-free txs with `None` hash mutually
incompatible. No funds risk, but test confusion.

**Action**: Replace with explicit error propagation:
```rust
let nf_hash = eip8130.nonce_free_hash.ok_or_else(|| {
    ERROR::from_string("nonce_free_hash missing for nonce-free tx".into())
})?;
```

---

### LOW-003 — `gas_remaining + unused_verification_gas` at line 909 lacks overflow guard

**Severity**: LOW  
**File**: `rust/op-revm/src/handler.rs:909`  

```rust
result_gas.erase_cost(gas_remaining + unused_verification_gas);
```

Both `gas_remaining: u64` and `unused_verification_gas: u64` are bounded:
- `gas_remaining` ≤ original `gas_limit` (execution budget).
- `unused_verification_gas` ≤ `custom_verifier_gas_cap` ≤ 200,000 by default.

Their sum cannot realistically overflow `u64` given that `gas_limit` is also a `u64` and
the total consumed gas must fit in a block. However:
1. If `set_custom_verifier_gas_cap()` is called with `u64::MAX`, then
   `unused_verification_gas` can be `u64::MAX`, causing the addition to overflow and
   `erase_cost` to receive `0` (wraparound), producing a wrong gas accounting result.
2. No range check exists on the `CUSTOM_VERIFIER_GAS_CAP` atom.

**Action**: Use `saturating_add`:
```rust
result_gas.erase_cost(gas_remaining.saturating_add(unused_verification_gas));
```
And add a validation that `set_custom_verifier_gas_cap` rejects values larger than some
reasonable maximum (e.g., 10,000,000).

---

### INFO-001 — `run_custom_verifier_staticcall` has 9 function parameters (clippy limit 7)

**Severity**: INFO  
**File**: `rust/op-revm/src/handler_aa_helpers.rs:467`  

Nine parameters make the function signature unwieldy and reduce readability. Clippy flags
this as a lint. Consider extracting a `VerifierCallParams` struct:

```rust
struct VerifierCallParams<'a> {
    verifier: Address,
    calldata: &'a Bytes,
    caller: Address,
    gas_cap: u64,
    gas_used: &'a mut u64,
    call_failed_msg: &'static str,
    invalid_owner_id_msg: &'static str,
}
```

---

### INFO-002 — Thread-local TX_CONTEXT concurrency note

**Severity**: INFO  

The `EIP8130_TX_CONTEXT` thread-local is safe for concurrent execution because:
1. Each OS thread has its own copy of the thread-local.
2. tokio tasks that run blocking work on dedicated threads (via `spawn_blocking`) would each
   get their own independent copy.
3. Standard tokio async tasks share threads but AA execution is synchronous (blocking DB
   access prevents truly async EVM execution).

No cross-thread contamination is possible with `thread_local!`. The concern is only same-
thread sequential reuse (addressed in MEDIUM-003 / BUG-CAND-C).

---

### INFO-003 — `ACCOUNT_CONFIG_DEPLOYED` Relaxed ordering is semantically correct but misleading

**Severity**: INFO  
**File**: `rust/op-revm/src/handler_aa_helpers.rs:84-85`  

```rust
static ACCOUNT_CONFIG_DEPLOYED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
```

`Ordering::Relaxed` is used for both the load and the store. The comment states "Relaxed
ordering is safe because the flag only transitions `false → true`". This is correct:
the flag is monotone, and the worst outcome of seeing a stale `false` is an extra SLOAD
(which happens anyway). There is no memory ordering hazard here. The note is for future
reviewers.

---

## Hunt-List Verification Summary

| Item | Verdict |
|---|---|
| 1. Thread-local lifecycle | Clear called at line 223 (before deposit), NOT in catch_error (MEDIUM-003). Stale context benign in current code but fragile. |
| 2. `load_account_with_code_mut(caller)` parity | RESOLVED — api/exec.rs:148,177 both fixed. |
| 3. PrecompilesMap wiring (BUG-008) | RESOLVED — aa_precompiles.rs + op_precompiles_map confirmed. Both AA precompiles in map for NATIVE_AA spec. |
| 4. Journal write/revert correctness | Phase checkpoint at line 785; revert at line 855 on failure. Pre-execution writes (nonce, delegation, code_placements) are NOT checkpointed — they are intentionally permanent. Config writes ARE in the execution() phase and subject to the `if !is_estimation` gate, but config writes themselves are not checkpointed per phase. If config write succeeds but a later phase reverts, config writes persist (correct per spec: config changes are unconditional on call success). |
| 5. System-call ordering | Order: clear_ctx → deduct payer → nonce → delegation → pre_writes → [execution()] → validate/verify → config_writes → phases. Nonce is committed before call phases; if all phases fail, nonce stays incremented (AA tx is consumed). This is correct per spec. |
| 6. INTEROP vs KARST vs NATIVE_AA ordering | BUG-CAND-B confirmed. INTEROP=111, KARST=110, NATIVE_AA=112. INTEROP.into_eth_spec()=PRAGUE, KARST.into_eth_spec()=OSAKA. NATIVE_AA gating uses `is_enabled_in` so no AA impact. |
| 7. Hardfork-gate consistency | validate_env uses `is_enabled_in(NATIVE_AA)`, op_precompiles_map uses `spec == NATIVE_AA` (HIGH-002). Txpool admission and parsing are in op-alloy-consensus (outside this scope). |
| 8. Witness/proof compatibility | BUG-CAND-A RESOLVED — both system_call paths now call load_account_with_code_mut(caller). |
| 9. u64/overflow | MEDIUM-001 (nonce overflow), LOW-003 (gas sum). |
| 10. Panic-safe handlers | No unwrap/expect in hot AA paths. `debug_assert!` at precompiles.rs:271 is acceptable (guard only fires in debug mode). mint.unwrap_or_default() and blob_gasprice.unwrap_or_default() are safe (Option, not Result). |
| 11. Concurrent execution | Thread-local is per-thread — safe (INFO-002). |
| 12. inspect vs system_call drift | Both `inspect_one_system_call_with_caller` and `system_call_one_with_caller` now have the `load_account_with_code_mut(caller)` call. No drift detected. |
| 13. Spec gating drift | env.rs, block/native_aa.rs, hardforks all use `is_native_aa_active_at_timestamp`. Consistent (BUG-CAND-012 POSITIVE confirmed). |
| 14. Re-entrancy via system_call | System txs (L1Block updates) use `SystemCallTx` not AA. No path from AA phase call back into a system call facility. |
| 15. Call depth | Both phase calls and verifier STATICCALLs use `depth: 0` as the outermost frame. Revm's internal depth counter increments from 0 for sub-calls, protecting against 1024 violation. This is correct. |

---

## Approval Verdict

**BLOCK — 2 HIGH + 3 MEDIUM issues found.**

### Required before merge

| ID | Action |
|---|---|
| HIGH-001 | Fix 61 clippy violations (doc backticks, arg count, simplifications) |
| HIGH-002 | Change `spec == NATIVE_AA` to `spec.is_enabled_in(NATIVE_AA)` in both `eip8130_precompiles_enabled` and `op_precompiles_map` |
| MEDIUM-001 | Replace `nonce_sequence + 1` with `checked_add(1)?` |
| MEDIUM-002 | Add `ACCOUNT_CONFIG_DEPLOYED.store(false, Relaxed)` to a process-level test reset, or eliminate the cache and always SLOAD |
| MEDIUM-003 | Add `crate::precompiles::clear_eip8130_tx_context()` to `catch_error` |

### Recommended (non-blocking)

| ID | Action |
|---|---|
| LOW-001 | Document the `aa_context` bypass in a comment; consider removing it if `validate_env` is always called first |
| LOW-002 | Replace `unwrap_or_default()` with explicit error for `nonce_free_hash` |
| LOW-003 | Use `saturating_add` for gas sum; validate `set_custom_verifier_gas_cap` input |
| INFO-001 | Extract `VerifierCallParams` struct to reduce arg count |
