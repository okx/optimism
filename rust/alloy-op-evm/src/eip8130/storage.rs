//! Storage-slot derivation for EIP-8130 system contracts.
//!
//! Mirrors Base's `eip8130/storage.rs`: deterministic slot computation for
//! `AccountConfiguration._ownerConfig`, `AccountConfiguration._accountState`,
//! and `NonceManager.nonce`. The handler consumes these as
//! [`op_revm::transaction::eip8130::Eip8130StorageWrite::slot`] inputs; on
//! the Solidity side the deployed contracts compute the same slots so the
//! protocol-written entries are visible to contract reads.

use alloy_primitives::{Address, B256, U256, keccak256, uint};

/// Base slot for `AccountConfiguration._ownerConfig`:
/// `mapping(bytes32 ownerId => mapping(address account => OwnerConfig))`.
///
/// Solidity: `_ownerConfig` is the first state variable, slot 0.
const OWNER_CONFIG_BASE_SLOT: U256 = uint!(0_U256);

/// Base slot for `AccountConfiguration._accountState`:
/// `mapping(address account => AccountState)` — packed
/// `multichainSequence(8) | localSequence(8) | unlocksAt(5) | unlockDelay(2) | …`.
const ACCOUNT_STATE_BASE_SLOT: U256 = uint!(1_U256);

/// `keccak256(account || keccak256(owner_id || OWNER_CONFIG_BASE_SLOT))`.
///
/// Storage layout for nested mapping `m[ownerId][account]`. The 12-byte
/// left-padding on the address half mirrors Solidity's `abi.encode`.
pub fn owner_config_slot(account: Address, owner_id: B256) -> B256 {
    let inner = {
        let mut buf = [0u8; 64];
        buf[..32].copy_from_slice(owner_id.as_slice());
        buf[32..].copy_from_slice(&OWNER_CONFIG_BASE_SLOT.to_be_bytes::<32>());
        keccak256(buf)
    };
    let mut buf = [0u8; 64];
    buf[12..32].copy_from_slice(account.as_slice());
    buf[32..].copy_from_slice(inner.as_slice());
    keccak256(buf)
}

/// `keccak256(account || ACCOUNT_STATE_BASE_SLOT)` — packed `_accountState`
/// word holding both sequences and lock fields.
pub fn account_state_slot(account: Address) -> B256 {
    let mut buf = [0u8; 64];
    buf[12..32].copy_from_slice(account.as_slice());
    buf[32..].copy_from_slice(&ACCOUNT_STATE_BASE_SLOT.to_be_bytes::<32>());
    keccak256(buf)
}

/// Encodes `(verifier, scope)` into an `OwnerConfig` storage word.
///
/// Solidity packs the struct right-aligned: `zeros(11) | scope(1) | verifier(20)`.
/// Bytes 12..32 = verifier (low-order); byte 11 = scope; bytes 0..11 = zeros.
pub fn encode_owner_config(verifier: Address, scope: u8) -> B256 {
    let mut bytes = [0u8; 32];
    bytes[12..32].copy_from_slice(verifier.as_slice());
    bytes[11] = scope;
    B256::from(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_config_slot_deterministic() {
        let account = Address::repeat_byte(0x01);
        let owner_id = B256::repeat_byte(0x02);
        assert_eq!(owner_config_slot(account, owner_id), owner_config_slot(account, owner_id));
    }

    #[test]
    fn owner_config_slot_distinguishes_account_and_owner() {
        let account_a = Address::repeat_byte(0x01);
        let account_b = Address::repeat_byte(0x03);
        let owner_a = B256::repeat_byte(0x02);
        let owner_b = B256::repeat_byte(0x04);
        assert_ne!(owner_config_slot(account_a, owner_a), owner_config_slot(account_b, owner_a));
        assert_ne!(owner_config_slot(account_a, owner_a), owner_config_slot(account_a, owner_b));
    }

    #[test]
    fn account_state_slot_deterministic() {
        let account = Address::repeat_byte(0x01);
        assert_eq!(account_state_slot(account), account_state_slot(account));
    }

    #[test]
    fn account_state_slot_per_account() {
        assert_ne!(
            account_state_slot(Address::repeat_byte(0x01)),
            account_state_slot(Address::repeat_byte(0x02)),
        );
    }

    #[test]
    fn encode_owner_config_packing() {
        let verifier = Address::repeat_byte(0xAA);
        let encoded = encode_owner_config(verifier, 0x0F);
        assert_eq!(&encoded.as_slice()[..11], &[0u8; 11]);
        assert_eq!(encoded.as_slice()[11], 0x0F);
        assert_eq!(&encoded.as_slice()[12..32], verifier.as_slice());
    }
}
