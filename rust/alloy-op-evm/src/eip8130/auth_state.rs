//! Resolves `tx.sender_auth` / `tx.payer_auth` into [`AuthState`] for the handler.
//!
//! The wire format is one of two shapes:
//!
//! - **EOA mode** (`tx.is_eoa()`, i.e. `tx.sender.is_none()`): bare 65-byte K1 signature, no verifier
//!   prefix. Always treated as K1.
//! - **Explicit-from / sponsored mode**: `[verifier_addr(20) || data(N)]`. The first 20 bytes name
//!   the verifier; the remaining bytes are verifier-specific (K1: 65-byte sig, `P256Raw`: 128
//!   bytes, `WebAuthn`: variable, Delegate: nested, custom: opaque).
//!
//! For native verifiers we run the cryptography eagerly here and produce
//! [`AuthState::Native`]. For custom verifiers (and currently the `WebAuthn` stub
//! and Delegate→Custom) we produce [`AuthState::Deferred`] with the
//! pre-encoded STATICCALL spec.
//!
//! **Validator-side defense**: this runs once per tx-bytes-decoding on both
//! sequencer and validator. A sequencer that includes a tx with a forged or
//! missing `sender_auth` is caught when this returns [`AuthState::Invalid`]
//! and the handler rejects in `validate_env`.

use alloy_primitives::{Address, B256, Bytes};
// Note: `Bytes::slice(range)` is zero-copy (Arc-backed reference counting),
// strictly preferred over `Bytes::copy_from_slice(&bytes[range])` which
// allocates a fresh buffer. Hot-path conversion code uses slice.
use op_alloy::consensus::{
    TxEip8130, encode_verify_call, payer_signature_hash_with_sender, sender_signature_hash,
};
use op_revm::{
    constants::{
        DELEGATE_VERIFIER_ADDRESS, K1_VERIFIER_ADDRESS, OWNER_SCOPE_PAYER, OWNER_SCOPE_SENDER,
    },
    transaction::eip8130::{AuthState, DelegateInner, Eip8130VerifyCall},
};

use super::native_verifier::{NativeVerifyResult, address_to_owner_id, try_native_verify};

/// Builds the [`AuthState`] for `tx.sender_auth`.
///
/// EOA mode (`tx.sender.is_none()`): bare 65-byte K1 sig over
/// `sender_signature_hash(tx)`. The sender of the tx is whoever the sig
/// recovers to — this is what `recover_eip8130_sender` returns and what the
/// txpool seeds as `caller`. The on-chain `owner_config` check at execution
/// time covers `caller == owner_id` via the implicit-EOA fallback.
///
/// Explicit-from mode (`tx.sender.is_some()`):
/// - empty auth → [`AuthState::Empty`] (estimateGas escape)
/// - K1 verifier prefix: recover, **require recovered == `tx.sender`** (xlayer K1 strict-self-owner
///   invariant), produce [`AuthState::Native`]
/// - P256-raw verifier prefix: native verify, `owner_id` = `keccak256(pubkey)`
/// - P256-WebAuthn verifier prefix: native verify (SHA-256 challenge match + P256 ECDSA),
///   `owner_id` = `keccak256(pubkey)` → [`AuthState::Native`]
/// - Delegate→Native: eager native verification of inner, `owner_id` =
///   `bytes32(bytes20(delegate_addr))`
/// - Delegate→Custom: STATICCALL deferred via [`AuthState::Deferred`] with `delegate_outer =
///   Some(delegate_addr)` for the handler's outer-binding check after the inner STATICCALL succeeds
/// - Custom verifier (any unknown 20-byte prefix): [`AuthState::Deferred`]
/// - malformed (auth shorter than 20 bytes in explicit-from): [`AuthState::Invalid`]
pub fn build_sender_auth_state(tx: &TxEip8130) -> AuthState {
    let sig_hash = sender_signature_hash(tx);

    // EOA mode: bare 65-byte K1 sig. No verifier prefix.
    if tx.is_eoa() {
        if tx.sender_auth.is_empty() {
            // EOA without auth: cannot recover, must reject.
            return AuthState::Invalid("sender_auth: EOA mode missing sig".into());
        }
        return match try_native_verify(K1_VERIFIER_ADDRESS, &tx.sender_auth, sig_hash) {
            NativeVerifyResult::Verified(owner_id) => {
                AuthState::Native { verifier: K1_VERIFIER_ADDRESS, owner_id, delegate_inner: None }
            }
            // Bubble the native-verifier reason through unchanged.
            NativeVerifyResult::Invalid(reason) => AuthState::Invalid(reason),
            // Should not happen for K1 (well-known native verifier), but stay
            // defensive: surface a stable reason rather than silently
            // converting Unsupported to a generic Invalid.
            NativeVerifyResult::Unsupported => {
                AuthState::Invalid("sender_auth: EOA K1 unexpectedly unsupported".into())
            }
        };
    }

    // Explicit-from mode: tx.sender must be Some.
    let from = match tx.sender {
        Some(addr) => addr,
        None => return AuthState::Invalid("sender_auth: explicit-from missing tx.sender".into()),
    };

    if tx.sender_auth.is_empty() {
        return AuthState::Empty;
    }

    if tx.sender_auth.len() < 20 {
        return AuthState::Invalid("sender_auth: too short for verifier prefix".into());
    }

    let verifier_addr = Address::from_slice(&tx.sender_auth[..20]);
    let verifier_data = tx.sender_auth.slice(20..);

    resolve_explicit_auth(
        verifier_addr,
        verifier_data,
        sig_hash,
        from,
        OWNER_SCOPE_SENDER,
        StrictK1Binding::ToFrom(from),
    )
}

/// Builds the [`AuthState`] for `tx.payer_auth`.
///
/// Self-pay (`tx.is_self_pay() == true`) short-circuits to [`AuthState::SelfPay`]
/// without inspecting `payer_auth`.
///
/// Sponsored mode mirrors explicit-from sender resolution, but with two
/// differences: K1 strict-self-owner is checked against `tx.payer` (not
/// `tx.sender`), and `required_scope` is [`OWNER_SCOPE_PAYER`].
pub fn build_payer_auth_state(tx: &TxEip8130, resolved_sender: Address) -> AuthState {
    if tx.is_self_pay() {
        return AuthState::SelfPay;
    }

    let payer = match tx.payer {
        Some(addr) => addr,
        // Defensive: `is_self_pay() == false` but `payer` is None.
        None => return AuthState::Invalid("payer_auth: sponsored mode missing tx.payer".into()),
    };

    if tx.payer_auth.is_empty() {
        return AuthState::Empty;
    }

    if tx.payer_auth.len() < 20 {
        return AuthState::Invalid("payer_auth: too short for verifier prefix".into());
    }

    let sig_hash = payer_signature_hash_with_sender(tx, resolved_sender);
    let verifier_addr = Address::from_slice(&tx.payer_auth[..20]);
    let verifier_data = tx.payer_auth.slice(20..);

    resolve_explicit_auth(
        verifier_addr,
        verifier_data,
        sig_hash,
        payer,
        OWNER_SCOPE_PAYER,
        StrictK1Binding::ToFrom(payer),
    )
}

/// Whether to enforce the K1 strict-self-owner invariant.
///
/// xlayer-specific: for K1 native verification in *explicit-from / sponsored*
/// mode, the recovered K1 address must equal the claimed `tx.sender` (sender) or
/// `tx.payer` (payer). This makes K1 explicit-from authenticate "the account
/// itself signed" — different K1 owners must use a different verifier or be
/// reached via an `owner_config` registration of their bytes20 `owner_id` which
/// the implicit-EOA fallback won't match.
///
/// (EOA-mode K1 doesn't go through this path — there's no claimed-from to
/// check against.)
#[derive(Clone, Copy, Debug)]
enum StrictK1Binding {
    /// Strict K1: returned `owner_id` must equal `bytes32(bytes20(addr))`.
    ToFrom(Address),
}

/// Shared explicit-from / sponsored auth resolver for sender + payer.
fn resolve_explicit_auth(
    verifier_addr: Address,
    verifier_data: Bytes,
    sig_hash: B256,
    account: Address,
    required_scope: u8,
    strict_k1: StrictK1Binding,
) -> AuthState {
    // Delegate gets its own resolver — it needs to surface the inner
    // (verifier, owner_id) pair so the handler can do the inner-binding
    // check on the delegate account's owner_config. Native dispatch alone
    // would lose that information.
    if verifier_addr == DELEGATE_VERIFIER_ADDRESS {
        return resolve_delegate_auth(&verifier_data, sig_hash, required_scope);
    }

    match try_native_verify(verifier_addr, &verifier_data, sig_hash) {
        NativeVerifyResult::Verified(owner_id) => {
            if verifier_addr == K1_VERIFIER_ADDRESS {
                // Strict K1 binding — recovered owner_id must equal bytes20(account).
                let StrictK1Binding::ToFrom(addr) = strict_k1;
                if owner_id != address_to_owner_id(addr) {
                    return AuthState::Invalid(
                        "K1 strict-self-owner: recovered != claimed from".into(),
                    );
                }
            }
            AuthState::Native { verifier: verifier_addr, owner_id, delegate_inner: None }
        }
        // Pass the native-verifier reason through untouched.
        NativeVerifyResult::Invalid(reason) => AuthState::Invalid(reason),
        NativeVerifyResult::Unsupported => AuthState::Deferred {
            spec: Eip8130VerifyCall {
                verifier: verifier_addr,
                calldata: encode_verify_call(sig_hash, &verifier_data),
                account,
                required_scope,
            },
            delegate_outer: None,
        },
    }
}

/// Resolves a Delegate-verifier auth blob into an [`AuthState`].
///
/// Layout: `verifier_data = delegate_addr(20) || inner_verifier(20) || inner_data`.
///
/// Branches on the inner verifier:
/// - **Inner native** (K1 / `P256Raw` / WebAuthn): runs the inner crypto eagerly and returns
///   [`AuthState::Native`] with `verifier = DELEGATE_VERIFIER_ADDRESS`, `owner_id =
///   bytes32(bytes20(delegate_addr))`, and `delegate_inner = Some(DelegateInner { verifier:
///   inner_verifier, owner_id: inner_owner_id })`. The handler runs both bindings on this state.
/// - **Inner custom**: returns [`AuthState::Deferred`] with the inner STATICCALL spec (account =
///   `delegate_addr`) and `delegate_outer = Some(delegate_addr)` for the handler's outer-binding
///   check after the STATICCALL.
/// - **Inner verify failed**: [`AuthState::Invalid`].
fn resolve_delegate_auth(verifier_data: &Bytes, sig_hash: B256, required_scope: u8) -> AuthState {
    if verifier_data.len() < 40 {
        return AuthState::Invalid("delegate: blob too short".into());
    }
    let delegate_addr = Address::from_slice(&verifier_data[..20]);
    let inner_verifier = Address::from_slice(&verifier_data[20..40]);

    // 1-hop only: reject Delegate→Delegate before recursing. Without this guard
    // a chained delegate auth would slip through with the leaf K1/P256 sig
    // mathematically valid but never checked against the innermost delegate's
    // owner_config (the recursion only surfaces the *intermediate* delegate's
    // owner_id). Defense-in-depth — `verify_delegate` also has this guard.
    if inner_verifier == DELEGATE_VERIFIER_ADDRESS {
        return AuthState::Invalid("delegate: 1-hop limit (inner is delegate)".into());
    }

    let inner_data = verifier_data.slice(40..);

    match try_native_verify(inner_verifier, &inner_data, sig_hash) {
        NativeVerifyResult::Verified(inner_owner_id) => AuthState::Native {
            verifier: DELEGATE_VERIFIER_ADDRESS,
            owner_id: address_to_owner_id(delegate_addr),
            delegate_inner: Some(DelegateInner {
                verifier: inner_verifier,
                owner_id: inner_owner_id,
            }),
        },
        // Bubble the inner native-verifier reason up unchanged.
        NativeVerifyResult::Invalid(reason) => AuthState::Invalid(reason),
        NativeVerifyResult::Unsupported => AuthState::Deferred {
            spec: Eip8130VerifyCall {
                verifier: inner_verifier,
                calldata: encode_verify_call(sig_hash, &inner_data),
                // Inner STATICCALL: "is the recovered owner valid for the
                // delegate account?" — account is the delegate, not the outer
                // sender/payer.
                account: delegate_addr,
                required_scope,
            },
            delegate_outer: Some(delegate_addr),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, B256, Bytes, U256};
    use alloy_signer::SignerSync;
    use alloy_signer_local::PrivateKeySigner;
    use op_alloy::consensus::Eip8130CallEntry;

    fn signer_from_seed(seed: u8) -> PrivateKeySigner {
        let bytes = B256::repeat_byte(seed);
        PrivateKeySigner::from_bytes(&bytes).expect("valid private key")
    }

    /// Build a sample tx with parameterized `from` and `sender_auth`.
    fn sample_tx(from: Option<Address>, sender_auth: Bytes) -> TxEip8130 {
        TxEip8130 {
            chain_id: 8453,
            from,
            nonce_key: U256::ZERO,
            nonce_sequence: 42,
            expiry: 0,
            max_priority_fee_per_gas: 1_000_000_000,
            max_fee_per_gas: 10_000_000_000,
            gas_limit: 100_000,
            payer: Some(Address::repeat_byte(0x33)),
            calls: vec![vec![Eip8130CallEntry {
                to: Address::repeat_byte(0xBB),
                data: Bytes::from_static(&[0xDE, 0xAD]),
            }]],
            sender_auth,
            payer_auth: Bytes::new(),
            ..Default::default()
        }
    }

    fn sample_tx_payer(payer: Option<Address>, payer_auth: Bytes) -> TxEip8130 {
        TxEip8130 {
            chain_id: 8453,
            sender: Some(Address::repeat_byte(0x01)),
            nonce_key: U256::ZERO,
            nonce_sequence: 42,
            expiry: 0,
            max_priority_fee_per_gas: 1_000_000_000,
            max_fee_per_gas: 10_000_000_000,
            gas_limit: 100_000,
            payer,
            calls: vec![vec![Eip8130CallEntry {
                to: Address::repeat_byte(0xBB),
                data: Bytes::from_static(&[0xDE, 0xAD]),
            }]],
            sender_auth: Bytes::new(),
            payer_auth,
            ..Default::default()
        }
    }

    fn k1_sig_blob(signer: &PrivateKeySigner, hash: B256) -> Vec<u8> {
        let sig = signer.sign_hash_sync(&hash).expect("sign");
        let mut buf = Vec::with_capacity(65);
        buf.extend_from_slice(&sig.r().to_be_bytes::<32>());
        buf.extend_from_slice(&sig.s().to_be_bytes::<32>());
        buf.push(if sig.v() { 1 } else { 0 });
        buf
    }

    fn k1_explicit_auth(signer: &PrivateKeySigner, hash: B256) -> Bytes {
        let mut buf = Vec::with_capacity(85);
        buf.extend_from_slice(K1_VERIFIER_ADDRESS.as_slice());
        buf.extend_from_slice(&k1_sig_blob(signer, hash));
        Bytes::from(buf)
    }

    fn expected_owner_id(addr: Address) -> B256 {
        let mut buf = [0u8; 32];
        buf[..20].copy_from_slice(addr.as_slice());
        B256::from(buf)
    }

    // ── sender ──

    #[test]
    fn explicit_from_empty_returns_empty() {
        let addr = Address::repeat_byte(0x01);
        let tx = sample_tx(Some(addr), Bytes::new());
        assert_eq!(build_sender_auth_state(&tx), AuthState::Empty);
    }

    #[test]
    fn eoa_empty_returns_invalid() {
        let tx = sample_tx(None, Bytes::new());
        match build_sender_auth_state(&tx) {
            AuthState::Invalid(reason) => {
                assert!(reason.contains("EOA mode missing"), "unexpected reason: {reason}");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn explicit_from_k1_round_trip() {
        let signer = signer_from_seed(0x11);
        let mut tx = sample_tx(Some(signer.address()), Bytes::new());
        let hash = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(&signer, hash);

        assert_eq!(
            build_sender_auth_state(&tx),
            AuthState::Native {
                verifier: K1_VERIFIER_ADDRESS,
                owner_id: expected_owner_id(signer.address()),
                delegate_inner: None,
            }
        );
    }

    #[test]
    fn eoa_k1_round_trip() {
        let signer = signer_from_seed(0x22);
        let mut tx = sample_tx(None, Bytes::new());
        let hash = sender_signature_hash(&tx);
        tx.sender_auth = Bytes::from(k1_sig_blob(&signer, hash));

        assert_eq!(
            build_sender_auth_state(&tx),
            AuthState::Native {
                verifier: K1_VERIFIER_ADDRESS,
                owner_id: expected_owner_id(signer.address()),
                delegate_inner: None,
            }
        );
    }

    #[test]
    fn explicit_from_k1_wrong_signer_invalid() {
        let victim = signer_from_seed(0x44);
        let attacker = signer_from_seed(0x55);
        assert_ne!(victim.address(), attacker.address());

        let mut tx = sample_tx(Some(victim.address()), Bytes::new());
        let hash = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(&attacker, hash);

        match build_sender_auth_state(&tx) {
            AuthState::Invalid(reason) => {
                assert!(reason.contains("strict-self-owner"), "unexpected reason: {reason}",);
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn auth_too_short_invalid() {
        let addr = Address::repeat_byte(0x77);
        let tx = sample_tx(Some(addr), Bytes::from(vec![0u8; 19]));
        match build_sender_auth_state(&tx) {
            AuthState::Invalid(reason) => {
                assert!(reason.contains("too short"), "unexpected reason: {reason}");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn custom_verifier_returns_deferred() {
        let addr = Address::repeat_byte(0xAA);
        let custom = Address::repeat_byte(0xBE);
        let mut blob = Vec::with_capacity(120);
        blob.extend_from_slice(custom.as_slice());
        blob.extend_from_slice(&[0x77u8; 100]);
        let tx = sample_tx(Some(addr), Bytes::from(blob));

        match build_sender_auth_state(&tx) {
            AuthState::Deferred { spec, delegate_outer } => {
                assert_eq!(spec.verifier, custom);
                assert_eq!(spec.account, addr);
                assert_eq!(spec.required_scope, OWNER_SCOPE_SENDER);
                assert!(delegate_outer.is_none());
            }
            other => panic!("expected Deferred, got {other:?}"),
        }
    }

    #[test]
    fn delegate_custom_inner_returns_deferred_with_outer() {
        // verifier = DELEGATE; inner = [delegate_addr || custom_inner_verifier || inner_data].
        let sender = Address::repeat_byte(0xAA);
        let delegate = Address::repeat_byte(0xCD);
        let inner_custom = Address::repeat_byte(0xEF);

        let mut blob = Vec::with_capacity(20 + 20 + 20 + 30);
        blob.extend_from_slice(DELEGATE_VERIFIER_ADDRESS.as_slice());
        blob.extend_from_slice(delegate.as_slice());
        blob.extend_from_slice(inner_custom.as_slice());
        blob.extend_from_slice(&[0xAB; 30]);
        let tx = sample_tx(Some(sender), Bytes::from(blob));

        match build_sender_auth_state(&tx) {
            AuthState::Deferred { spec, delegate_outer } => {
                assert_eq!(spec.verifier, inner_custom);
                assert_eq!(spec.account, delegate);
                assert_eq!(spec.required_scope, OWNER_SCOPE_SENDER);
                assert_eq!(delegate_outer, Some(delegate));
            }
            other => panic!("expected Deferred, got {other:?}"),
        }
    }

    #[test]
    fn delegate_with_delegate_inner_rejected_one_hop() {
        // Spec: delegation is 1-hop. A chained Delegate→Delegate→Native auth
        // must be rejected at parse time — otherwise the innermost native sig
        // would only be checked against the *intermediate* delegate's
        // address, never against the leaf delegate's owner_config.
        let signer = signer_from_seed(0x77);
        let sender = Address::repeat_byte(0xAA);
        let delegate1 = Address::repeat_byte(0xCD);
        let delegate2 = Address::repeat_byte(0xCE);

        let mut tx = sample_tx(Some(sender), Bytes::new());
        let hash = sender_signature_hash(&tx);

        let mut blob = Vec::with_capacity(20 + 20 + 20 + 20 + 20 + 65);
        blob.extend_from_slice(DELEGATE_VERIFIER_ADDRESS.as_slice());
        blob.extend_from_slice(delegate1.as_slice());
        blob.extend_from_slice(DELEGATE_VERIFIER_ADDRESS.as_slice()); // <- inner is delegate too
        blob.extend_from_slice(delegate2.as_slice());
        blob.extend_from_slice(K1_VERIFIER_ADDRESS.as_slice());
        blob.extend_from_slice(&k1_sig_blob(&signer, hash));
        tx.sender_auth = Bytes::from(blob);

        match build_sender_auth_state(&tx) {
            AuthState::Invalid(reason) => {
                assert!(reason.contains("1-hop"), "unexpected reason: {reason}");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn delegate_native_inner_returns_native() {
        let signer = signer_from_seed(0x33);
        let sender = Address::repeat_byte(0xAA);
        let delegate = Address::repeat_byte(0xCD);

        // Compute the hash via empty-auth tx, then sign.
        let mut tx = sample_tx(Some(sender), Bytes::new());
        let hash = sender_signature_hash(&tx);

        let mut blob = Vec::with_capacity(20 + 20 + 20 + 65);
        blob.extend_from_slice(DELEGATE_VERIFIER_ADDRESS.as_slice());
        blob.extend_from_slice(delegate.as_slice());
        blob.extend_from_slice(K1_VERIFIER_ADDRESS.as_slice());
        blob.extend_from_slice(&k1_sig_blob(&signer, hash));
        tx.sender_auth = Bytes::from(blob);

        // Outer (verifier=DELEGATE, owner_id=bytes20(delegate)) plus the
        // inner (K1, bytes20(signer.address())) so the handler can do the
        // owner_config[delegate][bytes20(signer)] = (K1, scope) inner check.
        assert_eq!(
            build_sender_auth_state(&tx),
            AuthState::Native {
                verifier: DELEGATE_VERIFIER_ADDRESS,
                owner_id: expected_owner_id(delegate),
                delegate_inner: Some(DelegateInner {
                    verifier: K1_VERIFIER_ADDRESS,
                    owner_id: expected_owner_id(signer.address()),
                }),
            }
        );
    }

    // ── payer ──

    #[test]
    fn payer_self_pay() {
        let tx = sample_tx_payer(None, Bytes::new());
        assert!(tx.is_self_pay());
        assert_eq!(build_payer_auth_state(&tx, tx.effective_sender()), AuthState::SelfPay);
    }

    #[test]
    fn payer_sponsored_empty_returns_empty() {
        let payer = Address::repeat_byte(0x33);
        let tx = sample_tx_payer(Some(payer), Bytes::new());
        assert_eq!(build_payer_auth_state(&tx, tx.effective_sender()), AuthState::Empty);
    }

    #[test]
    fn payer_sponsored_k1_round_trip() {
        let payer_signer = signer_from_seed(0x88);
        let mut tx = sample_tx_payer(Some(payer_signer.address()), Bytes::new());
        let hash = payer_signature_hash_with_sender(&tx, tx.effective_sender());
        tx.payer_auth = k1_explicit_auth(&payer_signer, hash);

        assert_eq!(
            build_payer_auth_state(&tx, tx.effective_sender()),
            AuthState::Native {
                verifier: K1_VERIFIER_ADDRESS,
                owner_id: expected_owner_id(payer_signer.address()),
                delegate_inner: None,
            }
        );
    }

    #[test]
    fn payer_sponsored_k1_wrong_signer_invalid() {
        let payer = signer_from_seed(0x99);
        let attacker = signer_from_seed(0xAA);
        let mut tx = sample_tx_payer(Some(payer.address()), Bytes::new());
        let hash = payer_signature_hash_with_sender(&tx, tx.effective_sender());
        tx.payer_auth = k1_explicit_auth(&attacker, hash);

        match build_payer_auth_state(&tx, tx.effective_sender()) {
            AuthState::Invalid(reason) => {
                assert!(reason.contains("strict-self-owner"), "unexpected reason: {reason}",);
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn payer_eoa_hash_rejects_cross_sender_replay() {
        let sender = signer_from_seed(0xB1);
        let other_sender = signer_from_seed(0xB2);
        let payer = signer_from_seed(0xB3);
        let mut tx = sample_tx(None, Bytes::new());
        tx.payer = Some(payer.address());

        let sender_hash = sender_signature_hash(&tx);
        tx.sender_auth = Bytes::from(k1_sig_blob(&sender, sender_hash));
        let payer_hash = payer_signature_hash_with_sender(&tx, sender.address());
        tx.payer_auth = k1_explicit_auth(&payer, payer_hash);

        assert_eq!(
            build_payer_auth_state(&tx, sender.address()),
            AuthState::Native {
                verifier: K1_VERIFIER_ADDRESS,
                owner_id: expected_owner_id(payer.address()),
                delegate_inner: None,
            }
        );

        match build_payer_auth_state(&tx, other_sender.address()) {
            AuthState::Invalid(reason) => {
                assert!(reason.contains("strict-self-owner"), "unexpected reason: {reason}");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn payer_sponsored_custom_returns_deferred() {
        let payer = Address::repeat_byte(0xCD);
        let custom = Address::repeat_byte(0xEF);
        let mut blob = Vec::with_capacity(100);
        blob.extend_from_slice(custom.as_slice());
        blob.extend_from_slice(&[0x42u8; 80]);
        let tx = sample_tx_payer(Some(payer), Bytes::from(blob));

        match build_payer_auth_state(&tx, tx.effective_sender()) {
            AuthState::Deferred { spec, delegate_outer } => {
                assert_eq!(spec.verifier, custom);
                assert_eq!(spec.account, payer);
                assert_eq!(spec.required_scope, OWNER_SCOPE_PAYER);
                assert!(delegate_outer.is_none());
            }
            other => panic!("expected Deferred, got {other:?}"),
        }
    }
}
