//! EIP-8130 wire-format signing helpers and EOA-mode K1 recovery.
//!
//! This module is the consensus-layer surface for EIP-8130 signature work:
//!
//! - [`sender_signature_hash`] / [`payer_signature_hash`]: keccak256 over the protocol-defined
//!   preimage. No crypto deps; used everywhere the auth payload is hashed.
//! - [`recover_eip8130_sender`]: K1 recovery for EOA-mode txs (`tx.from == None`). Used by
//!   `OpTxEnvelope` / `OpPooledTransaction` to surface the tx-level caller address, the same way
//!   EIP-1559/2930/etc. do via their own ECDSA signatures.
//! - [`encode_verify_call`] + [`IEip8130Verifier`]: ABI-encoded `verify(...)` calldata for
//!   STATICCALLs to custom-verifier contracts. Pure data, no crypto.
//!
//! **Higher-level auth resolution** — choosing between native verification,
//! custom-verifier STATICCALL deferral, and the EOA / explicit-from / sponsored
//! cases — is **not** here. That logic needs P256 / WebAuthn / Delegate-aware
//! dispatch and lives in [`alloy_op_evm::eip8130`] (EL-side) so the consensus
//! crate stays free of crypto deps beyond the K1 needed for the standard
//! envelope-recover trait surface.

#[cfg(feature = "k256")]
use alloy_consensus::crypto::{RecoveryError, secp256k1::recover_signer};
#[cfg(feature = "k256")]
use alloy_primitives::{Address, Signature, U256};
use alloy_primitives::{B256, Bytes, keccak256};
use alloy_sol_types::{SolCall, sol};

use super::TxEip8130;

sol! {
    /// EIP-8130 verifier interface. Used to derive the `verify(bytes32,bytes)`
    /// selector and ABI-encode calldata for custom-verifier STATICCALLs.
    interface IEip8130Verifier {
        function verify(bytes32 sigHash, bytes calldata data) external view returns (bool);
    }
}

/// Encodes a STATICCALL to `IEip8130Verifier.verify(sig_hash, data)`:
/// `selector("verify(bytes32,bytes)") || abi.encode(sig_hash, data)`.
///
/// Used by the EL-side auth-state builder when constructing a deferred
/// custom-verifier STATICCALL spec; the consumer fills in the verifier
/// address and account separately.
pub fn encode_verify_call(sig_hash: B256, data: &Bytes) -> Bytes {
    let call = IEip8130Verifier::verifyCall { sigHash: sig_hash, data: data.clone() };
    Bytes::from(call.abi_encode())
}

/// Returns the hash signed by `sender_auth`.
pub fn sender_signature_hash(tx: &TxEip8130) -> B256 {
    keccak256(tx.encoded_for_sender_signing())
}

/// Returns the hash signed by `payer_auth`.
pub fn payer_signature_hash(tx: &TxEip8130) -> B256 {
    keccak256(tx.encoded_for_payer_signing())
}

/// Recovers the EIP-8130 sender from either explicit `from` or EOA `sender_auth`.
///
/// For explicit-`from` mode (`tx.from.is_some()`) this returns `tx.from` **without**
/// verifying `sender_auth`. That's by design for the caller-recovery interface
/// used by reth's tx-iterator hook (it just needs an Address there). The full
/// EIP-8130 signature verification — including verifying the recovered key
/// matches `tx.from` in explicit-from mode and dispatching custom-verifier
/// STATICCALLs — happens in `alloy_op_evm::eip8130::auth_state` at conversion
/// time and is rejected at the handler-level via `validate_env`.
#[cfg(feature = "k256")]
pub fn recover_eip8130_sender(tx: &TxEip8130) -> Result<Address, RecoveryError> {
    if !tx.is_eoa() {
        return tx.from.ok_or_else(RecoveryError::new);
    }
    ecrecover_eoa_sender_auth(tx)
}

/// Decodes a bare 65-byte K1 signature blob (EOA mode) and recovers the signer
/// over `sender_signature_hash(tx)`.
///
/// EOA mode is `tx.from == None`; in that case the entire `sender_auth` is the 65-byte sig.
#[cfg(feature = "k256")]
fn ecrecover_eoa_sender_auth(tx: &TxEip8130) -> Result<Address, RecoveryError> {
    if tx.sender_auth.len() != 65 {
        return Err(RecoveryError::new());
    }
    recover_k1_signature(&tx.sender_auth, sender_signature_hash(tx))
}

/// Recovers the K1 signer from a 65-byte `[r(32) || s(32) || v(1)]` blob over
/// `sig_hash`. v must be `0`, `1`, `27`, or `28`.
#[cfg(feature = "k256")]
fn recover_k1_signature(sig_bytes: &[u8], sig_hash: B256) -> Result<Address, RecoveryError> {
    if sig_bytes.len() != 65 {
        return Err(RecoveryError::new());
    }
    let v = sig_bytes[64];
    let parity = match v {
        0 | 27 => false,
        1 | 28 => true,
        _ => return Err(RecoveryError::new()),
    };
    let signature = Signature::new(
        U256::from_be_slice(&sig_bytes[..32]),
        U256::from_be_slice(&sig_bytes[32..64]),
        parity,
    );
    recover_signer(&signature, sig_hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, Bytes, U256};

    #[test]
    fn sender_payer_hashes_are_deterministic() {
        let tx = TxEip8130 {
            chain_id: 8453,
            from: Some(Address::repeat_byte(0x01)),
            nonce_key: U256::ZERO,
            nonce_sequence: 42,
            expiry: 0,
            max_priority_fee_per_gas: 1_000_000_000,
            max_fee_per_gas: 10_000_000_000,
            gas_limit: 100_000,
            payer: Some(Address::repeat_byte(0x33)),
            calls: vec![vec![super::super::Eip8130CallEntry {
                to: Address::repeat_byte(0xBB),
                data: Bytes::from_static(&[0xDE, 0xAD]),
            }]],
            sender_auth: Bytes::from(vec![0x44; 65]),
            payer_auth: Bytes::from(vec![0x55; 65]),
            ..Default::default()
        };

        let sender_hash_1 = sender_signature_hash(&tx);
        let sender_hash_2 = sender_signature_hash(&tx);
        assert_eq!(sender_hash_1, sender_hash_2);

        let payer_hash_1 = payer_signature_hash(&tx);
        let payer_hash_2 = payer_signature_hash(&tx);
        assert_eq!(payer_hash_1, payer_hash_2);

        assert_ne!(sender_hash_1, payer_hash_1);
    }

    #[test]
    fn encode_verify_call_matches_selector_and_abi() {
        let expected_selector = &keccak256(b"verify(bytes32,bytes)").as_slice()[..4].to_vec();

        let sig_hash = B256::repeat_byte(0xAA);
        let data = Bytes::from(vec![0x01, 0x02, 0x03]);
        let encoded = encode_verify_call(sig_hash, &data);

        assert_eq!(&encoded[..4], expected_selector.as_slice());
        let decoded =
            IEip8130Verifier::verifyCall::abi_decode(&encoded).expect("verify(...) decode");
        assert_eq!(decoded.sigHash, sig_hash);
        assert_eq!(decoded.data, data);
    }
}
