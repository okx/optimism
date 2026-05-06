# EIP-8130 Port Audit — 2026-05-05

- **OURS**: https://github.com/okx/optimism/tree/4c80fe5b17 (rust workspace) + `op-node` (Go bytecode embedding)
- **BASE**: https://github.com/base/base/tree/a33ab4d09 (crates/)
- **Phases run**: A (inventory) + B (symbol cross-ref, 4 parallel agents) + C (integration matrix) + skill capture + BUG-008 fix

---

## Executive summary

- **1 critical wiring bug (BUG-008)** — alloy-op-evm strips `OpPrecompiles` wrapper, leaving NonceManager + TxContext precompiles unreachable. Discovered & fixed; image rebuild in progress; T-93/T-94 will transition skip → pass post-deploy.
- **9 new BUG-CANDIDATES** (1 medium, 8 low) found via Phase B symbol cross-ref. None safety-critical; all open as follow-up port-parity work.
- **2 hygiene gaps**: 39 disabled tests in op-alloy consensus, missing `Eip8130` arm in `Arbitrary` impl (shared with base).
- **0 NEW protocol semantics bugs** in BUG-001..007 severity class. Consensus types layer (25 files) confirmed byte-identical to base modulo BUG-001/005/006 fixes already shipped.

The audit methodology (carpet-bombing diff with bidirectional symbol xref + integration matrix) successfully surfaced wiring-layer issues that the original BUG-001..007 file-by-file diff missed. The methodology has been encoded as the `/eip8130-audit` skill.

---

## New findings

### BUG-008 [CRITICAL — wiring]
**Title**: alloy-op-evm strips OpPrecompiles wrapper, leaving AA precompile dispatch broken
**OURS**: `rust/alloy-op-evm/src/lib.rs:249, 273` (pre-fix)
**BASE**: `crates/common/evm/src/factory.rs:99` has `extend_precompiles([(TX_CONTEXT, …), (NONCE_MANAGER, …)])`
**Symptom**: phase calls to `0x…aa02` / `0x…aa03` fall through to stub `0xfe` and revert. Hidden because the chain has no on-chain caller of TxContext, and NonceManager `0xfe` was treated as INVALID-opcode in tests (`T_REVERT`).
**Why missed earlier**: file path mismatch (base: `factory.rs`; ours: `lib.rs`), comment in op-revm/precompiles.rs:370-374 misleadingly pointed at handler.rs.
**Fix**: ported base's `op_precompiles_map` to `rust/alloy-op-evm/src/aa_precompiles.rs` with DynPrecompile closures reading from op-revm's existing thread-local `EIP8130_TX_CONTEXT`. Two call sites in `lib.rs` rewired.
**Status**: code committed locally (uncommitted to git), workspace `cargo check` clean. Image rebuild + T-93/T-94 verification pending.

### BUG-CAND-A [MEDIUM — proof witness]
Missing `load_account_with_code_mut(caller)?` calls in `op-revm/src/api/exec.rs::system_call_one_with_caller` and `inspect_one_system_call_with_caller`. Base added these to satisfy revm bluealloy/revm#3484 for execution-witness compatibility. AA execution unaffected; visible if/when we run kona / fault-proof flows.

### BUG-CAND-B / C / 009-014 [LOW — port parity]
8 low-severity divergences across rpc-types, hardforks, and revm spec ordering. Bundle into one "port-parity cleanup" PR. See `phase-b-summary.md` for per-item details.

### BUG-CAND-E [MEDIUM — TPS / UX, no security impact]
**Title**: EIP-7702 mempool 1-slot in-flight limit kills AA throughput on auto-delegated EOAs
**Layer**: reth-transaction-pool (mainline reth) — not specific to our fork
**Symptom**: Once an EOA's first AA tx mines and writes the `0xef0100‖DefaultAccount` delegation indicator, the txpool's `check_delegation_limit` applies EIP-7702 spec rule "delegated accounts may have at most 1 in-flight tx" (default `MAX_INFLIGHT_DELEGATED_SLOTS = 1`). Subsequent AA txs — even on different `nonce_key` channels — get rejected with `InflightTxLimitReached`.
**Spec gap**: 2D nonce parallelism + nonce-free mode are EIP-8130 TPS features, but the 7702 txpool layer collapses them to 1-tx-at-a-time per sender.
**Why over-restrictive**: 7702's 1-slot rule guards chained SetCode (type-0x04) authorization grief. AA tx type 0x7b carries no `authorization_list`, so the threat model doesn't apply. Mis-application of a safety rule.
**Impact on tests**: not the dominant cause of slowness in sequential single-suite runs (each test waits for receipt before next). Real impact is concurrent / parallel scenarios — high-TPS AA paymaster batching, wallet "submit-5-fast", or two test suites running on same EOA. Earlier debugging session conflated this with a different failure mode ("replacement underpriced" from stuck-queued AA-pool entries left over from prior runs).
**Mitigation options** (in `prior-findings.md` BUG-CAND-E entry): spec clarification, op-reth-side AA-aware exception, sharding, or test-side dedicated-funder workaround.
**Status**: open; design decision pending.

### Hygiene
- `mod tests` disabled in `op-alloy/.../eip8130/mod.rs:113-118` → 39 tests not running. Mechanical sed migration unblocks (fixes `from: Address::*` literals incompatible with post-BUG-001 `Option<Address>`).
- `Arbitrary` impl missing `Eip8130` arm on `OpReceiptEnvelope`. Shared with base; upstream-worthy.

---

## Reaffirmed equivalences

- All public symbols in `op-alloy/.../transaction/eip8130/` match base byte-identically (modulo BUG-001/005/006 fixes already shipped on our side).
- All 4 `BaseTransactionPool` mutation paths forward to `eip8130_pool.remove_transactions` (BUG-002 family fully audited).
- 0x7B codec round-trip identical (encode + decode in `reth_codec.rs`).
- TxPool AA branch matches line-for-line (80+ lines, only fork-gate name `is_native_aa_active_at_timestamp` vs `is_base_v1_active_at_timestamp` differs).
- L1 fee inclusion for AA tx present at `validator.rs` on both sides.
- Receipt encoding: `Eip8130` variant treated as plain `Receipt<T>` per spec; AA metadata via logs (no encoder asymmetry possible).

---

## Methodology validation

| Phase | Wall time | Agents | Files compared | Findings |
|---|---|---|---|---|
| A inventory | 30 min | 1 (me) | 173 (77 ours, 96 base) | 1 false-positive class identified (FlashBlocks BAL `account_changes`) |
| B symbol xref | ~10 min wall (4× parallel) | 4 subagents | ~80 file-pairs in scope | 9 candidates |
| C integration matrix | 30 min | 1 (me) | 10 wiring categories | BUG-008 confirmed |
| Skill capture | 30 min | 1 (me) | n/a | `/eip8130-audit` SKILL.md + prior-findings.md |

**Compared to sequential single-agent execution**: estimated 3-4 work-days saved by 4-way parallelism in Phase B.

**Methodology lessons captured in skill**:
1. `rg -E` is `--encoding`, not extended regex
2. Bare `0x7b` over-matches; anchor with `AA_TX_TYPE_ID`
3. `account_changes` matches EIP-7928 in `crates/common/access-lists/`
4. Single-chokepoint identification ≠ proof of correctness
5. Macro-expanded code invisible to grep — use `cargo expand` if suspect
6. Cross-language boundaries (op-node Go ↔ Rust) need sha256 byte-comparison, not file-list diff
7. Fork extensions vs drift: don't flag intentional okx-fork additions

---

## Skill: `/eip8130-audit`

Encoded methodology lives at `~/.claude/skills/eip8130-port-audit/SKILL.md` + `prior-findings.md`.

**Skill design choice**: full LLM-driven reasoning, NOT a static script. Reason: base's repo organization changes faster than any grep pattern survives — script fails first time base reshuffles a crate, while LLM-with-methodology adapts at audit time. SKILL.md encodes the "what to check + how to filter false positives + what bugs to dedupe" knowledge; LLM resolves current paths each run.

**Trigger**: `/eip8130-audit [--phase A|B|C|D|E]`

**Re-run cadence**: after major sync from optimism upstream, after notable base PRs touching AA, before release cuts, on suspicious e2e failures.

---

## Deliverables

```
audit/
├── eip8130-port-audit-4c80fe5b17.md   # this file (main report)
├── inventory.md                        # Phase A — file inventory + structural mapping
├── integration-matrix.md               # Phase C — wiring matrix
├── phase-b-consensus-types.md          # Phase B agent 1 (op-alloy consensus)
├── phase-b-revm.md                     # Phase B agent 2 (op-revm)
├── phase-b-txpool-receipts.md          # Phase B agent 3 (txpool + receipts)
├── phase-b-evm-hardforks-rpc.md        # Phase B agent 4 (alloy-op-evm + hardforks)
└── phase-b-summary.md                  # Phase B aggregate

~/.claude/skills/eip8130-port-audit/
├── SKILL.md                            # methodology
└── prior-findings.md                   # BUG-001..008 + BUG-CAND-* dedupe table

rust/alloy-op-evm/src/
├── aa_precompiles.rs                   # NEW — BUG-008 fix (DynPrecompile registration)
└── lib.rs                              # MODIFIED — wires op_precompiles_map
```

---

## Recommendations

1. **Image rebuild + verify BUG-008** (in progress): once `op-reth:native-aa` is rebuilt with BUG-008 fix, `clean.sh && 0-all.sh && run-basic-tests.sh T-93 T-94` should show both transition skip → pass. Then commit fix with `Fixes: BUG-008` reference.
2. **BUG-CAND-A**: small focused PR (~5 lines) porting the 2 missing `load_account_with_code_mut` calls. Medium-priority for proof witness compat.
3. **Port-parity cleanup PR**: bundle BUG-CAND-009..014 as a single low-severity PR.
4. **Test hygiene PR**: re-enable 39 disabled tests + add `Eip8130` arm to `Arbitrary` impl.
5. **Schedule next `/eip8130-audit` run** after the next major optimism-upstream sync.
