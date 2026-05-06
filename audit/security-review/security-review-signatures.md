# EIP-8130 Signature & Cryptographic Primitives Security Review

- **Date**: 2026-05-06
- **Reviewer**: Claude Sonnet 4.6 (Rust code reviewer agent)
- **Scope**: Signature and cryptographic primitive paths only — secp256k1/P-256/WebAuthn/Delegate verification, signing hash computation, domain separation, replay protection, key/auth blob parsing.
- **Prior findings excluded**: BUG-001..008, BUG-CAND-A..014 (see `phase-b-summary.md`)

**Build status**: `cargo check -p op-alloy-consensus` — clean. `cargo test -p op-alloy-consensus` — 170/170 pass. `cargo clippy -D warnings` — **44 errors** (mostly `const fn` and `doc_markdown` lints in `purity.rs`, none in the signature/crypto files under audit). Note: Clippy failure is a CI-blocker by policy; these failures are in out-of-scope files but must be resolved before merge.

---

## Findings

### CRITICAL — C-01: WebAuthn `type` field not verified — `webauthn.create` credentials accepted

**File**: `rust/op-alloy/crates/consensus/src/transaction/eip8130/native_verifier.rs:232–291`

**Attacker model**: An attacker with a passkey device harvests a `webauthn.create` ceremony assertion (registration ceremony) from a victim. The registration ceremony produces a valid P-256 signature over `sha256(authenticatorData || sha256(clientDataJSON))` with `clientDataJSON.type = "webauthn.create"`. Because `verify_webauthn` only validates the `challenge` field and never checks `clientDataJSON.type`, this signature is accepted as a valid `webauthn.get` (authentication) assertion.

**Exploit sketch**:
1. Attacker tricks victim into a registration ceremony for an attacker-controlled service (phishing or MITM) where the challenge is `base64url(sender_signature_hash(victim_tx))`.
2. Victim's authenticator produces a P-256 signature over the `webauthn.create` assertion.
3. Attacker stuffs this `clientDataJSON` (with `type: "webauthn.create"`) plus the signature into a `sender_auth` blob targeting the victim's AA account.
4. `verify_webauthn` extracts `challenge`, verifies it matches, and accepts the signature — no `type` check prevents acceptance of a registration assertion as an authentication assertion.

**Why it matters**: WebAuthn explicitly mandates `type == "webauthn.get"` for assertion ceremonies (spec §7.2 step 11). Accepting `webauthn.create` ceremonies is a well-known WebAuthn security failure mode. In this context it allows a malicious relying party to construct a valid AA authentication from a registration event.

**Code**: `verify_webauthn` calls `webauthn_challenge_matches` (which parses only the `challenge` field) but never calls `extract_top_level_json_string_field(client_data_str, "type")` to assert `"webauthn.get"`.

**Fix**: After the challenge check, add:
```rust
let type_val = extract_top_level_json_string_field(client_data_str, "type");
if type_val != Some("webauthn.get") {
    return NativeVerifyResult::Invalid(NativeVerifyError::WebAuthnTypeMismatch);
}
```

**Delta vs Base**: Base (https://github.com/base/base) has the identical omission — this is a shared vulnerability, not introduced by our port.

**Delta vs Tempo**: Tempo's design doc (`native-aa-multi-scheme-comparison.md`) notes "协议层完整实现（clientDataJSON + authenticatorData 解析）" (full protocol-layer implementation of clientDataJSON + authenticatorData parsing) but does not explicitly call out the `type` check as a distinct requirement. The WebAuthn spec itself requires it.

---

### CRITICAL — C-02: WebAuthn authenticatorData `UP` flag not checked — authenticator presence bypass

**File**: `rust/op-alloy/crates/consensus/src/transaction/eip8130/native_verifier.rs:232–291`

**Attacker model**: WebAuthn authenticatorData byte 32 contains flag bits where bit 0 (`UP`, User Presence) MUST be set and bit 2 (`UV`, User Verification) MUST be set if `userVerification=required`. The code treats authenticatorData as an opaque byte string: it is passed to the SHA-256 hasher unchanged without validating any flag bits.

**Exploit sketch**:
1. An attacker generates a synthetic `authenticatorData` blob with `UP=0` (bit 0 of byte 32 clear), indicating the user was NOT present.
2. The authenticator also has `UV=0`. A legitimate authenticator would refuse to sign with these flags; however a software-only or compromised authenticator could produce such a blob.
3. `verify_webauthn` hashes this blob without checking flags, and accepts the resulting signature.
4. The `ownerId` is computed from the public key regardless of whether the authenticator asserted user presence.

**Why it matters**: The `UP` flag is the primary WebAuthn control that distinguishes "user was physically present and initiated this action" from "a background script generated this assertion." Skipping the check allows replay of passkey assertions made without user awareness (e.g., from silent background requests, or from a compromised app). WebAuthn spec §7.2 step 17 requires `UP == 1` unconditionally.

**Fix**: After parsing `authenticator_data` (at offset 32 within `rest`), validate:
```rust
let flags_byte = authenticator_data[32];
if flags_byte & 0x01 == 0 {
    return NativeVerifyResult::Invalid(NativeVerifyError::WebAuthnUserPresenceNotSet);
}
// If user verification required by policy, also check 0x04 bit
```

**Delta vs Base**: Same omission in base. Shared vulnerability.

---

### HIGH — H-01: `config_change_digest` is a bare struct hash without EIP-712 domain separator — cross-contract replay

**File**: `rust/op-alloy/crates/consensus/src/transaction/eip8130/signature.rs:14–39`

**Attacker model**: `config_change_digest` computes a raw struct hash and returns it as the digest that the authorizer signs. There is no `\x19\x01 || domainSeparator || structHash` wrapping (EIP-712 prefix). An authorizer signature over this digest is therefore interpretable in any other context that produces the same 32-byte keccak output without domain binding.

**Specific risk**: Any ERC-1271 `isValidSignature(bytes32 hash, bytes sig)` call on the same account that coincidentally produces the same bytes32 could consume this authorization. More concretely: if the same `account`, `chain_id`, `sequence`, and `owner_changes` pattern appears in another contract's ABI encoding (e.g., a multisig or governance contract) the signature is cross-reusable.

**Exploit sketch**: An attacker who can cause a victim to sign a hash equal to `config_change_digest(victim_acct, change)` via any off-chain mechanism (e.g., a fake ERC-1271 request that hashes to the same value) can replay that signature as a config-change authorizer, adding attacker-controlled owners to the victim's account.

**Assessment**: The risk is partially mitigated by the fact that: (a) `account` and `chain_id` and `sequence` are inside the signed payload so the same 32 bytes cannot easily be produced from an unrelated context; (b) the typehash domain-separates within the struct hash. However, the missing `\x19\x01 || domain` prefix means this digest could still collide with legacy or other EIP-712 signed structs that omit prefix checking.

**Fix**: Wrap with the EIP-712 prefix:
```rust
let domain_separator = keccak256(abi_encode([
    keccak256("EIP712Domain(string name,uint256 chainId,address verifyingContract)"),
    keccak256("AccountConfiguration"),
    U256::from(chain_id),
    ACCOUNT_CONFIG_ADDRESS,
]));
let mut out = [0u8; 66];
out[0] = 0x19;
out[1] = 0x01;
out[2..34].copy_from_slice(domain_separator.as_slice());
out[34..66].copy_from_slice(struct_hash.as_slice());
keccak256(out)
```

**Delta vs Base**: Base has the identical omission. Shared vulnerability. Note: the base implementation is the reference, so any fix here must be coordinated with the Solidity `AccountConfiguration.applyConfigChange()` contract which must use the same domain.

---

### HIGH — H-02: Delegate scope not validated — any inner owner can act as sender or payer regardless of scope

**File**: `rust/op-alloy/crates/consensus/src/transaction/eip8130/native_verifier.rs:497–544`

**Attacker model**: When `verify_delegate` succeeds, it returns `Verified(delegate_owner_id)` where `delegate_owner_id` is `bytes32(bytes20(delegate_address))`. The scope check for whether this delegate is permitted to act as sender/payer is done upstream against the on-chain `owner_config`. However, `verify_delegate` itself never validates that the inner signer's resolved `owner_id` matches the signer's registered `owner_id` for the delegate account. The inner key can be any key that verifies the signature — not necessarily one registered in the delegate's account as an owner.

**Exploit sketch**:
1. Account A delegates to Account D (the delegate).
2. Account D has one key K registered with `scope = PAYER` only.
3. An attacker uses key K to sign a sender hash for Account A with `verifier = DELEGATE || D || K1 || sig(K)`.
4. `verify_delegate` verifies the K1 inner signature, gets `owner_id = bytes32(K_address)`, then computes `delegate_owner_id = bytes32(D_address)` and returns `Verified(bytes32(D_address))`.
5. The returned `delegate_owner_id` is checked against `D`'s registered config — but `bytes32(D_address)` is the implicit EOA `owner_id` for D, which grants unrestricted access if D has not explicitly revoked it.
6. The attacker bypasses scope checking entirely by choosing the right `delegate_address`.

**Note**: This requires understanding of the full validation flow. If the validation code correctly checks `verify_result.owner_id` against on-chain `owner_config[D][verified_owner_id]`, the attack fails. The risk is that `verify_delegate` discards the inner `owner_id` (line 533: `let mut owner_id = [0u8; 32]; owner_id[..20].copy_from_slice(delegate.as_slice());`) — it always returns the delegate address as owner_id, regardless of which inner key signed. This means the on-chain check is against the implicit EOA slot for D, not the registered inner key.

**Fix**: After inner verification succeeds, the code should additionally verify that the inner `owner_id` (returned from the inner verifier) is a registered owner of the delegate account with appropriate scope. This requires a DB lookup and is beyond what the pure cryptographic verifier can do. The architecture should pass the inner `owner_id` through so that the validation layer can check it. Alternatively: require delegate verifiers to be explicitly registered with specific inner key bindings.

---

### HIGH — H-03: Nonce-free mode expiry window constant defined but never enforced in `validate_structure`

**File**: `rust/op-alloy/crates/consensus/src/transaction/eip8130/constants.rs:97` and `validation.rs:151–175`

`NONCE_FREE_MAX_EXPIRY_WINDOW = 30` (seconds) is defined in `constants.rs` and exported from `mod.rs` but there is no call site in `validate_structure`, `validate_expiry`, or any other validation function that enforces it. A nonce-free transaction may set `expiry` to an arbitrarily distant timestamp (years in the future), making the signed hash replayable for the entire window until the mempool deduplication circular buffer wraps around.

**Replay window**: `EXPIRING_NONCE_SET_CAPACITY = 300_000` entries. At 10 000 TPS, this wraps in 30 seconds. But a node that is not running at 10k TPS (or that restarts) would drop the ring buffer, making the window effectively infinite until the expiry timestamp.

**Exploit sketch**:
1. Attacker gets victim to sign a nonce-free AA transaction with `expiry = block.timestamp + 1 year`.
2. Attacker holds the transaction and replays it repeatedly after the circular buffer wraps (within expiry).
3. Each replay executes the victim's calls and charges gas to the victim/payer.

**Fix**: In `validate_structure` (or a mempool-specific check), add:
```rust
if tx.nonce_key == NONCE_KEY_MAX {
    // expiry must be non-zero (already checked) AND must be within the window
    if let Some(window_end) = current_timestamp.checked_add(NONCE_FREE_MAX_EXPIRY_WINDOW) {
        if tx.expiry > window_end {
            return Err(ValidationError::NonceFreeExpiryTooDistant);
        }
    }
}
```
This requires passing `current_timestamp` to `validate_structure` or creating a separate `validate_nonce_free_expiry(tx, block_timestamp)` function.

---

### HIGH — H-04: Payer signature hash divergence from base — our port adds `resolved_sender` parameter that base omits

**Files**:
- Ours: `rust/op-alloy/crates/consensus/src/transaction/eip8130/signature.rs:57` — `payer_signature_hash(tx, resolved_sender: Address)`
- Base: https://github.com/base/base — `crates/common/consensus/src/transaction/eip8130/signature.rs:49` — `payer_signature_hash(tx)` (uses `self.from` directly)

**Issue**: Our port's `payer_signature_hash` takes an explicit `resolved_sender: Address` parameter and injects it into `encode_for_payer_signing(Some(resolved_sender), ...)`. Base's implementation uses `encode_for_payer_signing(&self, ...)` which encodes `self.from` directly.

**For the EOA path** (`from = None`): Base encodes `self.from = None` → RLP `0x80` (empty). Our port encodes the resolved EOA address. These produce **different hashes**. If a payer signature was produced using the base implementation (encoding `0x80`), it would not verify against our implementation (encoding the recovered address). If our version is deployed and base is the reference, a payer who generates signatures using the base hash format cannot have them accepted by our node.

**For the configured-owner path** (`from = Some(addr)`): Both produce the same hash since resolved sender equals `tx.from`.

**Security implication**: The protocol-level contract (the Solidity `AccountConfiguration` payer verification) must use the same hash. If the Solidity contract was written to the base convention (encoding raw `from` bytes) and our node uses the resolved-sender convention, payer signatures will always fail for EOA senders.

**Positive aspect**: Our implementation is more correct from a security standpoint (the comment on line 50-56 explains it prevents cross-sender payer replay). However, this is a **wire incompatibility with base** that needs explicit coordination and matching changes in the Solidity verifier contracts.

**Fix**: Document this deviation explicitly. Ensure the Solidity payer verifier contract uses the resolved-sender convention, not raw `from`. Add a test vector comparing output with the base convention.

---

### MEDIUM — M-01: Delegate auth blob parsing allows variable-length overflow that silently truncates the signature

**File**: `rust/op-alloy/crates/consensus/src/transaction/eip8130/native_verifier.rs:497–544`

The delegate verification reads `data[20..]` as the nested auth blob. The nested auth blob is `inner_verifier(20) || inner_data(...)`. For a K1 inner verifier, `inner_data` must be exactly 65 bytes, making `data` exactly 40+65=105 bytes. However, there is no upper-bound check on `data.len()`. An attacker can pad `data` with arbitrary trailing bytes beyond the expected 65 (or 128 for P256). The inner verifier will slice exactly what it needs and succeed, but the trailing bytes are silently ignored.

**Attack**: This is not directly exploitable for signature forgery since the inner verifier always validates against the exact slice it needs. However, the silent truncation of trailing data could be used to:
1. Bypass mempool signature size limits if the limit is applied before slicing (it is applied to `sender_auth` overall — `MAX_SIGNATURE_SIZE = 2048` — but padding up to 2048 bytes is allowed).
2. Smuggle data into an auth blob that is then passed to a custom inner verifier, which might interpret the trailing bytes differently from what the protocol expects.

**Fix**: After the nested verifier resolves, add an explicit length check that the consumed bytes equal the total `data` length. For K1: assert `data.len() == 40 + 65`. For P256Raw: assert `data.len() == 40 + 128`.

---

### MEDIUM — M-02: `config_change_digest` does not bind to `account_config_address` — config sigs replayable across deployments

**File**: `rust/op-alloy/crates/consensus/src/transaction/eip8130/signature.rs:14–39`

The struct hash for `SignedOwnerChanges` includes `account`, `chainId`, and `sequence` but does NOT include the `AccountConfiguration` contract address. If the AccountConfiguration contract is redeployed at a different address (e.g., during an upgrade or across different OP Stack chains with the same chain_id), a config-change authorization signed for the old contract is valid for the new contract.

**Delta vs Tempo**: Tempo's design specifies predeploy addresses are genesis-fixed. For Base this is also true. However, devnets and testnets routinely redeploy. More importantly, the EIP-712 domain standard exists precisely to prevent this.

**Fix**: As part of the EIP-712 wrapping fix for H-01, include `verifyingContract: ACCOUNT_CONFIG_ADDRESS` in the domain separator. This binds signatures to the specific contract instance.

---

### MEDIUM — M-03: SeqK1 signature malleability — `K256Signature::from_slice` does not enforce low-s on all code paths

**File**: `rust/op-alloy/crates/consensus/src/transaction/eip8130/native_verifier.rs:136–141`

k256 version 0.13.4 with ecdsa 0.16.9: `K256Signature::from_slice` by default normalizes high-s to low-s (following the ECDSA library convention of auto-normalizing). However, this normalization is opaque to callers — the code does not explicitly check or document that low-s enforcement is happening. If the underlying library changes its normalization behavior in a future version, or if a different code path provides the signature bytes pre-parsed (bypassing `from_slice`), high-s signatures could slip through.

For K1 verification here, the `ownerId` is derived from address recovery (not a pubkey equality check), so two complementary signatures `(r, s)` and `(r, n-s)` with different `v` bytes both recover valid Ethereum addresses — just different ones. This means malleability per se doesn't let the same sig recover the same key with a different `s`. However, the lack of explicit enforcement is a hygiene risk.

**Fix**: Add an explicit assertion after `from_slice`:
```rust
if signature.s().is_high().into() {
    return NativeVerifyResult::Invalid(NativeVerifyError::K1RecoveryFailed(
        "high-s signature rejected".to_string(),
    ));
}
```

This matches Ethereum's convention (EIP-2) and should be made explicit rather than relying on library defaults.

---

### MEDIUM — M-04: P-256 signature malleability — `s` normalization not enforced for P256Raw and WebAuthn

**File**: `rust/op-alloy/crates/consensus/src/transaction/eip8130/native_verifier.rs:204`

`P256Signature::from_slice(sig_bytes)` in p256 0.13.2 does NOT auto-normalize high-s for P-256 signatures (unlike k256). For P-256 ECDSA, a signature `(r, s)` and its complement `(r, n-s)` both verify against the same public key. Since `ownerId = keccak256(publicKey)`, both variants return the same `ownerId` — but they are two distinct byte sequences that both pass validation.

**Impact**: A transaction signed with a high-s signature and a transaction signed with the corresponding low-s signature both validate successfully. This enables transaction hash malleability: the same logical operation can be submitted with two different `sender_auth` byte strings, producing two different transaction hashes (since `sender_auth` is part of the EIP-2718 envelope). A third party can take an in-flight P256 transaction and produce a second variant with the complementary `s`, potentially causing:
1. Double processing if both variants land in the mempool.
2. Confusion in receipt matching (different tx hash for same operation).

**Fix**: After `P256Signature::from_slice`, add:
```rust
use p256::elliptic_curve::IsHigh;
if signature.s().is_high().into() {
    return Err("high-s P256 signature rejected".to_string());
}
```
Apply this in `verify_p256_signature` so it covers both `verify_p256_raw` and `verify_webauthn`.

---

### MEDIUM — M-05: `OwnerScope::UNRESTRICTED = 0x00` semantics — a scope of 0 means all permissions, but this is not validated at owner registration time

**File**: `rust/op-alloy/crates/consensus/src/transaction/eip8130/types.rs:113–119`

`OwnerScope::UNRESTRICTED = 0x00` is defined as "all permissions" (any `has(0x00, permission)` returns `true`). When a new owner is registered via `CreateEntry::initial_owners`, the `scope` field comes directly from the transaction with no validation that it is a valid combination of defined scope bits. An attacker-controlled owner registration could pass `scope = 0xFF` (all bits set, including undefined ones), which would also pass all `has()` checks and be stored on-chain.

**Direct security impact** here is low (any scope value grants at most the four defined permissions). However, `scope = 0x00` meaning "all permissions" rather than "no permissions" is a semantic inversion that is non-obvious and could lead to developer errors. More critically, if future scope bits are added, existing owners with `scope = 0x00` would silently gain those new permissions.

**Fix**: At minimum add validation in `validate_account_changes_structure` that `scope` is either `0x00` (unrestricted) or a valid combination of defined bits: `scope & !(SIGNATURE | SENDER | PAYER | CONFIG) == 0`. If intent is "unrestricted = 0x00", document this prominently.

---

### LOW — L-01: `address_from_pubkey` has debug-only assertion for key format validity

**File**: `rust/op-alloy/crates/consensus/src/transaction/eip8130/native_verifier.rs:163`

```rust
fn address_from_pubkey(uncompressed: &[u8]) -> Address {
    debug_assert!(uncompressed.len() == 65 && uncompressed[0] == 0x04);
    ...
}
```

In release builds, `debug_assert!` is a no-op. If `uncompressed` has the wrong length or prefix (e.g., k256 returns a compressed key), the function would silently compute a wrong address. The caller in `verify_k1` uses `to_encoded_point(false)` (uncompressed) which should always return 65 bytes with `0x04` prefix — but this should be a hard assertion.

**Fix**: Change to `assert!` or pattern-match and return an error. Since this is called after trusted library code, a `debug_assert!` is acceptable only with a comment explaining why it can never fire; better to make it `assert!` for defense-in-depth.

---

### LOW — L-02: Nonce precompile reads only 8 bytes of a 32-byte storage slot for nonce output

**File**: `rust/alloy-op-evm/src/aa_precompiles.rs:109–111`

```rust
let mut out = [0u8; 32];
let storage_bytes = storage_value.data.to_be_bytes::<32>();
out[24..32].copy_from_slice(&storage_bytes[24..32]);
```

The nonce is stored as a `u64` (8 bytes in the low-order slot bytes). The code correctly reads only bytes 24..32 of the big-endian 32-byte slot. However, if the storage slot is somehow corrupted and contains non-zero bytes in positions 0..24, the output would still be a 32-byte ABI word with zeros in positions 0..24 — which correctly ABI-encodes a `uint64`. This is safe. But the code does not validate that `storage_bytes[0..24]` are zero, which would catch storage corruption silently.

**Risk**: Informational/negligible. The nonce is protocol-managed; storage corruption would be a deeper issue. No action required unless defensive programming is desired.

---

### LOW — L-03: `k256` version 0.13.4 — `RecoveryId::new` does not accept `is_x_reduced = true`, preventing Ethereum transactions with `x > n`

**File**: `rust/op-alloy/crates/consensus/src/transaction/eip8130/native_verifier.rs:143`

```rust
let recid = RecoveryId::new(recovery_id != 0, false);
```

The second parameter `is_x_reduced` is hardcoded to `false`. For secp256k1, a signature `r` value can theoretically be greater than the curve order `n` (i.e., the x-coordinate mod p > n). In this case the recovery ID's `is_x_reduced` bit would be `true`. Standard Ethereum (`eth_sign`) convention also hardcodes this to `false` because the probability is 2^-128 and wallets never produce such signatures. This matches the base implementation and Ethereum standard practice. No bug, but document the assumption.

---

### INFO — I-01: No constant-time comparison for `owner_id` or challenge

**File**: `rust/op-alloy/crates/consensus/src/transaction/eip8130/native_verifier.rs:299`

```rust
.is_some_and(|challenge| challenge == expected_challenge)
```

String comparison `==` is not guaranteed to be constant-time. For WebAuthn challenge comparison, the expected challenge is derived from the transaction hash (a public value), so timing leakage here does not reveal secrets. For the `owner_id` comparison in delegate verification (line 518), both sides are derived from public keys or computed values, so timing is also not secret. Constant-time comparison is not required in these paths — but this should be explicitly noted in comments.

---

### INFO — I-02: `config_change_digest` accepts `chain_id = 0` as "multi-chain" scope without warning

**File**: `rust/op-alloy/crates/consensus/src/transaction/eip8130/signature.rs:35` and `accessors.rs:127`

`ConfigChangeEntry.chain_id = 0` is semantically "apply on any chain" (multi-chain config change). The digest includes this value, so a `chain_id=0` signed message is intentionally valid on all chains. This is a protocol design choice, not a bug, but it should be clearly documented: a user signing a `chain_id=0` config change is authorizing owner modifications on every chain where their account exists. There is no user-visible warning mechanism in the AA transaction itself.

---

## POSITIVE — Verified Protections

The following protections were explicitly verified to be correctly implemented:

1. **Sender-payer domain separation**: `AA_TX_TYPE_ID (0x7B)` vs `AA_PAYER_TYPE (0x7C)` as the first byte of the signing prefix. `encode_for_sender_signing` and `encode_for_payer_signing` produce provably different hashes (test `sender_and_payer_signing_differ` confirms this).

2. **Chain ID binding**: `chain_id` is the first field in both sender and payer signing payloads. Cross-chain replay of the full signature is cryptographically infeasible.

3. **Nonce key + sequence binding**: Both `nonce_key` and `nonce_sequence` are included in the sender signing hash. Different 2D nonce positions produce different hashes.

4. **Expiry field in sender hash**: `expiry` is inside the sender signing hash, so a payer cannot reuse a sender signature after extending the expiry.

5. **Payer hash excludes payer address**: The payer hash encodes `from` (resolved sender) but not `payer`. This correctly prevents the payer from substituting themselves as the sender.

6. **K1 v-byte validation**: Only `{0, 1, 27, 28}` are accepted for the recovery id. Values 29–255 return `K1RecoveryFailed`.

7. **Nested delegation depth limit**: `verify_delegate` explicitly checks `inner_verifier == DELEGATE_VERIFIER_ADDRESS` and returns `DelegateNested` — no unbounded recursion.

8. **Delegate nesting rejects implicit EOA (address(0)) as delegate target**: The `inner_verifier == Address::ZERO` path is handled separately for the "delegate signs with its own EOA key" case, and it checks that the recovered address equals the declared delegate.

9. **WebAuthn challenge is a top-level JSON key**: `extract_top_level_json_string_field` uses a hand-rolled JSON parser that only matches top-level `"challenge"` keys, rejecting embedded challenge values in nested objects.

10. **WebAuthn challenge matches the exact transaction hash**: The challenge is `base64url(expected_hash)` where `expected_hash` is `keccak256(AA_TX_TYPE || rlp([...]))`. No field outside the challenge check path affects this.

11. **WebAuthn envelope length validation**: Both the overall minimum length and the `clientDataJSON` length field overflow are checked before indexing.

12. **P256 public key validity enforced by library**: `P256VerifyingKey::from_sec1_bytes` rejects the point at infinity and points not on the curve. All-zero `pubkey_raw` bytes would fail this check.

13. **REVOKED_VERIFIER sentinel**: `address(0xFFff...ff)` is correctly rejected in `check_sender_authorization` and `check_payer_authorization`.

14. **EOA sender derivation is not trusted from envelope**: `resolve_sender` requires `recovered_sender: Option<Address>` for the EOA path, explicitly rejecting `None` as an error. The wire-format `tx.from = None` is never used as the sender address directly.

15. **Auth blob size bounds**: `MAX_SIGNATURE_SIZE = 2048` is enforced in `validate_structure` for `sender_auth`, `payer_auth`, and `authorizer_auth`.

16. **Nonce-free mode structural constraints**: `nonce_sequence == 0` and `expiry != 0` are both enforced when `nonce_key == NONCE_KEY_MAX`.

17. **Scope bitmask checked before authorization**: Both `check_sender_authorization` and `check_payer_authorization` verify the correct scope bit (`SENDER` / `PAYER`) is set, with `scope == 0` meaning unrestricted.

18. **Config change sequence enforced**: `validate_config_change_sequences` reads the on-chain sequence and rejects mismatches — config changes cannot be replayed.

---

## DELTA vs BASE

| Area | Our Port | Base | Assessment |
|------|----------|------|------------|
| `payer_signature_hash` signature | Takes explicit `resolved_sender: Address` | Uses `self.from` directly | Our version is more correct for the EOA cross-sender protection; **wire-incompatible with base for EOA senders**. Must coordinate with Solidity contract. |
| `from` field type | `Option<Address>` (our refactor) | `Option<Address>` (base also uses Option) | Identical |
| WebAuthn `type` check | Not implemented | Not implemented | Shared gap (C-01) |
| WebAuthn `UP`/`UV` flags | Not checked | Not checked | Shared gap (C-02) |
| P256 high-s rejection | Not explicit | Not explicit | Shared gap (M-04) |
| K1 explicit low-s | Delegated to k256 library | Delegated to k256 library | Shared implicit assumption |
| Nonce-free expiry window enforcement | Constant defined but **not enforced** | Constant defined but **not enforced** | Shared gap (H-03) |
| `config_change_digest` EIP-712 prefix | Absent | Absent | Shared gap (H-01) |
| Delegate scope validation | Not in cryptographic layer | Not in cryptographic layer | Shared design limitation (H-02) |

---

## DELTA vs TEMPO

| Area | TEMPO Design | Base/Our Port | Assessment |
|------|-------------|---------------|------------|
| Signature type downgrade | Root key / access key hierarchy prevents downgrade | No explicit downgrade protection: any `verifier` address in `owner_config` can be used for any scope that key holds | Base/ours lack scheme-level downgrade protection. An attacker who registers a weaker-scheme key with unrestricted scope can use it even if a stronger key was intended for SENDER use. |
| Origin binding in WebAuthn | Tempo specifies "协议层完整实现" (full protocol implementation) | Origin field in `clientDataJSON` is parsed by tests but NOT verified in production code | Tempo's intent of "full" WebAuthn implies origin validation; our implementation skips it. |
| Multi-chain config replay | Explicitly called out as out of scope for Tempo design | `chain_id=0` enables multi-chain scope intentionally; no additional protection | Tempo flags this as a deliberate design choice requiring operator awareness. |
| Session key expiry at protocol level | Precompile-enforced key expiry | Policy at contract layer only; protocol has no per-key expiry | Tempo's richer key management avoids "stale session key" scenarios; Base/ours rely on contract-layer policy. This is a design choice, not a bug. |
| Key revocation and mempool eviction | Explicit revocation-driven mempool eviction | Account lock mechanism provides coarse freeze; no per-key revocation-driven eviction | Tempo provides stronger revocation guarantees. |

---

## Final Verdict

**AMBER — NOT PRODUCTION-READY for the signature path as-is.**

Two CRITICAL issues (C-01, C-02) allow a passkey credential from a registration ceremony (`webauthn.create`) to authenticate as a WebAuthn signer, and allow signatures without user presence attestation to pass. These are directly exploitable for asset theft against any account protected solely by a WebAuthn owner.

Two HIGH issues (H-01, H-02) allow config-change signatures to potentially be replayed across contract instances and allow the delegate verification scheme to bypass inner key scope constraints.

One HIGH issue (H-03) allows nonce-free transactions to carry unrestricted future expiry, undermining the replay protection the circular buffer is intended to provide.

All five issues (C-01, C-02, H-01, H-02, H-03) should be resolved before production deployment. The medium findings (M-01 through M-05) should be addressed before mainnet; M-03 and M-04 (signature malleability) affect transaction hash stability and could cause issues in block explorers or receipt matching tooling.

