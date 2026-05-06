//! EIP-8130 CREATE2 address derivation for account creation.
//!
//! Mirrors Base's `eip8130/address.rs` reference: a Create entry's deployed
//! address is `CREATE2(deployer = ACCOUNT_CONFIG_ADDRESS, effective_salt,
//! init_code = deployment_header(len) || bytecode)`. The header is a tiny
//! EVM program that RETURNs `bytecode` as the runtime code, so the runtime
//! placement bytes are exactly `bytecode` (no header stays at the address).
//!
//! `effective_salt = keccak256(user_salt || keccak256(sorted owners))` —
//! sorting owners by `owner_id` makes the derived address independent of the
//! order in which the entry lists its initial owners.

use alloc::vec::Vec;
use alloy_primitives::{Address, B256, Bytes, keccak256};
use op_alloy::consensus::Owner;

/// Deployment header length: a fixed 14-byte EVM program that RETURNs the
/// bytecode appended after it.
const DEPLOYMENT_HEADER_LEN: usize = 14;

/// Builds the 14-byte EVM deployment header that wraps arbitrary bytecode for
/// CREATE2.
///
/// ```text
/// PUSH2 <len_hi> <len_lo>   // 0x61 LL HH — push runtime size
/// DUP1                       // 0x80
/// PUSH1 0x0e                 // 0x600e — header size = code offset
/// PUSH1 0x00                 // 0x6000
/// CODECOPY                   // 0x39
/// PUSH1 0x00                 // 0x6000
/// RETURN                     // 0xf3
/// ```
const fn deployment_header(bytecode_len: usize) -> [u8; DEPLOYMENT_HEADER_LEN] {
    // PUSH2 encodes a 16-bit immediate: bytecode_len cannot exceed u16::MAX.
    // EIP-170 (`MAX_CODE_SIZE = 24576`) is well below this; `validate_env`
    // is responsible for rejecting oversized bytecode upstream. Assert
    // here so a caller bypassing validation gets a deterministic panic
    // rather than silent truncation that would mismatch the CREATE2
    // address derivation.
    assert!(bytecode_len <= u16::MAX as usize, "EIP-8130: bytecode > u16::MAX");
    let len = bytecode_len as u16;
    let hi = (len >> 8) as u8;
    let lo = (len & 0xFF) as u8;
    [
        0x61, hi, lo, // PUSH2 len
        0x80, // DUP1
        0x60, 0x0e, // PUSH1 14 (header size = code offset)
        0x60, 0x00, // PUSH1 0
        0x39, // CODECOPY
        0x60, 0x00, // PUSH1 0
        0xf3, // RETURN
        0x00, 0x00, // padding to 14 bytes
    ]
}

/// `header(len) || bytecode` — the CREATE2 init code.
fn deployment_code(bytecode: &[u8]) -> Vec<u8> {
    let header = deployment_header(bytecode.len());
    let mut code = Vec::with_capacity(header.len() + bytecode.len());
    code.extend_from_slice(&header);
    code.extend_from_slice(bytecode);
    code
}

/// Computes the `effective_salt` for CREATE2 address derivation.
///
/// Owners are sorted by `owner_id` before commitment so the derived address
/// is independent of the listing order.
fn effective_salt(user_salt: B256, initial_owners: &[Owner]) -> B256 {
    let mut sorted: Vec<&Owner> = initial_owners.iter().collect();
    sorted.sort_by_key(|o| o.owner_id);

    // 32 (owner_id) + 20 (verifier) + 1 (scope) per owner.
    let mut commitment_input = Vec::with_capacity(sorted.len() * 53);
    for owner in &sorted {
        commitment_input.extend_from_slice(owner.owner_id.as_slice());
        commitment_input.extend_from_slice(owner.verifier.as_slice());
        commitment_input.push(owner.scope);
    }
    let owners_commitment = keccak256(&commitment_input);

    let mut salt_input = [0u8; 64];
    salt_input[..32].copy_from_slice(user_salt.as_slice());
    salt_input[32..].copy_from_slice(owners_commitment.as_slice());
    keccak256(salt_input)
}

/// `keccak256(0xff || deployer || salt || keccak256(init_code))[12:]`.
fn create2_address(deployer: Address, salt: B256, init_code: &[u8]) -> Address {
    let init_code_hash = keccak256(init_code);
    let mut buf = [0u8; 1 + 20 + 32 + 32];
    buf[0] = 0xFF;
    buf[1..21].copy_from_slice(deployer.as_slice());
    buf[21..53].copy_from_slice(salt.as_slice());
    buf[53..85].copy_from_slice(init_code_hash.as_slice());
    let hash = keccak256(buf);
    Address::from_slice(&hash[12..])
}

/// Full address derivation for an EIP-8130 Create entry.
///
/// `deployer` is `ACCOUNT_CONFIG_ADDRESS` — the `AccountConfiguration` system
/// contract that the protocol treats as the CREATE2 factory.
pub fn derive_account_address(
    deployer: Address,
    user_salt: B256,
    bytecode: &Bytes,
    initial_owners: &[Owner],
) -> Address {
    let salt = effective_salt(user_salt, initial_owners);
    let code = deployment_code(bytecode);
    create2_address(deployer, salt, &code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deployment_header_is_14_bytes() {
        let h = deployment_header(256);
        assert_eq!(h.len(), DEPLOYMENT_HEADER_LEN);
        assert_eq!(h[0], 0x61); // PUSH2
        assert_eq!(h[1], 0x01); // 256 >> 8
        assert_eq!(h[2], 0x00); // 256 & 0xFF
    }

    #[test]
    fn effective_salt_order_independent() {
        let owner_a = Owner {
            verifier: Address::repeat_byte(1),
            owner_id: B256::repeat_byte(0x01),
            scope: 0,
        };
        let owner_b = Owner {
            verifier: Address::repeat_byte(2),
            owner_id: B256::repeat_byte(0x02),
            scope: 0,
        };
        let salt = B256::repeat_byte(0xAA);
        let s1 = effective_salt(salt, &[owner_a.clone(), owner_b.clone()]);
        let s2 = effective_salt(salt, &[owner_b, owner_a]);
        assert_eq!(s1, s2);
    }

    #[test]
    fn derive_account_address_deterministic() {
        let deployer = Address::repeat_byte(0xDD);
        let salt = B256::repeat_byte(0xAA);
        let bytecode = Bytes::from_static(&[0x60, 0x00, 0xf3]);
        let owners = vec![Owner {
            verifier: Address::repeat_byte(1),
            owner_id: B256::repeat_byte(0x01),
            scope: 0,
        }];
        let a1 = derive_account_address(deployer, salt, &bytecode, &owners);
        let a2 = derive_account_address(deployer, salt, &bytecode, &owners);
        assert_eq!(a1, a2);
        assert_ne!(a1, Address::ZERO);
    }

    #[test]
    fn different_owners_yield_different_addresses() {
        let deployer = Address::repeat_byte(0xDD);
        let salt = B256::repeat_byte(0xAA);
        let bytecode = Bytes::from_static(&[0x60, 0x00, 0xf3]);
        let owners_a = vec![Owner {
            verifier: Address::repeat_byte(1),
            owner_id: B256::repeat_byte(0x01),
            scope: 0,
        }];
        let owners_b = vec![Owner {
            verifier: Address::repeat_byte(2),
            owner_id: B256::repeat_byte(0x02),
            scope: 0,
        }];
        let a1 = derive_account_address(deployer, salt, &bytecode, &owners_a);
        let a2 = derive_account_address(deployer, salt, &bytecode, &owners_b);
        assert_ne!(a1, a2);
    }
}
