# Phase B Master Summary — Symbol-Level Cross-Reference

- **Generated**: 2026-05-05
- **Method**: 4 parallel subagents covering 4 layer subsets.

## Aggregate findings

**0 NEW BUGs in BUG-001..008 severity class.**
**9 NEW BUG-CANDIDATES**, mostly LOW or hygiene. One MEDIUM worth following up.

| Layer | Agent report | New BUG-CANDIDATEs | Severity max | Hygiene gaps |
|---|---|---|---|---|
| Consensus types (op-alloy/crates/consensus) | `phase-b-consensus-types.md` | 0 | — | 1 (disabled `mod tests` — 39 tests not running) |
| revm extensions (op-revm) | `phase-b-revm.md` | 3 | 🟡 MEDIUM | — |
| TxPool + receipts | `phase-b-txpool-receipts.md` | 0 | — | 1 (`Arbitrary` impl missing Eip8130 arm; shared with base) |
| alloy-op-evm + hardforks + rpc-types | `phase-b-evm-hardforks-rpc.md` | 6 | 🟢 LOW | — |

## New BUG-CANDIDATES (priority order)

### 🟡 BUG-CAND-A (MEDIUM) — Missing `load_account_with_code_mut(caller)?` in op-revm system-call paths
- **Location**: `op-revm/src/api/exec.rs`, `system_call_one_with_caller` + `inspect_one_system_call_with_caller`
- **What's missing**: base added two `load_account_with_code_mut(caller)?` calls to satisfy revm bluealloy/revm#3484 for Geth proof / execution-witness compatibility
- **Symptom**: AA execution itself unaffected (L1Block system tx runs cleanly), but witness-based proofs may diverge from base on the L1Block step
- **Risk**: visible if/when we run kona / fault-proof flows that consume execution witnesses
- **Suggested action**: port the two missing calls; low blast-radius

### 🟢 BUG-CAND-B (LOW) — `INTEROP` spec ordered between `KARST` and `NATIVE_AA`
- **Location**: `op-revm/src/spec.rs`
- **Issue**: `OpSpecId::INTEROP.is_enabled_in(KARST) == true` (suggesting INTEROP > KARST) but `INTEROP.into_eth_spec() == PRAGUE` while `KARST.into_eth_spec() == OSAKA` (regression in Eth spec mapping)
- **Symptom**: latent correctness trap — monotone invariant `is_enabled_in` ↔ `into_eth_spec` ordering is broken
- **AA-specific impact**: none (NATIVE_AA gating uses `is_enabled_in(NATIVE_AA)` directly)
- **Suggested action**: reorder enum or document the divergence

### 🟢 BUG-CAND-C (INFO) — `clear_eip8130_tx_context()` not called on error/panic paths
- **Location**: `op-revm/src/handler.rs` thread-local lifecycle
- **Issue**: thread-local can be stale on thread reuse if a previous tx errored/panicked before clearing
- **Symptom**: practically benign — precompile dispatch independently gates on `tx.tx_type() == EIP8130_TX_TYPE`, so a stale thread-local for a non-AA tx is ignored
- **Shared with base**: yes
- **Suggested action**: defer; document; if cleanup desired, wrap in `scopeguard` or `Drop` impl

### 🟢 BUG-CAND-009 (LOW) — `unwrap().try_into().unwrap()` in receipt OtherFields conversion
- **Location**: `op-alloy/crates/rpc-types/src/receipt.rs:155-159`
- **Issue**: ours uses `.unwrap()` chain; base migrated to fallible `TryFrom`
- **Symptom**: latent panic on future schema changes
- **Suggested action**: port the `TryFrom` migration

### 🟢 BUG-CAND-010 (DOC) — Permissive doc comment in `block/native_aa.rs:13-17`
- **Location**: `alloy-op-evm/src/block/native_aa.rs`
- **Issue**: doc wording slightly more permissive than base
- **Symptom**: semantics match (txpool gates correctly), just documentation consistency
- **Suggested action**: tighten prose

### 🟢 BUG-CAND-011 (LOW) — `OpTransactionRequest` missing `chain_id()` and `deploy_code()` builder methods
- **Location**: `op-alloy/crates/rpc-types/src/transaction/request.rs`
- **Issue**: API parity gap — base added these helpers, ours doesn't have them
- **Symptom**: callers must construct manually; not a bug, just ergonomic gap
- **Suggested action**: port the methods

### 🟢 BUG-CAND-012 (NONE) — `is_native_aa_active_at_timestamp` rename verified consistent
- **Location**: env.rs:38, block/native_aa.rs:45, hardforks/lib.rs:252
- **Issue**: not an issue — confirmed the rename is consistent
- **Suggested action**: none

### 🟢 BUG-CAND-013 (LOW) — `block/native_aa.rs` has zero tests
- **Location**: `alloy-op-evm/src/block/native_aa.rs`
- **Issue**: base ships 3 tests for predeploy stub deployment; ours has none
- **Symptom**: test-coverage gap for the BASE_V1/NATIVE_AA fork-activation flow
- **Suggested action**: port 3 tests

### 🟢 BUG-CAND-014 (POSITIVE) — RPC wire format byte-compatible with base
- **Location**: receipts + transaction request JSON serialization
- **Issue**: not an issue — confirmed
- **Suggested action**: none (positive finding; AA receipts/requests are interoperable)

## Hygiene gaps (not bugs but worth tracking)

### GAP-CONSENSUS-TESTS — 39 disabled tests in op-alloy consensus
- **Location**: `op-alloy/crates/consensus/src/transaction/eip8130/mod.rs:113-118` disables `mod tests`
- **Cause**: tests use pre-BUG-001 schema (`from: Address::*` literals incompatible with `Option<Address>`)
- **Fix**: mechanical sed migration (~20 lines) restores the suite
- **Already-tested via**: inline module tests (gas:26, validation:18, signature:5, tx:9 = 58 cases) plus all our T-XX e2e

### Arbitrary impl missing Eip8130 arm
- **Location**: `OpReceiptEnvelope` arbitrary derive on both forks
- **Cause**: pre-existing in base, not specific to our fork
- **Affects**: fuzz coverage only, not production
- **Fix**: add the `Eip8130` arm to both forks; upstream-worthy

## Methodology validation

Phase B caught what file-by-file diff would miss:

- **BUG-CAND-A** (missing system-call code-load calls) hides at semantic level — same function names exist on both sides, but base's body has 2 extra lines we didn't notice
- **BUG-CAND-B** (INTEROP ordering) is a 1-line enum reorder; visible only by reading the spec.rs definitions side-by-side
- **BUG-CAND-009** uses different fallibility patterns; both compile, but error semantics differ

These are exactly the class of issues the **carpet-bombing audit was designed to catch**. The 4 parallel agents covered ~250 file-pairs in ~10 minutes wall time vs. ~3-4 work-days estimated for sequential single-agent execution.

## Combined audit count

| Source | Findings | Severity |
|---|---|---|
| Phase A (file inventory) | 0 bugs, 1 false-positive class identified | hygiene |
| Phase C (integration matrix) | BUG-008 confirmed | 🔴 functional |
| Phase B aggregate (this report) | 9 candidates: 1 medium, 8 low/info | mostly hygiene |
| **Total new findings since BUG-001..007 catalog** | **10** (BUG-008 + 9 candidates) | 1 critical, 1 medium, 8 low |

## Recommendations

1. **BUG-008 fix**: in flight (alloy-op-evm/src/aa_precompiles.rs added; image rebuild + T-93/T-94 verification pending)
2. **BUG-CAND-A** (medium): port the 2 missing `load_account_with_code_mut` calls — small focused PR
3. **GAP-CONSENSUS-TESTS** (hygiene): re-enable the 39 disabled tests in a separate cleanup PR
4. **BUG-CAND-009..014**: bundle into a "low-severity port-parity" PR; mostly mechanical
5. Update `~/.claude/skills/eip8130-port-audit/prior-findings.md` with these new candidates so the next audit run dedupes correctly
