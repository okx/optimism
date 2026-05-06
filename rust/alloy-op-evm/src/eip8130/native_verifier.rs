//! Native verifier dispatch for the four EIP-8130 native verifier addresses.
//!
//! The four native verifiers, by address:
//!
//! | Variant         | Address                                      | Data layout                                                                              | `owner_id`                       |
//! |-----------------|----------------------------------------------|------------------------------------------------------------------------------------------|----------------------------------|
//! | K1              | `0x0000…0001`                                | `r(32) \|\| s(32) \|\| v(1)` (65 bytes)                                                  | `bytes32(bytes20(ecrecover))`    |
//! | `P256Raw`         | `0x75E9779603e826f2D8d4dD7Edee3F0a737e4228d` | `pubkey(64) \|\| r(32) \|\| s(32)` (128 bytes)                                           | `keccak256(pubkey)`              |
//! | `P256WebAuthn`    | `0xb2c8b7ec119882fBcc32FDe1be1341e19a5Bd53E` | `pubkey(64) \|\| authData(37+) \|\| jsonLen(4 BE) \|\| json \|\| sig(64)`                 | `keccak256(pubkey)`              |
//! | Delegate        | `0x30A76831b27732087561372f6a1bef6Fc391d805` | `delegate_addr(20) \|\| inner_auth(verifier(20) \|\| inner_data)`                         | `bytes32(bytes20(delegate_addr))`|
//!
//! Custom verifiers (any other address) return [`NativeVerifyResult::Unsupported`];
//! the auth-state builder turns these into [`op_revm::transaction::eip8130::AuthState::Deferred`]
//! for the handler to STATICCALL at execution time.
//!
//! All four native verifiers are fully implemented; see [`verify_webauthn`] for the
//! `WebAuthn` assertion check (signed-message convention, base64url challenge binding).

use alloy_primitives::{Address, B256, Bytes, Signature, U256, keccak256, uint};
use op_revm::constants::{
    DELEGATE_VERIFIER_ADDRESS, K1_VERIFIER_ADDRESS, P256_RAW_VERIFIER_ADDRESS,
    P256_WEBAUTHN_VERIFIER_ADDRESS,
};

// ── P256 (NIST secp256r1) low-s threshold ──
//
// ECDSA is malleable: `(r, N - s)` verifies the same message as `(r, s)`.
// For our setup, tx-hash replay protection already covers this surface, but
// enforcing low-s closes it deterministically and aligns with tempo's
// hardening (`tt_signature.rs::P256N_HALF`).

/// `N / 2` — the threshold for low-s form. Signatures with `s > N/2` are
/// rejected. Matches `tempo::tt_signature::P256N_HALF`.
const P256_N_HALF: U256 =
    uint!(0x7FFFFFFF800000007FFFFFFFFFFFFFFFDE737D56D38BCF4279DCE5617E3192A8_U256);

/// Identifies an EIP-8130 native verifier by its sentinel address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeVerifier {
    /// K1 (secp256k1 / ECDSA) at [`K1_VERIFIER_ADDRESS`].
    K1,
    /// P256 raw signature at [`P256_RAW_VERIFIER_ADDRESS`].
    P256Raw,
    /// P256 `WebAuthn` at [`P256_WEBAUTHN_VERIFIER_ADDRESS`].
    P256WebAuthn,
    /// 1-hop delegate verifier at [`DELEGATE_VERIFIER_ADDRESS`].
    Delegate,
}

impl NativeVerifier {
    /// Returns the [`NativeVerifier`] matching `address`, or [`None`] for
    /// custom verifiers.
    #[inline]
    pub fn from_address(address: Address) -> Option<Self> {
        match address {
            a if a == K1_VERIFIER_ADDRESS => Some(Self::K1),
            a if a == P256_RAW_VERIFIER_ADDRESS => Some(Self::P256Raw),
            a if a == P256_WEBAUTHN_VERIFIER_ADDRESS => Some(Self::P256WebAuthn),
            a if a == DELEGATE_VERIFIER_ADDRESS => Some(Self::Delegate),
            _ => None,
        }
    }

    /// Returns the on-chain sentinel address for this verifier.
    #[inline]
    pub const fn address(self) -> Address {
        match self {
            Self::K1 => K1_VERIFIER_ADDRESS,
            Self::P256Raw => P256_RAW_VERIFIER_ADDRESS,
            Self::P256WebAuthn => P256_WEBAUTHN_VERIFIER_ADDRESS,
            Self::Delegate => DELEGATE_VERIFIER_ADDRESS,
        }
    }
}

/// Outcome of dispatching `(verifier, data, sig_hash)` through native verification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeVerifyResult {
    /// Native verification succeeded. `owner_id` is the verifier-specific
    /// 32-byte identifier (see [`NativeVerifier`] table).
    Verified(B256),
    /// `verifier` is not a known native address — the auth-state builder
    /// turns this into [`op_revm::transaction::eip8130::AuthState::Deferred`]
    /// and the handler runs the STATICCALL at execution time.
    Unsupported,
    /// `verifier` is native but data was malformed or the signature failed
    /// to verify. Builder turns this into
    /// [`op_revm::transaction::eip8130::AuthState::Invalid`] (passing the
    /// reason through unchanged).
    ///
    /// Carries a reason describing which specific check rejected the data
    /// (e.g. `"K1: bad sig length"`). `String` rather than `&'static str`
    /// to match `AuthState::Invalid`'s payload type — see the rationale
    /// there.
    Invalid(alloc::string::String),
}

/// Dispatches `(verifier, data, sig_hash)` to the matching native verifier.
///
/// `verifier` is the on-chain verifier address (the auth blob's first 20
/// bytes in explicit-from / sponsored mode, or [`K1_VERIFIER_ADDRESS`] in
/// EOA mode). `data` is the verifier-specific payload (everything after the
/// 20-byte address prefix in explicit-from mode; the bare 65-byte sig in EOA
/// mode). `sig_hash` is `sender_signature_hash(tx)` or
/// `payer_signature_hash(tx)`.
#[inline]
pub fn try_native_verify(verifier: Address, data: &Bytes, sig_hash: B256) -> NativeVerifyResult {
    match NativeVerifier::from_address(verifier) {
        Some(NativeVerifier::K1) => verify_k1(data, sig_hash),
        Some(NativeVerifier::P256Raw) => verify_p256_raw(data, sig_hash),
        Some(NativeVerifier::P256WebAuthn) => verify_webauthn(data, sig_hash),
        Some(NativeVerifier::Delegate) => verify_delegate(data, sig_hash),
        None => NativeVerifyResult::Unsupported,
    }
}

/// Encodes a 20-byte address as `bytes32(bytes20(addr))` (Solidity convention):
/// address bytes occupy the high 20 bytes, low 12 bytes are zero.
///
/// Used for K1 / Delegate `owner_id` derivation. **Not** the same as
/// `bytes32(uint256(uint160(addr)))` (right-padded form used elsewhere in
/// Ethereum) — that form is incorrect for EIP-8130.
#[inline]
pub fn address_to_owner_id(addr: Address) -> B256 {
    let mut buf = [0u8; 32];
    buf[..20].copy_from_slice(addr.as_slice());
    B256::from(buf)
}

/// Verifies a K1 (secp256k1) signature.
///
/// `data` is `r(32) || s(32) || v(1)`. Accepts `v ∈ {0, 1, 27, 28}`.
///
/// Length / parity checks happen here so each rejection can carry a specific
/// reason in [`NativeVerifyResult::Invalid`].
#[inline]
fn verify_k1(data: &Bytes, sig_hash: B256) -> NativeVerifyResult {
    if data.len() != 65 {
        return NativeVerifyResult::Invalid("K1: bad sig length".into());
    }
    let v = data[64];
    let parity = match v {
        0 | 27 => false,
        1 | 28 => true,
        _ => return NativeVerifyResult::Invalid("K1: bad v parity".into()),
    };
    let signature = Signature::new(
        U256::from_be_slice(&data[..32]),
        U256::from_be_slice(&data[32..64]),
        parity,
    );
    alloy_consensus::crypto::secp256k1::recover_signer(&signature, sig_hash).map_or_else(
        |_| NativeVerifyResult::Invalid("K1: ecrecover failed".into()),
        |addr| NativeVerifyResult::Verified(address_to_owner_id(addr)),
    )
}

/// Verifies a raw P256 (secp256r1) signature.
///
/// `data` layout: `pubkey_uncompressed_no_prefix(64) || r(32) || s(32)` (128 bytes).
/// `owner_id = keccak256(pubkey_uncompressed_no_prefix)` (the hash of the 64-byte
/// concatenated `(x || y)` representation).
fn verify_p256_raw(data: &Bytes, sig_hash: B256) -> NativeVerifyResult {
    use p256::ecdsa::{Signature as P256Sig, VerifyingKey, signature::hazmat::PrehashVerifier};

    if data.len() != 128 {
        return NativeVerifyResult::Invalid("P256-raw: bad data length".into());
    }
    let pubkey_bytes = &data[..64];
    let r_bytes = &data[64..96];
    let s_bytes = &data[96..128];

    // Reconstruct uncompressed SEC1 (`0x04 || x || y`) for VerifyingKey::from_sec1_bytes.
    let mut sec1 = [0u8; 65];
    sec1[0] = 0x04;
    sec1[1..].copy_from_slice(pubkey_bytes);
    let Ok(vk) = VerifyingKey::from_sec1_bytes(&sec1) else {
        return NativeVerifyResult::Invalid("P256-raw: invalid pubkey".into());
    };

    if !p256_s_is_low(s_bytes) {
        return NativeVerifyResult::Invalid("P256-raw: high-s signature".into());
    }

    let mut sig_bytes = [0u8; 64];
    sig_bytes[..32].copy_from_slice(r_bytes);
    sig_bytes[32..].copy_from_slice(s_bytes);
    let Ok(sig) = P256Sig::from_slice(&sig_bytes) else {
        return NativeVerifyResult::Invalid("P256-raw: invalid signature".into());
    };

    if vk.verify_prehash(sig_hash.as_slice(), &sig).is_err() {
        return NativeVerifyResult::Invalid("P256-raw: verify failed".into());
    }
    NativeVerifyResult::Verified(keccak256(pubkey_bytes))
}

/// Returns `true` iff the 32-byte big-endian P256 `s` scalar is in low-s form
/// (`0 < s <= N/2`). Rejects zero and high-s. ECDSA is malleable: `(r, N-s)`
/// is a valid alternate signature for `(r, s)`, and accepting both forms
/// opens malleability surfaces. We close it deterministically for both
/// raw-P256 and the inner P256 sig of `WebAuthn`.
#[inline]
fn p256_s_is_low(s_bytes: &[u8]) -> bool {
    if s_bytes.len() != 32 {
        return false;
    }
    let s = U256::from_be_slice(s_bytes);
    !s.is_zero() && s <= P256_N_HALF
}

/// Minimal `clientDataJSON` view, deserialized from the assertion blob.
///
/// We only care about the `type` and `challenge` fields. `serde_json` ignores
/// any other fields (including `origin`) — those are the verifier contract's
/// concern at the spec level. We never decode `challenge` back to bytes; we
/// re-encode the expected `sig_hash` to base64url and string-compare, which
/// avoids needing a base64 decoder and is unambiguous because `URL_SAFE_NO_PAD`
/// encoding is deterministic for a fixed-size 32-byte input.
#[derive(serde::Deserialize)]
struct ClientDataJson<'a> {
    #[serde(rename = "type", borrow)]
    type_field: &'a str,
    #[serde(borrow)]
    challenge: &'a str,
}

/// Verifies a P256 `WebAuthn` assertion.
///
/// `data` layout:
/// `pubkey(64) || authenticatorData(37) || clientDataJSONLen(4 BE) || clientDataJSON ||
/// signature(64)`
///
/// We treat `authenticatorData` as exactly 37 bytes (rpIdHash 32 + flags 1 +
/// signCount 4); EIP-8130 doesn't permit extensions on the assertion path,
/// so the longer-form attestedCredentialData / extension layouts don't appear.
///
/// Algorithm (matches `WebAuthn` `webauthn.get` assertion + tempo's convention):
///
/// 1. Parse the wire layout.
/// 2. Validate authenticatorData flags: at least one of UP / UV must be set; AT and ED must NOT be
///    set (assertion-only, no attested cred data, no extensions).
/// 3. Validate `clientDataJSON.type == "webauthn.get"` (rejects replay of a registration ceremony
///    assertion as authentication).
/// 4. Validate `clientDataJSON.challenge == base64url_no_pad(sig_hash)` (binds the assertion to
///    *this* transaction).
/// 5. Reject high-s signatures (low-s form only).
/// 6. Compute `signedMessage = sha256(authenticatorData || sha256(clientDataJSON))`.
/// 7. P256-verify `(r || s)` over `signedMessage` using `pubkey`.
///
/// On success: `NativeVerifyResult::Verified(keccak256(pubkey))` — same
/// `owner_id` derivation as P256-raw.
fn verify_webauthn(data: &Bytes, sig_hash: B256) -> NativeVerifyResult {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use p256::ecdsa::{Signature as P256Sig, VerifyingKey, signature::hazmat::PrehashVerifier};
    use sha2::{Digest, Sha256};

    const PUBKEY_LEN: usize = 64;
    const AUTH_DATA_LEN: usize = 37;
    const SIG_LEN: usize = 64;
    const LEN_PREFIX: usize = 4;
    const MIN_TOTAL: usize = PUBKEY_LEN + AUTH_DATA_LEN + LEN_PREFIX + SIG_LEN;

    // authenticatorData flag bits (offset 32 within auth_data).
    const FLAG_UP: u8 = 0x01;
    const FLAG_UV: u8 = 0x04;
    const FLAG_AT: u8 = 0x40;
    const FLAG_ED: u8 = 0x80;

    if data.len() < MIN_TOTAL {
        return NativeVerifyResult::Invalid("webauthn: data too short".into());
    }

    // 1. Slice fields left-to-right.
    let pubkey_bytes = &data[..PUBKEY_LEN];
    let auth_data_end = PUBKEY_LEN + AUTH_DATA_LEN;
    let auth_data = &data[PUBKEY_LEN..auth_data_end];

    // 2. Flag checks — explicit and early so the rejection reason is clear, rather than failing
    //    later via a length-mismatch / sig-verify failure.
    let flags = auth_data[32];
    if flags & FLAG_AT != 0 {
        // Attested credential data is only valid on registration ceremonies.
        return NativeVerifyResult::Invalid("webauthn: AT flag must not be set".into());
    }
    if flags & FLAG_ED != 0 {
        // Extensions push auth_data beyond 37 bytes; we don't parse them and
        // the wire format has no per-blob auth_data length to recover from it.
        return NativeVerifyResult::Invalid("webauthn: ED flag must not be set".into());
    }
    if flags & (FLAG_UP | FLAG_UV) == 0 {
        // Spec mandates at least one of UP / UV. UV implies UP.
        return NativeVerifyResult::Invalid("webauthn: neither UP nor UV flag set".into());
    }

    let mut len_be = [0u8; LEN_PREFIX];
    len_be.copy_from_slice(&data[auth_data_end..auth_data_end + LEN_PREFIX]);
    let client_data_len = u32::from_be_bytes(len_be) as usize;

    let cd_start = auth_data_end + LEN_PREFIX;
    // `client_data_len` is attacker-controlled — overflow-check before adding.
    let Some(cd_end) = cd_start.checked_add(client_data_len) else {
        return NativeVerifyResult::Invalid("webauthn: clientDataJSONLen out of bounds".into());
    };
    let Some(expected_total) = cd_end.checked_add(SIG_LEN) else {
        return NativeVerifyResult::Invalid("webauthn: clientDataJSONLen out of bounds".into());
    };
    if data.len() < expected_total {
        return NativeVerifyResult::Invalid("webauthn: clientDataJSONLen out of bounds".into());
    }
    if data.len() != expected_total {
        return NativeVerifyResult::Invalid("webauthn: trailing bytes".into());
    }

    let client_data = &data[cd_start..cd_end];
    let sig_bytes = &data[cd_end..cd_end + SIG_LEN];

    // 3. Validate pubkey (cheap, deterministic).
    let mut sec1 = [0u8; 65];
    sec1[0] = 0x04;
    sec1[1..].copy_from_slice(pubkey_bytes);
    let Ok(vk) = VerifyingKey::from_sec1_bytes(&sec1) else {
        return NativeVerifyResult::Invalid("webauthn: invalid pubkey".into());
    };

    // 4. Parse clientDataJSON via serde_json. `borrow`'d &str fields fail to
    // deserialize if the input contains escape sequences in those fields,
    // which is exactly what we want — challenges are base64url and types are
    // ASCII literals; an escape there indicates injection.
    let Ok(cd) = serde_json::from_slice::<ClientDataJson<'_>>(client_data) else {
        return NativeVerifyResult::Invalid("webauthn: clientDataJSON parse failed".into());
    };
    if cd.type_field != "webauthn.get" {
        return NativeVerifyResult::Invalid("webauthn: type != webauthn.get".into());
    }

    // 5. Compare challenge by re-encoding sig_hash to base64url-no-pad. Uses the `base64` crate's
    //    `encode_slice` with a fixed 43-byte stack buffer (32 bytes -> 43 base64url-no-pad chars),
    //    zero-alloc. Aligns with tempo's `URL_SAFE_NO_PAD.encode(...)` approach.
    let mut expected_challenge = [0u8; 43];
    let n = URL_SAFE_NO_PAD
        .encode_slice(sig_hash.as_slice(), &mut expected_challenge)
        .expect("32 bytes always encode to 43 base64url-no-pad chars");
    debug_assert_eq!(n, 43);
    if cd.challenge.as_bytes() != expected_challenge {
        return NativeVerifyResult::Invalid("webauthn: challenge mismatch".into());
    }

    // 6. Low-s rejection on the inner P256 sig.
    if !p256_s_is_low(&sig_bytes[32..64]) {
        return NativeVerifyResult::Invalid("webauthn: high-s signature".into());
    }

    // 7. signedMessage = sha256(authenticatorData || sha256(clientDataJSON))
    let client_data_hash = Sha256::digest(client_data);
    let mut hasher = Sha256::new();
    hasher.update(auth_data);
    hasher.update(client_data_hash);
    let signed_message = hasher.finalize();

    // 8. P256-verify (r || s) over signed_message.
    let Ok(sig) = P256Sig::from_slice(sig_bytes) else {
        return NativeVerifyResult::Invalid("webauthn: signature parse failed".into());
    };
    if vk.verify_prehash(&signed_message, &sig).is_err() {
        return NativeVerifyResult::Invalid("webauthn: verify failed".into());
    }

    NativeVerifyResult::Verified(keccak256(pubkey_bytes))
}

/// Verifies a 1-hop delegate auth.
///
/// `data` layout: `delegate_address(20) || inner_auth(inner_verifier(20) || inner_data)`.
/// On success `owner_id = bytes32(bytes20(delegate_address))`.
///
/// The inner verifier may itself be either native or custom.
/// - Inner native: this fn verifies the inner cryptography eagerly. The handler later validates
///   `owner_config[account][bytes32(delegate_address)] = (DELEGATE, scope)` and (via the
///   delegate-special-case branch) `owner_config[delegate_address][...]` for the inner verifier.
/// - Inner custom: this fn returns [`NativeVerifyResult::Unsupported`] because we cannot run the
///   inner STATICCALL eagerly. The auth-state builder turns that into
///   [`op_revm::transaction::eip8130::AuthState::Deferred`] with `delegate_outer =
///   Some(delegate_address)` so the handler does both the inner STATICCALL and the outer
///   delegate-binding check.
///
/// **1-hop only**: if the inner verifier is itself `DELEGATE_VERIFIER_ADDRESS`,
/// this rejects with [`NativeVerifyResult::Invalid`]. Without this guard a
/// recursive Delegate→Delegate→Native chain would slip through with the
/// nested K1/P256 sig validating mathematically but never being checked
/// against the innermost delegate's `owner_config` (the recursion only
/// returns the *intermediate* delegate's address as `owner_id`).
fn verify_delegate(data: &Bytes, sig_hash: B256) -> NativeVerifyResult {
    if data.len() < 40 {
        // Need at least delegate_address(20) + inner_verifier(20).
        return NativeVerifyResult::Invalid("delegate: blob too short".into());
    }
    let delegate_addr = Address::from_slice(&data[..20]);
    let inner_verifier = Address::from_slice(&data[20..40]);

    // Reject Delegate→Delegate (1-hop limit per spec).
    if inner_verifier == DELEGATE_VERIFIER_ADDRESS {
        return NativeVerifyResult::Invalid("delegate: 1-hop limit (inner is delegate)".into());
    }

    // Zero-copy slice — `Bytes` is Arc-backed.
    let inner_data = data.slice(40..);

    match try_native_verify(inner_verifier, &inner_data, sig_hash) {
        NativeVerifyResult::Verified(_inner_owner_id) => {
            // Inner native verification passed; outer owner_id is the delegate
            // address. Inner (verifier, owner_id) binding is checked by the
            // handler's Native dispatch's `delegate_inner` arm.
            NativeVerifyResult::Verified(address_to_owner_id(delegate_addr))
        }
        // Bubble the inner reason up unchanged so we know which inner check
        // rejected the auth.
        NativeVerifyResult::Invalid(reason) => NativeVerifyResult::Invalid(reason),
        NativeVerifyResult::Unsupported => NativeVerifyResult::Unsupported,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, B256, Bytes, U256};
    use alloy_signer::SignerSync;
    use alloy_signer_local::PrivateKeySigner;

    fn signer_from_seed(seed: u8) -> PrivateKeySigner {
        let bytes = B256::repeat_byte(seed);
        PrivateKeySigner::from_bytes(&bytes).expect("valid private key")
    }

    fn k1_sig_blob(signer: &PrivateKeySigner, hash: B256) -> Vec<u8> {
        let sig = signer.sign_hash_sync(&hash).expect("sign");
        let mut buf = Vec::with_capacity(65);
        buf.extend_from_slice(&sig.r().to_be_bytes::<32>());
        buf.extend_from_slice(&sig.s().to_be_bytes::<32>());
        buf.push(if sig.v() { 1 } else { 0 });
        buf
    }

    #[test]
    fn from_address_dispatch() {
        assert_eq!(NativeVerifier::from_address(K1_VERIFIER_ADDRESS), Some(NativeVerifier::K1));
        assert_eq!(
            NativeVerifier::from_address(P256_RAW_VERIFIER_ADDRESS),
            Some(NativeVerifier::P256Raw),
        );
        assert_eq!(
            NativeVerifier::from_address(P256_WEBAUTHN_VERIFIER_ADDRESS),
            Some(NativeVerifier::P256WebAuthn),
        );
        assert_eq!(
            NativeVerifier::from_address(DELEGATE_VERIFIER_ADDRESS),
            Some(NativeVerifier::Delegate),
        );
        assert_eq!(NativeVerifier::from_address(Address::repeat_byte(0xAB)), None);
    }

    #[test]
    fn k1_round_trip() {
        let signer = signer_from_seed(0x11);
        let hash = B256::repeat_byte(0xAA);
        let sig = Bytes::from(k1_sig_blob(&signer, hash));

        match try_native_verify(K1_VERIFIER_ADDRESS, &sig, hash) {
            NativeVerifyResult::Verified(owner_id) => {
                let mut expected = [0u8; 32];
                expected[..20].copy_from_slice(signer.address().as_slice());
                assert_eq!(owner_id, B256::from(expected));
            }
            other => panic!("expected Verified, got {other:?}"),
        }
    }

    #[test]
    fn k1_wrong_length_invalid() {
        let bytes = Bytes::from(vec![0u8; 64]);
        match try_native_verify(K1_VERIFIER_ADDRESS, &bytes, B256::ZERO) {
            NativeVerifyResult::Invalid(reason) => {
                assert!(reason.contains("length"), "unexpected reason: {reason}");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn k1_bad_v_invalid() {
        let mut bytes = vec![0u8; 65];
        bytes[64] = 99;
        match try_native_verify(K1_VERIFIER_ADDRESS, &Bytes::from(bytes), B256::ZERO) {
            NativeVerifyResult::Invalid(reason) => {
                assert!(reason.contains("v parity"), "unexpected reason: {reason}");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn p256_raw_wrong_length_invalid() {
        let bytes = Bytes::from(vec![0u8; 127]);
        match try_native_verify(P256_RAW_VERIFIER_ADDRESS, &bytes, B256::ZERO) {
            NativeVerifyResult::Invalid(reason) => {
                assert!(reason.contains("length"), "unexpected reason: {reason}");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn p256_raw_round_trip() {
        use p256::ecdsa::SigningKey;

        // Deterministic signing key.
        let scalar = U256::from(42u64).to_be_bytes::<32>();
        let sk = SigningKey::from_bytes((&scalar).into()).expect("valid");
        let pk = sk.verifying_key();
        let sec1 = pk.to_encoded_point(false);
        // sec1 = 0x04 || x(32) || y(32); strip the 0x04.
        let pk64 = &sec1.as_bytes()[1..];

        let hash = B256::repeat_byte(0xBE);
        let sig_bytes = sign_p256_low_s(&sk, hash.as_slice());

        let mut data = Vec::with_capacity(128);
        data.extend_from_slice(pk64);
        data.extend_from_slice(&sig_bytes);

        match try_native_verify(P256_RAW_VERIFIER_ADDRESS, &Bytes::from(data), hash) {
            NativeVerifyResult::Verified(owner_id) => {
                assert_eq!(owner_id, keccak256(pk64));
            }
            other => panic!("expected Verified, got {other:?}"),
        }
    }

    #[test]
    fn p256_raw_wrong_signature_invalid() {
        // 128 bytes of zeroes: pubkey portion (first 64) decodes to point
        // at infinity which `from_sec1_bytes` rejects, so failure mode is
        // "invalid pubkey", not signature verification. Pinning the reason
        // lets a refactor that flips ordering (e.g., parse sig before
        // pubkey) get caught instead of silently passing on a different
        // failure path.
        let bytes = Bytes::from(vec![0u8; 128]);
        match try_native_verify(P256_RAW_VERIFIER_ADDRESS, &bytes, B256::ZERO) {
            NativeVerifyResult::Invalid(reason) => {
                assert!(
                    reason.contains("invalid pubkey"),
                    "expected pubkey-decode failure, got: {reason}",
                );
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    /// Encodes `bytes` as base64url without padding (URL-safe alphabet).
    /// Used only by tests to craft the `challenge` field. The production
    /// verifier has its own fixed-size 32-byte encoder
    /// ([`base64url_encode_32`]) — this test helper is variable-length and
    /// independently written so a bug in either won't mask the other.
    fn b64url_encode(bytes: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
            let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
            let triple = (b0 << 16) | (b1 << 8) | b2;
            out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
            out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
            if chunk.len() > 1 {
                out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
            }
            if chunk.len() > 2 {
                out.push(TABLE[(triple & 0x3F) as usize] as char);
            }
        }
        out
    }

    /// Builds a `WebAuthn` auth blob for the given key, with `challenge` set to
    /// Signs `prehash` with `sk` and normalizes to low-s form (rejects the
    /// p256 crate's possibly-high-s output by flipping it to `N - s`). Always
    /// returns 64 bytes `r || s` in low-s form so the production verifier
    /// (which now enforces low-s) accepts the signature.
    fn sign_p256_low_s(sk: &p256::ecdsa::SigningKey, prehash: &[u8]) -> [u8; 64] {
        use p256::ecdsa::{Signature as P256Sig, signature::hazmat::PrehashSigner};
        let sig: P256Sig = sk.sign_prehash(prehash).expect("sign prehash");
        // `Signature::normalize_s` returns `Some(low_s)` if the input was
        // high-s, `None` otherwise — keep whichever is low.
        let normalized = sig.normalize_s().unwrap_or(sig);
        normalized.to_bytes().into()
    }

    /// `challenge_hash` and `signed_message_hash` used to compute the P256
    /// signature. Splitting the two lets tests craft "valid challenge but
    /// signed over a different message" (and vice-versa).
    fn build_webauthn_blob(
        sk: &p256::ecdsa::SigningKey,
        challenge_hash: B256,
        signed_message_hash: B256,
    ) -> Vec<u8> {
        // Default to UP-only flags (0x01); test cases that need other flag
        // combinations call `build_webauthn_blob_with_flags` directly.
        build_webauthn_blob_with_flags(sk, challenge_hash, signed_message_hash, 0x01)
    }

    /// Like [`build_webauthn_blob`] but with explicit `flags` byte at offset
    /// 32 of `authenticatorData` — used by flag-validation tests.
    fn build_webauthn_blob_with_flags(
        sk: &p256::ecdsa::SigningKey,
        challenge_hash: B256,
        signed_message_hash: B256,
        flags: u8,
    ) -> Vec<u8> {
        use sha2::{Digest, Sha256};

        let pk = sk.verifying_key();
        let sec1 = pk.to_encoded_point(false);
        let pk64 = &sec1.as_bytes()[1..];

        // 32-byte rpIdHash (zeros) + 1 flag byte + 4 signCount bytes (zeros).
        let mut auth_data = [0u8; 37];
        auth_data[32] = flags;

        let challenge_b64 = b64url_encode(challenge_hash.as_slice());
        let client_data_json = format!(
            r#"{{"type":"webauthn.get","challenge":"{}","origin":"https://example.test"}}"#,
            challenge_b64,
        );
        let cd_bytes = client_data_json.as_bytes();
        let cd_len = (cd_bytes.len() as u32).to_be_bytes();

        // signedMessage = sha256(authenticatorData || sha256(<msg-source>)).
        // For the positive test, msg-source == clientDataJSON. For "bad sig",
        // re-derive over a *different* clientDataJSON.
        let actual_cd_hash = if challenge_hash == signed_message_hash {
            Sha256::digest(cd_bytes).into()
        } else {
            let other_b64 = b64url_encode(signed_message_hash.as_slice());
            let other_cd = format!(
                r#"{{"type":"webauthn.get","challenge":"{}","origin":"https://example.test"}}"#,
                other_b64,
            );
            <[u8; 32]>::from(Sha256::digest(other_cd.as_bytes()))
        };
        let mut hasher = Sha256::new();
        hasher.update(auth_data);
        hasher.update(actual_cd_hash);
        let signed_message = hasher.finalize();

        let sig_bytes = sign_p256_low_s(sk, &signed_message);

        let mut blob = Vec::with_capacity(64 + 37 + 4 + cd_bytes.len() + 64);
        blob.extend_from_slice(pk64);
        blob.extend_from_slice(&auth_data);
        blob.extend_from_slice(&cd_len);
        blob.extend_from_slice(cd_bytes);
        blob.extend_from_slice(&sig_bytes);
        blob
    }

    fn deterministic_p256_signing_key(scalar_seed: u64) -> p256::ecdsa::SigningKey {
        use p256::ecdsa::SigningKey;
        let scalar = U256::from(scalar_seed).to_be_bytes::<32>();
        SigningKey::from_bytes((&scalar).into()).expect("valid P256 scalar")
    }

    #[test]
    fn webauthn_round_trip() {
        let sk = deterministic_p256_signing_key(7);
        let pk = sk.verifying_key();
        let sec1 = pk.to_encoded_point(false);
        let pk64 = &sec1.as_bytes()[1..];

        let hash = B256::repeat_byte(0x42);
        let blob = build_webauthn_blob(&sk, hash, hash);

        match try_native_verify(P256_WEBAUTHN_VERIFIER_ADDRESS, &Bytes::from(blob), hash) {
            NativeVerifyResult::Verified(owner_id) => {
                assert_eq!(owner_id, keccak256(pk64));
            }
            other => panic!("expected Verified, got {other:?}"),
        }
    }

    #[test]
    fn webauthn_challenge_mismatch_invalid() {
        let sk = deterministic_p256_signing_key(11);
        let tx_hash = B256::repeat_byte(0x42);
        let other_hash = B256::repeat_byte(0xAB);
        // Challenge encodes `other_hash`, signed message also over that
        // (so the signature is valid but the bound hash isn't ours).
        let blob = build_webauthn_blob(&sk, other_hash, other_hash);

        match try_native_verify(P256_WEBAUTHN_VERIFIER_ADDRESS, &Bytes::from(blob), tx_hash) {
            NativeVerifyResult::Invalid(reason) => {
                assert!(reason.contains("challenge mismatch"), "unexpected reason: {reason}",);
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn webauthn_truncated_blob_invalid() {
        let sk = deterministic_p256_signing_key(13);
        let hash = B256::repeat_byte(0x42);
        let full = build_webauthn_blob(&sk, hash, hash);

        // (a) under MIN_TOTAL bytes → "data too short".
        let short = Bytes::from(full[..64 + 37 + 4 + 63].to_vec());
        match try_native_verify(P256_WEBAUTHN_VERIFIER_ADDRESS, &short, hash) {
            NativeVerifyResult::Invalid(reason) => {
                assert!(reason.contains("data too short"), "unexpected reason: {reason}");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }

        // (b) clientDataJSONLen overflows the blob: rewrite the 4-byte BE len
        // prefix to a huge value while keeping overall length unchanged.
        let mut overflow = full.clone();
        let len_off = 64 + 37;
        overflow[len_off..len_off + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        match try_native_verify(P256_WEBAUTHN_VERIFIER_ADDRESS, &Bytes::from(overflow), hash) {
            NativeVerifyResult::Invalid(reason) => {
                assert!(
                    reason.contains("clientDataJSONLen out of bounds"),
                    "unexpected reason: {reason}",
                );
            }
            other => panic!("expected Invalid, got {other:?}"),
        }

        // (c) extra trailing bytes → "trailing bytes".
        let mut trailing = full;
        trailing.push(0u8);
        match try_native_verify(P256_WEBAUTHN_VERIFIER_ADDRESS, &Bytes::from(trailing), hash) {
            NativeVerifyResult::Invalid(reason) => {
                assert!(reason.contains("trailing bytes"), "unexpected reason: {reason}");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn webauthn_bad_signature_invalid() {
        // Layout & challenge are valid, but the signature is over a different
        // message (signed_message_hash != challenge_hash).
        let sk = deterministic_p256_signing_key(17);
        let challenge_hash = B256::repeat_byte(0x42);
        let signed_over = B256::repeat_byte(0xFE);
        let blob = build_webauthn_blob(&sk, challenge_hash, signed_over);
        match try_native_verify(P256_WEBAUTHN_VERIFIER_ADDRESS, &Bytes::from(blob), challenge_hash)
        {
            NativeVerifyResult::Invalid(reason) => {
                assert!(reason.contains("verify failed"), "unexpected reason: {reason}");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn webauthn_missing_challenge_field_invalid() {
        // Hand-craft a blob where clientDataJSON has no `challenge` field.
        // serde_json fails the borrow-deserialize before we even reach the
        // challenge comparison — surfaces as the parse-failed reason.
        use p256::ecdsa::{Signature as P256Sig, signature::hazmat::PrehashSigner};
        use sha2::{Digest, Sha256};

        let sk = deterministic_p256_signing_key(19);
        let pk = sk.verifying_key();
        let sec1 = pk.to_encoded_point(false);
        let pk64 = &sec1.as_bytes()[1..];

        let auth_data: [u8; 37] = [0x33; 37];
        let cd = br#"{"type":"webauthn.get","origin":"https://x.test"}"#;
        let cd_len = (cd.len() as u32).to_be_bytes();

        let client_data_hash = Sha256::digest(cd);
        let mut h = Sha256::new();
        h.update(auth_data);
        h.update(client_data_hash);
        let msg = h.finalize();
        let sig: P256Sig = sk.sign_prehash(&msg).unwrap();

        let mut blob = Vec::new();
        blob.extend_from_slice(pk64);
        blob.extend_from_slice(&auth_data);
        blob.extend_from_slice(&cd_len);
        blob.extend_from_slice(cd);
        blob.extend_from_slice(&sig.to_bytes());

        match try_native_verify(P256_WEBAUTHN_VERIFIER_ADDRESS, &Bytes::from(blob), B256::ZERO) {
            NativeVerifyResult::Invalid(reason) => {
                assert!(
                    reason.contains("clientDataJSON parse failed"),
                    "unexpected reason: {reason}",
                );
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn webauthn_invalid_pubkey_invalid() {
        // 64 bytes of zeroes is not a valid P256 point.
        let sk = deterministic_p256_signing_key(23);
        let hash = B256::repeat_byte(0x10);
        let mut blob = build_webauthn_blob(&sk, hash, hash);
        for b in &mut blob[..64] {
            *b = 0;
        }
        match try_native_verify(P256_WEBAUTHN_VERIFIER_ADDRESS, &Bytes::from(blob), hash) {
            NativeVerifyResult::Invalid(reason) => {
                assert!(reason.contains("invalid pubkey"), "unexpected reason: {reason}");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn base64_crate_url_safe_no_pad_matches_independent_encoder() {
        // Smoke-test that the `base64` crate's URL_SAFE_NO_PAD output equals
        // the test helper's hand-rolled encoder on representative 32-byte
        // inputs — pins the encoding convention the verifier relies on for
        // challenge comparison.
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        for seed in [0x00u8, 0x01, 0xAB, 0xFF] {
            let raw = [seed; 32];
            let mut buf = [0u8; 43];
            let n = URL_SAFE_NO_PAD.encode_slice(raw, &mut buf).expect("43");
            assert_eq!(n, 43);
            let prod = core::str::from_utf8(&buf).expect("ascii");
            let test = b64url_encode(&raw);
            assert_eq!(prod, test.as_str(), "encoder mismatch on [{seed:#04x}; 32]");
        }
    }

    #[test]
    fn delegate_native_inner_verified() {
        let signer = signer_from_seed(0x22);
        let delegate_addr = Address::repeat_byte(0xCD);
        let hash = B256::repeat_byte(0xCC);

        let mut data = Vec::with_capacity(20 + 20 + 65);
        data.extend_from_slice(delegate_addr.as_slice());
        data.extend_from_slice(K1_VERIFIER_ADDRESS.as_slice());
        data.extend_from_slice(&k1_sig_blob(&signer, hash));

        match try_native_verify(DELEGATE_VERIFIER_ADDRESS, &Bytes::from(data), hash) {
            NativeVerifyResult::Verified(owner_id) => {
                assert_eq!(owner_id, address_to_owner_id(delegate_addr));
            }
            other => panic!("expected Verified, got {other:?}"),
        }
    }

    #[test]
    fn delegate_with_custom_inner_unsupported() {
        // delegate_addr || inner_verifier=0x77... || inner_data
        let mut data = Vec::with_capacity(60);
        data.extend_from_slice(Address::repeat_byte(0xCD).as_slice());
        data.extend_from_slice(Address::repeat_byte(0x77).as_slice());
        data.extend_from_slice(&[0u8; 20]);

        assert_eq!(
            try_native_verify(DELEGATE_VERIFIER_ADDRESS, &Bytes::from(data), B256::ZERO),
            NativeVerifyResult::Unsupported,
        );
    }

    #[test]
    fn delegate_inner_delegate_rejected_one_hop() {
        // Defense-in-depth: even if the auth-state builder didn't reject
        // Delegate→Delegate, `verify_delegate` itself does. Layout below is
        // outer delegate addr (20) || inner verifier=DELEGATE (20) || padding.
        let mut data = Vec::with_capacity(60);
        data.extend_from_slice(Address::repeat_byte(0xCD).as_slice());
        data.extend_from_slice(DELEGATE_VERIFIER_ADDRESS.as_slice());
        data.extend_from_slice(&[0u8; 20]);
        match try_native_verify(DELEGATE_VERIFIER_ADDRESS, &Bytes::from(data), B256::ZERO) {
            NativeVerifyResult::Invalid(reason) => {
                assert!(reason.contains("1-hop"), "unexpected reason: {reason}");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn delegate_too_short_invalid() {
        let bytes = Bytes::from(vec![0u8; 39]);
        match try_native_verify(DELEGATE_VERIFIER_ADDRESS, &bytes, B256::ZERO) {
            NativeVerifyResult::Invalid(reason) => {
                assert!(reason.contains("too short"), "unexpected reason: {reason}");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn custom_verifier_unsupported() {
        let bytes = Bytes::from(vec![0u8; 100]);
        assert_eq!(
            try_native_verify(Address::repeat_byte(0xAB), &bytes, B256::ZERO),
            NativeVerifyResult::Unsupported,
        );
    }

    // ── Ported from `tempo::tt_signature.rs` (xlayer-aligned hardening) ─────
    //
    // Coverage adapted to our wire format (pubkey at front, fixed 37-byte
    // auth_data, explicit length prefix). Tempo's wire-format-specific tests
    // (variable webauthn_data, tempo-signature enum decoding, address
    // derivation) are not portable and intentionally skipped.

    #[test]
    fn webauthn_flag_validation_at_rejected() {
        // Tempo: `test_webauthn_flag_validation` AT branch.
        // 0x41 = UP (0x01) | AT (0x40); AT must reject for assertion.
        let sk = deterministic_p256_signing_key(31);
        let hash = B256::repeat_byte(0x42);
        let blob = build_webauthn_blob_with_flags(&sk, hash, hash, 0x41);
        match try_native_verify(P256_WEBAUTHN_VERIFIER_ADDRESS, &Bytes::from(blob), hash) {
            NativeVerifyResult::Invalid(reason) => {
                assert!(reason.contains("AT flag"), "unexpected reason: {reason}");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn webauthn_flag_validation_ed_rejected() {
        // Tempo: `test_webauthn_flag_validation` ED branch.
        // 0x81 = UP (0x01) | ED (0x80); ED must reject — extensions unsupported.
        let sk = deterministic_p256_signing_key(32);
        let hash = B256::repeat_byte(0x42);
        let blob = build_webauthn_blob_with_flags(&sk, hash, hash, 0x81);
        match try_native_verify(P256_WEBAUTHN_VERIFIER_ADDRESS, &Bytes::from(blob), hash) {
            NativeVerifyResult::Invalid(reason) => {
                assert!(reason.contains("ED flag"), "unexpected reason: {reason}");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn webauthn_missing_up_and_uv_flags_rejected() {
        // Tempo: `test_webauthn_data_verification_missing_up_and_uv_flags`.
        // flags = 0x00 → neither UP nor UV → reject.
        let sk = deterministic_p256_signing_key(33);
        let hash = B256::repeat_byte(0x42);
        let blob = build_webauthn_blob_with_flags(&sk, hash, hash, 0x00);
        match try_native_verify(P256_WEBAUTHN_VERIFIER_ADDRESS, &Bytes::from(blob), hash) {
            NativeVerifyResult::Invalid(reason) => {
                assert!(reason.contains("neither UP nor UV"), "unexpected reason: {reason}",);
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn webauthn_uv_only_accepted() {
        // Tempo: `test_webauthn_data_verification_missing_up_and_uv_flags`
        // (positive branch). 0x04 = UV only — UV implies user presence per
        // spec, so verification must succeed.
        let sk = deterministic_p256_signing_key(34);
        let pk = sk.verifying_key();
        let sec1 = pk.to_encoded_point(false);
        let pk64 = &sec1.as_bytes()[1..];
        let hash = B256::repeat_byte(0x42);
        let blob = build_webauthn_blob_with_flags(&sk, hash, hash, 0x04);
        assert_eq!(
            try_native_verify(P256_WEBAUTHN_VERIFIER_ADDRESS, &Bytes::from(blob), hash),
            NativeVerifyResult::Verified(keccak256(pk64)),
        );
    }

    #[test]
    fn webauthn_invalid_type_rejected() {
        // Tempo: `test_webauthn_data_verification_invalid_type`.
        // Hand-craft a clientDataJSON with `type: "webauthn.create"` (the
        // registration ceremony type — must not be replayable as
        // authentication).
        use sha2::{Digest, Sha256};

        let sk = deterministic_p256_signing_key(35);
        let pk = sk.verifying_key();
        let sec1 = pk.to_encoded_point(false);
        let pk64 = &sec1.as_bytes()[1..];

        let hash = B256::repeat_byte(0x42);
        let challenge_b64 = b64url_encode(hash.as_slice());
        let cd = format!(
            r#"{{"type":"webauthn.create","challenge":"{}","origin":"https://x.test"}}"#,
            challenge_b64,
        );
        let cd_bytes = cd.as_bytes();

        let mut auth_data = [0u8; 37];
        auth_data[32] = 0x01;
        let cd_hash = Sha256::digest(cd_bytes);
        let mut h = Sha256::new();
        h.update(auth_data);
        h.update(cd_hash);
        let signed = h.finalize();
        let sig_bytes = sign_p256_low_s(&sk, &signed);

        let mut blob = Vec::with_capacity(64 + 37 + 4 + cd_bytes.len() + 64);
        blob.extend_from_slice(pk64);
        blob.extend_from_slice(&auth_data);
        blob.extend_from_slice(&(cd_bytes.len() as u32).to_be_bytes());
        blob.extend_from_slice(cd_bytes);
        blob.extend_from_slice(&sig_bytes);

        match try_native_verify(P256_WEBAUTHN_VERIFIER_ADDRESS, &Bytes::from(blob), hash) {
            NativeVerifyResult::Invalid(reason) => {
                assert!(reason.contains("type != webauthn.get"), "unexpected reason: {reason}",);
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn p256_raw_high_s_rejected() {
        // Tempo: `test_p256_high_s_rejection`. Sign normally, then flip s →
        // N - s to produce the high-s alternative. Both verify the same
        // message mathematically, but our verifier rejects high-s to close
        // signature malleability.
        use p256::ecdsa::SigningKey;

        let scalar = U256::from(42u64).to_be_bytes::<32>();
        let sk = SigningKey::from_bytes((&scalar).into()).expect("valid");
        let pk = sk.verifying_key();
        let sec1 = pk.to_encoded_point(false);
        let pk64 = &sec1.as_bytes()[1..];

        let hash = B256::repeat_byte(0xCD);
        // sign_p256_low_s normalizes; flip s back to high.
        let low = sign_p256_low_s(&sk, hash.as_slice());
        let s_low = U256::from_be_slice(&low[32..]);
        // P256 group order, hardcoded (matches `P256_N_HALF * 2 + 1`).
        let n: U256 =
            uint!(0xFFFFFFFF00000000FFFFFFFFFFFFFFFFBCE6FAADA7179E84F3B9CAC2FC632551_U256);
        let s_high = n - s_low;
        let mut high_sig = [0u8; 64];
        high_sig[..32].copy_from_slice(&low[..32]);
        high_sig[32..].copy_from_slice(&s_high.to_be_bytes::<32>());

        let mut data = Vec::with_capacity(128);
        data.extend_from_slice(pk64);
        data.extend_from_slice(&high_sig);

        match try_native_verify(P256_RAW_VERIFIER_ADDRESS, &Bytes::from(data), hash) {
            NativeVerifyResult::Invalid(reason) => {
                assert!(reason.contains("high-s"), "unexpected reason: {reason}");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn webauthn_high_s_rejected() {
        // Mirror of `p256_raw_high_s_rejected` but on the WebAuthn path.
        use sha2::{Digest, Sha256};

        let sk = deterministic_p256_signing_key(36);
        let pk = sk.verifying_key();
        let sec1 = pk.to_encoded_point(false);
        let pk64 = &sec1.as_bytes()[1..];

        let hash = B256::repeat_byte(0x42);
        let challenge_b64 = b64url_encode(hash.as_slice());
        let cd = format!(
            r#"{{"type":"webauthn.get","challenge":"{}","origin":"https://x.test"}}"#,
            challenge_b64,
        );
        let cd_bytes = cd.as_bytes();

        let mut auth_data = [0u8; 37];
        auth_data[32] = 0x01;
        let cd_hash = Sha256::digest(cd_bytes);
        let mut h = Sha256::new();
        h.update(auth_data);
        h.update(cd_hash);
        let signed = h.finalize();
        let low = sign_p256_low_s(&sk, &signed);
        let s_low = U256::from_be_slice(&low[32..]);
        let n: U256 =
            uint!(0xFFFFFFFF00000000FFFFFFFFFFFFFFFFBCE6FAADA7179E84F3B9CAC2FC632551_U256);
        let s_high = n - s_low;
        let mut high_sig = [0u8; 64];
        high_sig[..32].copy_from_slice(&low[..32]);
        high_sig[32..].copy_from_slice(&s_high.to_be_bytes::<32>());

        let mut blob = Vec::with_capacity(64 + 37 + 4 + cd_bytes.len() + 64);
        blob.extend_from_slice(pk64);
        blob.extend_from_slice(&auth_data);
        blob.extend_from_slice(&(cd_bytes.len() as u32).to_be_bytes());
        blob.extend_from_slice(cd_bytes);
        blob.extend_from_slice(&high_sig);

        match try_native_verify(P256_WEBAUTHN_VERIFIER_ADDRESS, &Bytes::from(blob), hash) {
            NativeVerifyResult::Invalid(reason) => {
                assert!(reason.contains("high-s"), "unexpected reason: {reason}");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }
}
