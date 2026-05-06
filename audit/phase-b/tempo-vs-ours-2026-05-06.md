# Tempo-vs-Ours EIP-8130 Audit — 2026-05-06

- **OURS**: https://github.com/okx/optimism/tree/feat/eip-8130-port (rust workspace + op-node)
- **REFERENCE**: https://github.com/tempoxyz/tempo (mainnet-deployed, security-hardened reference)
- **Method**: 5 parallel deep-review subagents across asset-safety-critical paths
- **Detail reports**: `/tmp/tempo-vs-ours-{A,B,C,D,E}-*.md`

---

## Executive summary

**12 CRITICAL** asset-safety findings. **17 HIGH**. **20 MEDIUM**. **7 LOW**.

The earlier port audit (BUG-001..008, plus BUG-009/010/011 known) caught file-by-file divergences. This sweep found a **second class of bugs**: cross-cutting state-machine and atomicity gaps where tempo enforces an invariant via a single chokepoint that we replicate at multiple sites — and miss at one or more.

The most dangerous finding cluster is **G-D1/G-D2/G-D3 (phase atomicity)**: config writes, code placements, and account creation persist on phase revert. **An attacker can authorize a new owner via a tx whose phase intentionally reverts; observers see "failed tx" but the attacker now controls the account.** This is a clean asset-drain.

The next most dangerous cluster is **G-E2 (nonce-free replay) + G-E1 (nonce overflow)**: nonce-free replay protection is wired as constants and slot helpers but no callsite reads or writes them. Every nonce-free tx is replayable until expiry.

The third critical cluster is **G-A5 (delegate inner discard)**: when an inner native verifier (K1/P256/WebAuthn) succeeds inside a Delegate envelope, we discard the inner owner_id and unconditionally return the delegate address. If any caller fails to re-validate `(inner_verifier, inner_owner_id) ∈ delegate.authorized_keys`, this is full delegate impersonation.

---

## CRITICAL findings (12)

| ID | Domain | Title |
|---|---|---|
| G-A1 | crypto | P256 raw verifier never enforces low-s — WebAuthn inherits the gap |
| G-A2 | crypto | WebAuthn skips authenticatorData flags entirely (UP/UV/AT/ED) |
| G-A5 | crypto | Delegate native-inner branch discards inner_owner_id, returns delegate unconditionally |
| G-B1 | envelope | encode_2718 / network_encode wrapping divergence vs alloy upstream |
| G-B2 | envelope | tx_hash does not bind a canonical signature digest — defeats `expiringNonceSeen` |
| G-C1 | mempool | AA validation bypasses base-fee + `priority ≤ max` + cumulative-cost gates |
| G-C2 | mempool | `Reorg` notifications silently dropped — orphaned mined AA txs not re-injected |
| G-D1 | handler | Per-phase checkpoints don't have an outer revert boundary on engine errors |
| G-D2 | handler | `config_writes` (owner authorization) applied unconditionally before phase loop, persist on revert |
| G-D3 | handler | `pre_writes` / `code_placements` / `account_creation_logs` applied in caller-deduction phase, survive phase revert |
| G-E1 | state | Nonce increment `current + 1` panics/wraps at `u64::MAX` |
| G-E2 | state | Expiring-nonce replay protection is dead code (slots/constants exist, no callsite uses them) |

### CRITICAL detail (top 3 ranked by exploitability × asset impact)

**G-D2/G-D3 (config writes survive phase revert)** — `op-revm/src/handler.rs:744–776` applies `config_writes`, `sequence_updates`, and `config_change_logs` before the phase loop without any checkpoint covering them. Phase revert at line 855 only rolls back the failing phase. Tempo equivalents at `tempo/crates/revm/src/handler.rs:337,379` use a single outer journal checkpoint. Attack: tx that (a) authorizes attacker as new owner via `account_changes` and (b) intentionally reverts phase 1. Tx reports `tx_succeeded = false`; receipt status reflects failure; the new owner authorization is **on-chain**. Combined with G-D3, attacker can also pre-provision counterfactual accounts via "failed" txs.

**G-E2 (nonce-free replay)** — `storage.rs:115-136` defines `expiring_seen_slot`, `expiring_ring_slot`, `EXPIRING_RING_PTR_SLOT`. `constants.rs:99-109` defines `NONCE_FREE_MAX_EXPIRY_WINDOW = 30` and ring capacity. `validation.rs:159-166` only validates `nonce_sequence==0` and `expiry!=0`. **No callsite reads or writes the seen-set or ring buffer.** Tempo's `tempo/crates/precompiles/src/nonce/mod.rs:134-184` `check_and_mark_expiring_nonce` enforces 4 atomic checks. Attack: nonce-free tx (`nonce_key == U256::MAX`) with 30s expiry is replayable on every block until `now > expiry`; payer drained N times.

**G-A5 (delegate inner discard)** — `native_verifier.rs:529-543`: when inner verifier is K1/P256/WebAuthn and `try_native_verify(...)` succeeds, the code copies `delegate.as_slice()` into `owner_id` (line 533) and discards the actual inner key. The implicit-EOA branch (line 511-527) DOES check signer == delegate; the parallel check is **missing** for native non-EOA inner verifiers. If our caller fails to re-validate `(inner_verifier, inner_owner_id) ∈ delegate.authorized_keys`, **anyone with any P256/WebAuthn key can impersonate any delegate.**

---

## HIGH findings (17)

| ID | Domain | Title |
|---|---|---|
| G-A3 | crypto | WebAuthn JSON parser only checks `challenge` (no `type`/`origin`/`crossOrigin`) |
| G-A4 | crypto | K1 verifier accepts non-canonical s (no low-s enforcement) |
| G-B3 | envelope | `decode_nested_calls` doesn't assert inner-phase consumption (consensus split risk) |
| G-B4 | envelope | `Owner::decode` / `Call::decode` / `OwnerChange::decode` accept trailing bytes inside lists |
| G-B5 | envelope | `Call` missing `value` field — RPC adapter silently drops user-submitted value |
| G-B7 | envelope | `sender_auth` / `payer_auth` length cap enforced at validation, not decode (mempool DoS) |
| G-B8 | envelope | `MAX_CALLS_PER_TX` / `MAX_ACCOUNT_CHANGES_PER_TX` not enforced at decode |
| G-C3 | mempool | No per-tx `gas_limit` cap (tempo enforces 30M); single AA tx can lock pool slot |
| G-C4 | mempool | Per-payer cumulative pending cost not enforced (only count) |
| G-C5 | mempool | Future-nonce AA txs rejected at validation; queue/promotion machinery unreachable |
| G-C6 | mempool | `VerifierPurityCache` unbounded memory growth (no LRU/cap) |
| G-D4 | handler | Sponsored AA tx: tip debited from sender (caller), not payer; payer over-refunded |
| G-D5 | handler | Phase OOG drops accumulated_refunds from prior successful phases |
| G-D6 | handler | Empty `payer_auth` in sponsored mode: payer debited but no auth verified |
| G-D7 | handler | `alloy-op-evm` TxContext precompile lacks AA-tx-type gate |
| G-E3 | state | `nonce_key == 0` not reserved (cross-domain replay vs legacy nonce) |
| G-E4 | state | TOCTOU between `read_nonce` and `increment_nonce_op` |

Plus G-E5 (chain-id binding on multichain auth), G-E6 (read_nonce U256→u64 truncate), G-E7 (nonce_key not u192-bounded) — also HIGH.

---

## MEDIUM findings (20)

`G-A6` WebAuthn length truncation, `G-A7` envelope DoS, `G-A8` delegate cycle via STATICCALL, `G-B6` non-canonical 19/21-byte address, `G-B9` AA_TX_TYPE_ID const_assert, `G-B10` AccountChangeEntry shape, `G-B11` payer-sig sender bind (already fixed — confirm), `G-B12` chain_id u64, `G-B13` nonce_key canonical, `G-C7` aggregate gossip backpressure (`MAX_TEMPO_AUTHORIZATIONS` missing), `G-C8` peer reputation hook on bad-sig AA txs, `G-C9` 7702 chain_id pre-filter, `G-D8` NonceManager static-call enforcement, `G-D9` nonce-free ring eviction atomicity, `G-D10` `state_gas_spent` zeroed on phase revert despite earlier commits (subsidized state pollution), `G-D11` `ACCOUNT_CONFIG_DEPLOYED` AtomicBool reorg unsafety, `G-E8` AccountConfiguration deployment cache process-global, `G-E9` `write_owner_config_op` self-revoke sentinel guard, `G-E10` `unlocks_at = u64::MAX` doesn't round-trip through encoder.

Plus a 20th: G-D6 has a sub-finding about `payer_auth_empty` flag being used for gas estimation but not as a security gate.

---

## LOW findings (7)

`G-A9` JSON parser variable-time, `G-A10` K1 EOA delegate-branch unreachable arm, `G-B14` `hash_slow` re-encodes (perf), `G-B15` expiring_seen_slot keying (subsumed by G-B2 fix), `G-D12` EIP-7623 floor disabled for AA, `G-D13` `getMaxCost()` excludes payer_auth_cost, `G-E11` initial-owner writes blind (no read-before-write at counterfactual addresses).

---

## Reaffirmed equivalences (high-confidence)

From all 5 reports combined:
- secp256k1 ECRECOVER address derivation
- WebAuthn message construction `sha256(authenticatorData || sha256(clientDataJSON))`
- Base64URL-no-padding encoding
- P256 SEC1 public-key parsing (off-curve / identity rejected at library level)
- Domain separation between sender hash (0x7B) and payer hash (0x7C) — already fixed in `91e7606a6f`
- Storage-slot derivation matches Solidity (Foundry fixture passes)
- `effective_salt` order-independent
- Implicit-EOA fallback refuses on `REVOKED_VERIFIER`
- `(sender, nonce_key)` two-keccak collision-resistant path
- Predeploy addresses pinned at `0xaa02` / `0xaa03`
- Per-call `value: U256::ZERO` enforced by compat layer
- Phase ordering breaks on first failure with subsequent phases padded as failed
- Custom verifier dispatched STATICCALL with `is_static: true`
- 2D nonce key check (cold/warm) intrinsic baseline + warm refund
- op-revm internal precompile dispatcher gates on `tx_type == EIP8130_TX_TYPE`

---

## Recommended fix priority

**Tier 1 (must-fix before mainnet of any sort)** — atomicity + replay:
1. **G-D1/D2/D3 bundle** — outer journal checkpoint at top of `execution()`, revert on any non-success outcome. Mirror tempo handler.rs:337,379. ~40 lines.
2. **G-E2** — port `check_and_mark_expiring_nonce` from tempo `precompiles/src/nonce/mod.rs:134-184`, call from `validation.rs:159` for nonce-free txs. ~80 lines.
3. **G-A5** — return `inner_owner_id` from native-inner delegate branch; audit all callers to confirm they re-validate against `delegate.authorized_keys`. ~30 lines + caller audit.
4. **G-D6** — reject sponsored tx with empty `payer_auth` in `validate_env`. ~5 lines.
5. **G-D4** — override `reward_beneficiary` for AA txs to skip duplicate debit. ~20 lines.

**Tier 2 (mempool DoS / asset-safety griefing)**:
6. **G-C1** — add base-fee floor + `priority ≤ max` to AA validation path. ~15 lines.
7. **G-C2** — handle `CanonStateNotification::Reorg` and re-inject orphan AA txs. ~50 lines.
8. **G-C6** — bound `VerifierPurityCache` (LRU 64K, only cache `Pure`). ~20 lines.
9. **G-A1/A4** — add low-s rejection to P256 (covers WebAuthn) + K1 paths. ~10 lines.
10. **G-A2** — read & enforce WebAuthn flags byte (UP||UV required, AT/ED rejected). ~15 lines.
11. **G-E1** — `checked_add(1)` on nonce increment. ~3 lines.
12. **G-B7/B8** — enforce length/count caps at RLP decode. ~20 lines.

**Tier 3 (defense in depth, structural)**:
13. **G-A3** — WebAuthn JSON `type` field check (BUG-010) + crossOrigin handling.
14. **G-B3/B4** — strict decode trailing-byte rejection across all nested decodes.
15. **G-D7** — AA-tx-type gate on alloy-op-evm TxContext precompile.
16. **G-E3** — reject `nonce_key == 0` as reserved.
17. **G-E5** — chain-id binding on `authorizer_auth` for multichain config changes.
18. **G-D11/E8** — replace `ACCOUNT_CONFIG_DEPLOYED` AtomicBool with per-(chain,fork) cache.

**Tier 4 (hygiene, dead-code, perf)**: remaining MEDIUM/LOW items — bundle into one cleanup PR.

---

## Methodology

5 parallel general-purpose subagents, each with a tight scope and asset-safety focus:
- A: signature/crypto verification (`native_verifier.rs` vs `tt_signature.rs`)
- B: tx envelope + RLP codec (`tx.rs/types.rs/abi.rs/storage.rs` vs `envelope.rs/tt_signed.rs/tempo_transaction.rs`)
- C: validation + mempool (`validation.rs/eip8130_validate.rs` vs `validator.rs/tt_2d_pool.rs/tempo_pool.rs`)
- D: revm handler / phase exec (`op-revm/handler.rs/handler_aa_helpers.rs` vs `tempo/revm/*.rs`)
- E: auth + nonce + storage state machine (`accessors.rs/storage.rs/predeploys.rs` vs `precompiles/src/nonce` etc.)

Total wall time ≈ 7 min (longest subagent), vs estimated 4-6 hours sequential. Detail reports preserved at `/tmp/tempo-vs-ours-{A,B,C,D,E}-*.md` for line-level citations.

---

## Status

- [x] All 5 subagents completed
- [x] Findings consolidated (this document)
- [ ] Triage CRITICAL findings against actual call paths (some may be guarded by callers the audit didn't trace)
- [ ] Update `~/.claude/skills/eip8130-port-audit/prior-findings.md` with BUG-012..N entries
- [ ] User decision on fix priority (per prior instruction "bug先不用修，先补充其他测试" — but new asset-drain CRITICALs may shift this)
