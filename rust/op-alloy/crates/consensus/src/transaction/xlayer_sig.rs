//! EIP-8130 signature hash computation and auth parsing.

#[cfg(feature = "k256")]
use alloy_consensus::crypto::{RecoveryError, secp256k1::recover_signer};
#[cfg(feature = "k256")]
use alloy_primitives::Signature;
#[cfg(feature = "k256")]
use alloy_primitives::{Address, U256};
use alloy_primitives::{B256, keccak256};

use super::TxEip8130;

/// Returns the hash signed by `sender_auth`.
pub fn sender_signature_hash(tx: &TxEip8130) -> B256 {
    keccak256(tx.encoded_for_sender_signing())
}

/// Returns the hash signed by `payer_auth`.
pub fn payer_signature_hash(tx: &TxEip8130) -> B256 {
    keccak256(tx.encoded_for_payer_signing())
}

/// Recovers the EIP-8130 sender from either explicit `from` or EOA `sender_auth`.
#[cfg(feature = "k256")]
pub fn recover_eip8130_sender(tx: &TxEip8130) -> Result<Address, RecoveryError> {
    if !tx.is_eoa() {
        return tx.from.ok_or_else(RecoveryError::new);
    }

    if tx.sender_auth.len() != 65 {
        return Err(RecoveryError::new());
    }

    let sig_hash = sender_signature_hash(tx);
    let v = tx.sender_auth[64];
    let parity = match v {
        0 | 27 => false,
        1 | 28 => true,
        _ => return Err(RecoveryError::new()),
    };
    let signature = Signature::new(
        U256::from_be_slice(&tx.sender_auth[..32]),
        U256::from_be_slice(&tx.sender_auth[32..64]),
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
            calls: vec![vec![super::super::Call {
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
}
