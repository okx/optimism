# EIP-8130 Native AA Rust Port Security Review — Codex Contrarian Pass

- **Date**: 2026-05-06
- **Scope**: OUR port at `https://github.com/okx/optimism/tree/cf38ca5666`; BASE upstream at `https://github.com/base/base/tree/a33ab4d09`; Tempo docs at `https://github.com/tempoxyz/tempo/tree/7b619478e0/docs/`
- **Method**: time-boxed source review biased toward cross-file exploit chains, OP Stack integration, fork activation, predeploy deployment, producer/consumer divergence, and WebAuthn/delegation/sponsor edge cases.
- **Constraint**: read-only audit; this markdown report is the only file written.

## TL;DR

**RED / NO-GO for production.** The prior Claude reports already contain multiple production blockers. This contrarian pass found additional OP Stack integration blockers around fork activation, Go derivation, and L1-driven predeploy deployment: Go derivation accepts pre-fork `0x7B` AA transactions that BASE explicitly rejects, Rust can advertise NativeAA-at-genesis while op-node never deploys the AA Solidity contracts for a genesis-active fork, and the NativeAA activation block can carry sequencer batch transactions despite the local sequencer marking it `NoTxPool`. Separately, WebAuthn remains phishable even after Claude's `type`/`UP` fixes because the verifier has no RP ID or origin binding. Treat the Native AA port as not production-ready until the fork-activation contract between op-node derivation, Rust txpool, Rust execution, and deployed bytecode is specified and covered by cross-client tests.

## Top Findings

### CRITICAL

#### C-CODEX-01 — WebAuthn assertions are not bound to RP ID or origin; phishing RP can mint valid AA signatures

**Files:**  
- OUR: `rust/op-alloy/crates/consensus/src/transaction/eip8130/native_verifier.rs:261-275`, `277-284`  
- BASE: `https://github.com/base/base/tree/a33ab4d09/crates/common/consensus/src/transaction/eip8130/native_verifier.rs:261-275`, `277-284`  
- Tempo: `https://github.com/tempoxyz/tempo/tree/7b619478e0/docs/xlayer-native-aa-requirements.md:63`, `https://github.com/tempoxyz/tempo/tree/7b619478e0/docs/native-aa-multi-scheme-comparison.md:23`

**Attacker model:** malicious relying party / phishing site that can ask a victim's passkey for a WebAuthn assertion over an attacker-chosen challenge.

**Issue:** `verify_webauthn` treats `authenticatorData` as opaque bytes and only checks that top-level `clientDataJSON.challenge` equals `base64url(sender_or_payer_hash)`. It does **not** check:
- `authenticatorData[0..32] == sha256(expected_rp_id)`;
- `clientDataJSON.origin` is an allowed wallet/origin;
- `clientDataJSON.crossOrigin == false`.

The prior signature review correctly reported missing `type == "webauthn.get"` and missing `UP`; this is a separate phishing boundary. Even after those two fixes, a malicious RP can perform a valid `webauthn.get` ceremony with `type = "webauthn.get"` and `UP = 1`, using the AA transaction hash as the challenge. Because the protocol has no RP/origin binding, the assertion is accepted on-chain as AA authorization.

**Exploit:**
1. Victim has a WebAuthn owner registered on an AA account.
2. Attacker prepares a drain transaction and computes `sender_signature_hash(tx)`.
3. Attacker-controlled website calls WebAuthn `navigator.credentials.get()` with that hash as the challenge under attacker RP ID/origin.
4. Victim approves a normal-looking passkey prompt for the attacker site.
5. Attacker submits the assertion in `sender_auth`.
6. OUR verifier checks only `challenge` at lines 272-275, hashes the attacker RP's `authenticatorData` at lines 277-282, and verifies the P-256 signature at line 284. No RP/origin mismatch is observable to the protocol, so assets move.

**Fix:** Add explicit WebAuthn policy to the account configuration model. At minimum, store an expected `rp_id_hash` and allowed origin set per WebAuthn owner or per account. In `verify_webauthn`, parse and reject unless `authenticatorData[0..32]` matches the configured RP ID hash, `type == "webauthn.get"`, `crossOrigin == false`, and `origin` is allowed. If the protocol cannot carry this policy, WebAuthn must not be a native verifier for accounts holding real assets.

**Delta vs Claude:** **EXTENDS** C-01/C-02. Claude noted `type` and `UP`; this pass elevates RP/origin binding as an independent asset-theft path.

### HIGH

#### H-CODEX-01 — Go derivation accepts EIP-8130 batch transactions before NativeAA; BASE explicitly rejects them

**Files:**  
- OUR single-batch validation: `op-node/rollup/derive/batches.go:175-188`  
- OUR span-batch validation: `op-node/rollup/derive/batches.go:384-397`  
- OUR Rust txpool gate: `rust/op-reth/crates/txpool/src/validator.rs:267-274`  
- OUR Rust block consensus gate: `rust/op-reth/crates/consensus/src/lib.rs:120-129`  
- BASE single-batch rejection: `https://github.com/base/base/tree/a33ab4d09/crates/consensus/protocol/src/batch/single.rs:176-180`  
- BASE span-batch rejection: `https://github.com/base/base/tree/a33ab4d09/crates/consensus/protocol/src/batch/span.rs:509-517`

**Attacker model:** malicious or faulty sequencer posts batch data containing raw type `0x7B` EIP-8130 transactions before `NativeAA`.

**Issue:** The Rust txpool and Rust consensus layer reject AA txs before NativeAA, but the OP Stack Go derivation batch validator does not. In `batches.go`, the only typed transaction fork check is `SetCodeTxType` before Isthmus. BASE's upstream protocol explicitly drops EIP-8130 transactions before BASE_V1 in both single and span batches.

**Exploit:**
1. NativeAA is scheduled for timestamp `T`, current safe derivation is at `< T`.
2. Sequencer posts a valid-looking batch whose transaction list contains one `0x7B` AA transaction.
3. Go derivation accepts the batch because lines 175-188 / 384-397 do not reject `0x7B` pre-fork.
4. Downstream Rust execution/consensus rejects the block at `validator.rs:267-274`, `consensus/src/lib.rs:120-129`, or `handler.rs:102-109`.
5. Result: derivation/EL disagreement or safe-head stall. In mixed EL deployments, an EL that decodes/handles `0x7B` differently can diverge from Rust nodes.

**Fix:** Add an explicit `NativeAA` batch validity rule in both single and span paths:
`if !cfg.IsNativeAA(blockTimestamp) && txBytes[0] == 0x7B { return BatchDrop }`.
Add Go tests equivalent to BASE's `test_check_batch_drop_8130_pre_base_v1` and span-batch pre-fork test. Use a named transaction type constant rather than magic `0x7B`.

**Delta vs Claude:** **UNIQUE.** Prior reports focused on Rust txpool/execution; this is an OP Stack Go derivation mismatch.

#### H-CODEX-02 — NativeAA-at-genesis is advertised by Rust but op-node never deploys AA Solidity contracts for genesis-active forks

**Files:**  
- OUR activation predicate: `op-node/rollup/types.go:595-599`  
- OUR upgrade tx injection: `op-node/rollup/derive/attributes.go:187-198`  
- OUR Rust chain spec builder: `rust/op-reth/crates/chainspec/src/lib.rs:217-221`  
- OUR Rust native precompile stubs only: `rust/alloy-op-evm/src/block/native_aa.rs:13-18`, `37-45`, `63-74`  
- OUR deployed contract constants/comments: `rust/op-alloy/crates/consensus/src/transaction/eip8130/predeploys.rs:15-17`, `76-124`  
- BASE activation comparison: `https://github.com/base/base/tree/a33ab4d09/crates/consensus/derive/src/attributes/stateful.rs:165-168`

**Attacker model:** chain operator or deployment tooling configures `native_aa_time` at genesis, which the Rust chain spec explicitly supports; users then submit AA transactions on a chain whose AA contracts were never deployed.

**Issue:** `IsNativeAAActivationBlock` returns true only when the current block is active and the parent timestamp was inactive (`l2BlockTime >= BlockTime && !IsNativeAA(l2BlockTime-BlockTime)`). If NativeAA is active at genesis, the first derived block's parent is already NativeAA-active, so op-node does **not** inject `NativeAANetworkUpgradeTransactions` at `attributes.go:191-198`. Rust exposes `native_aa_activated()` at genesis, and `ensure_aa_predeploys` only force-deploys `0xaa02` / `0xaa03` stubs; it explicitly does not deploy `AccountConfiguration`, `DefaultAccount`, or verifiers.

This contradicts the Rust predeploy doc comment saying genesis-active devnets get upgrade deposits at block 0. The actual Go activation predicate skips genesis activation.

**Exploit / failure sequence:**
1. Devnet or production chain sets `native_aa_time = genesis_time` / `0`.
2. Rust execution enables NativeAA at timestamp 0 (`native_aa_activated`, `is_native_aa_active_at_timestamp`).
3. op-node never injects the six deployment deposits because `IsNativeAAActivationBlock` is false for the first derived block.
4. `ensure_aa_predeploys` installs only `0xfe` stubs at `0xaa02` and `0xaa03`; the real Solidity contracts at `ACCOUNT_CONFIG_ADDRESS`, `P256_*`, `DELEGATE`, and `DEFAULT_ACCOUNT` remain empty.
5. AA behavior is split: simple implicit-EOA flows may appear partially alive, config changes are rejected as `AccountConfigNotDeployed`, custom verifier calls hit empty accounts, and wallets can make irreversible assumptions about owner configuration that never exists on-chain.

**Fix:** Pick one invariant and enforce it everywhere:
- genesis-active NativeAA is unsupported: reject `native_aa_time <= genesis_time` in rollup config / genesis parsing; or
- genesis-active NativeAA is supported: include the six deployed contract bytecodes in genesis alloc, or make op-node inject the upgrade deposits in the first derived block even when the fork was active in genesis.

Add a cross-client test that sets NativeAA at genesis and asserts bytecode exists at all six deployed addresses plus stubs at `0xaa02`/`0xaa03`.

**Delta vs Claude:** **UNIQUE / DISAGREES** with the earlier broad claim that hardfork activation race was secure. That claim did not cover genesis-active NativeAA.

### MEDIUM

#### M-CODEX-01 — NativeAA activation block can carry sequencer batch txs despite sequencing code marking it `NoTxPool`

**Files:**  
- OUR sequencer side: `op-node/rollup/sequencing/sequencer.go:602-605`  
- OUR derivation batch side: `op-node/rollup/derive/batches.go:136-143`  
- OUR upgrade injection/gas extension: `op-node/rollup/derive/attributes.go:187-198`, `223-231`

**Attacker model:** malicious sequencer ignores the local `NoTxPool` setting and submits batch data for the NativeAA activation block.

**Issue:** The sequencer implementation correctly sets `NoTxPool = true` for the NativeAA activation block, but derivation validity only drops non-empty activation batches for Jovian, Karst, and Interop. NativeAA is omitted from the guard at `batches.go:136-143`, even though `attributes.go` adds six upgrade deposits and extends the gas limit for this block.

**Exploit / impact:**
1. NativeAA activates at timestamp `T`.
2. Honest sequencing path would produce only L1 info, deposits, and NativeAA upgrade txs.
3. Malicious sequencer posts a non-empty batch for `T`.
4. Derivation accepts it; user txs execute in the same block as predeploy deployment, despite the local sequencing path forbidding that condition.

This is a centralization assumption leak. It may not directly steal funds because upgrade deposits are prepended before batch txs, but it invalidates the "empty upgrade block" invariant and enables first-block MEV against newly deployed AA contracts.

**Fix:** Add `cfg.IsNativeAAActivationBlock(batch.Timestamp)` to the non-empty activation-block drop rule in both single and span batch validation. Add a regression test mirroring the sequencer `NoTxPool` rule.

**Delta vs Claude:** **UNIQUE.** This is Go derivation / sequencing policy, not Rust execution.

### LOW

#### L-CODEX-01 — HYPOTHESIS: `alloy-op-evm` can compile without its local `native-verifier` feature, causing zero owner IDs in execution helpers

**Files:**  
- OUR feature fallback: `rust/alloy-op-evm/src/eip8130_compat.rs:77-80`, `125-128`  
- OUR feature declaration: `rust/alloy-op-evm/Cargo.toml:37-64`  
- OUR owner-id use: `rust/alloy-op-evm/src/eip8130_compat.rs:323-324`, `479-484`  
- OUR execution validation: `rust/op-revm/src/handler.rs:711-740`

**HYPOTHESIS:** The main op-reth node build likely enables `alloy-op-evm/native-verifier` through the txpool dependency, so this may not affect the production binary. I did not have enough time to prove every binary feature graph.

**Risk:** When the local `native-verifier` feature is disabled, `derive_sender_owner_id` and `derive_payer_owner_id` return `B256::ZERO`. The handler later uses those IDs for native verifier owner-config checks. This is probably a liveness failure for normal accounts, but if an account has authorized `owner_id == 0x00..00`, malformed native-auth transactions could be validated against that zero owner instead of the actual recovered key in feature-minimal execution builds.

**Fix:** Remove the non-verifying fallback for execution builds. Make `alloy-op-evm`'s default or required features include `native-verifier`, or make EIP-8130 transaction conversion return a hard error when native verification is unavailable. Also reject authorizing `owner_id == B256::ZERO` unless the spec explicitly reserves it.

**Delta vs Claude:** **UNIQUE HYPOTHESIS.** Keep this as a build-graph audit item, not a confirmed production exploit.

## Cross-Check vs Claude

| Claude finding | Verdict | Notes |
|---|---|---|
| C-01 WebAuthn type not checked | AGREE | Prior report is well anchored; not duplicated below unless extended. |
| C-02 WebAuthn UP flag not checked | EXTEND | C-CODEX-01 adds missing RP ID, origin, and crossOrigin binding. |
| H-01 config_change_digest no EIP-712 domain | AGREE | Production blocker unless spec intentionally accepts bare struct hashes. |
| H-02 Delegate scheme discards inner owner_id | AGREE | Also related to delegate-chain/cycle concerns; see Tempo gaps. |
| H-03 nonce-free max expiry unenforced | AGREE | Production blocker for replay/liveness assumptions. |
| H-04 payer_signature_hash wire-incompat | AGREE | Must be resolved with Solidity/base vectors before launch. |
| M-04 P-256 high-s malleability | AGREE | Medium unless tx-hash uniqueness is part of protocol accounting. |
| AUTH-001 REVOKED_VERIFIER accepted by helper | AGREE | Latent API trap; less direct than admission/execution paths. |
| AUTH-002 dual ACCOUNT_CONFIG_DEPLOYED atomics | EXTEND | The deeper issue is process-global deployment caching versus fork/genesis state transitions. |
| AUTH-003 code_placements no collision guard | AGREE | Practical exploit requires CREATE2 collision/race, but invariant should be explicit. |
| AUTH-006 last-key self-revocation bricks account | AGREE | Spec-level production footgun. |
| HIGH-NR-001 reorg drops nonce-free txs | AGREE | Liveness blocker for short-expiry nonce-free UX. |
| HIGH-NR-002 slot_to_seq `or_insert` orphaning | AGREE WITH SEVERITY CAVEAT | Keccak collision angle is not credible; replacement/removal invariant still deserves tests. |
| MED-NR-003 nonce-free dedup excludes payer_auth | AGREE | DoS/sponsor race, not direct asset theft. |
| MED-NR-004 u64 nonce wrap | AGREE | Correctness bug; practically unreachable but cheap to fix. |
| H-001 exact equality AA precompile gating | AGREE | Prior finding covers `aa_precompiles.rs`; this pass focuses on additional fork-boundary surfaces. |
| H-002 payer revoke timing | AGREE WITH CAVEAT | Needs a concrete exploit proof; still a design assumption to document/test. |
| M-002 delegation gas underpriced | AGREE | Accounting bug. |
| M-003 estimation calldata gas underestimate | AGREE | UX/liveness risk for custom verifiers. |
| BUG-001..008 resolved | AGREE | Not re-reviewed exhaustively. |
| MEDIUM-003 catch_error thread-local stale | AGREE | Low direct exploitability today. |
| Claude "hardfork activation race secure" observation | DISAGREE | H-CODEX-01/H-CODEX-02/M-CODEX-01 show Go derivation and genesis activation gaps not covered by Rust-only checks. |

## Tempo Design Gaps

- Tempo says WebAuthn support requires protocol-level `clientDataJSON` and `authenticatorData` parsing (`xlayer-native-aa-requirements.md:63`, `native-aa-multi-scheme-comparison.md:23`). The Rust verifier parses only enough JSON to find `challenge` and treats `authenticatorData` as opaque bytes. Missing RP ID/origin/crossOrigin/type/flags policy is a design gap, not just an implementation bug.
- Tempo docs reviewed in this pass did not define a cross-client fork activation invariant for NativeAA: exact activation block, genesis-active behavior, required predeploy bytecode hashes, and whether user batch txs are forbidden in the upgrade block. The code currently spreads that invariant across `op-node`, `alloy-op-evm`, `op-reth` txpool, and `op-revm`.
- Tempo's sponsor/passkey flow examples focus on the happy path and do not specify phishing-origin rejection for passkeys or failed-user-op versus sponsor-fee atomicity. The prior gas/sponsor report covers payer timing; C-CODEX-01 covers the missing WebAuthn boundary.

## Verdict

**RED.** Production blockers include the prior CRITICAL/HIGH signature and replay issues plus C-CODEX-01, H-CODEX-01, and H-CODEX-02. Do not ship to mainnet with real assets until there are cross-client fork-activation tests, deployed-bytecode hash checks, WebAuthn RP/origin policy, and one canonical set of EIP-8130 signing/predeploy vectors shared by Rust, Go derivation, Solidity, and BASE upstream.
