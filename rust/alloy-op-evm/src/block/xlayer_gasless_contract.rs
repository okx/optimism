//! On-chain gasless contract handle for OP block execution.
//!
//! [`GaslessContract`] performs an uncommitted system call to a configured gasless contract before
//! the user tx is executed. The contract exposes a single view that returns both whether the call
//! is allowed and the maximum gas the gasless allowance covers:
//!
//! ```solidity
//! function getGaslessAllowance(address to, bytes calldata dataPrefix)
//!     external view returns (bool allowed, uint64 gasLimit);
//! ```
//!
//! A tx is treated as gasless only when the contract returns `allowed == true` **and** the tx's
//! gas limit does not exceed the returned `gasLimit` (see [`GaslessContract::is_gasless`]).

use alloc::vec::Vec;
use alloy_consensus::Transaction;
use alloy_eips::eip4788::SYSTEM_ADDRESS;
use alloy_evm::{Evm, block::BlockExecutionError};
use alloy_primitives::{Address, Bytes, address};
use revm::context_interface::result::{ExecutionResult, Output};

/// XLayer mainnet (chain id 196) gasless whitelist predeploy address.
///
/// TODO: confirm the final mainnet address — this is a placeholder.
pub const XLAYER_MAINNET_GASLESS_CONTRACT: Address =
    address!("0x4200000000000000000000000000000000000901");

/// XLayer testnet (chain id 1952) gasless whitelist predeploy address.
///
/// TODO: confirm the final testnet address — this is a placeholder.
pub const XLAYER_TESTNET_GASLESS_CONTRACT: Address =
    address!("0x4200000000000000000000000000000000000901");

/// XLayer devnet (chain id 195) gasless whitelist predeploy address.
pub const XLAYER_DEVNET_GASLESS_CONTRACT: Address =
    address!("0x0000000000000000000000000000000000009999");

/// Returns the XLayer gasless whitelist predeploy address for the given chain id, or `None` for a
/// non-XLayer chain (gasless disabled).
///
/// The per-network mapping lives here (in `deps/optimism`) rather than in xlayer-chainspec so the
/// gasless contract can be derived from the chain spec at every [`OpEvmConfig`] construction (see
/// `OpEvmConfig::new`). This makes gasless detection **consensus-uniform across block building and
/// validation by construction** — there is no per-construction-site wiring to forget, and a node's
/// local config cannot change it.
///
/// The contract at the returned address must expose
/// `getGaslessAllowance(address,bytes) returns (bool,uint64)`.
///
/// [`OpEvmConfig`]: https://docs.rs/reth-optimism-evm
#[inline]
pub const fn xlayer_gasless_contract(chain_id: u64) -> Option<Address> {
    match chain_id {
        196 => Some(XLAYER_MAINNET_GASLESS_CONTRACT),
        1952 => Some(XLAYER_TESTNET_GASLESS_CONTRACT),
        195 => Some(XLAYER_DEVNET_GASLESS_CONTRACT),
        _ => None,
    }
}

/// 4-byte selector of `getGaslessAllowance(address,bytes)`.
///
/// `keccak256("getGaslessAllowance(address,bytes)")[..4] == 0xbad12ebe`
const GET_GASLESS_ALLOWANCE_SELECTOR: [u8; 4] = [0xba, 0xd1, 0x2e, 0xbe];

/// Handle to the on-chain gasless contract.
///
/// Holds the address of the gasless contract. The contract must expose:
///
/// ```solidity
/// function getGaslessAllowance(address to, bytes calldata dataPrefix)
///     external view returns (bool allowed, uint64 gasLimit);
/// ```
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GaslessContract {
    /// Address of the gasless contract on the chain.
    pub contract: Address,
}

impl GaslessContract {
    /// Creates a new [`GaslessContract`] pointed at `contract`.
    #[inline]
    pub const fn new(contract: Address) -> Self {
        Self { contract }
    }

    /// Returns the contract address
    #[inline]
    pub const fn contract(&self) -> Address {
        self.contract
    }

    /// Queries the gasless contract's `getGaslessAllowance(tx.to, tx.input)` view and returns the
    /// `(allowed, gas_limit)` pair it reports.
    ///
    /// `allowed` is the contract's whitelist decision; `gas_limit` is the maximum per-tx gas the
    /// gasless allowance covers. A create tx (no `to`) is never gasless, so it returns
    /// `(false, 0)` without making a call.
    pub fn get_gasless_allowance<E: Evm>(
        &self,
        evm: &mut E,
        tx: &impl Transaction,
    ) -> Result<(bool, u64), BlockExecutionError> {
        let Some(target) = tx.kind().into_to() else {
            return Ok((false, 0));
        };
        let result =
            transact(evm, self.contract, encode_get_gasless_allowance(target, tx.input()))?;
        Ok(decode_allowance(result))
    }

    /// Returns whether `tx` qualifies as gasless: the contract must allow it **and** the tx's gas
    /// limit must not exceed the contract-returned per-tx gas allowance. This pairs the whitelist
    /// decision with the gas-limit cap so a whitelisted tx that requests more gas than the
    /// allowance is not executed for free.
    pub fn is_gasless<E: Evm>(
        &self,
        evm: &mut E,
        tx: &impl Transaction,
    ) -> Result<bool, BlockExecutionError> {
        let (allowed, gas_limit) = self.get_gasless_allowance(evm, tx)?;
        Ok(allowed && tx.gas_limit() <= gas_limit)
    }
}

fn encode_get_gasless_allowance(target: Address, input: &Bytes) -> Bytes {
    let input_words = input.len().div_ceil(32);
    let input_offset = 4 + 32 * 3;
    let mut buf = Vec::with_capacity(input_offset + input_words * 32);
    buf.resize(input_offset + input_words * 32, 0);

    buf[..4].copy_from_slice(&GET_GASLESS_ALLOWANCE_SELECTOR);
    // ABI: left-pad address to 32 bytes.
    buf[16..36].copy_from_slice(target.as_slice());
    // ABI offset to dynamic bytes payload, measured from the start of the arguments block.
    buf[67] = 64;
    buf[92..100].copy_from_slice(&(input.len() as u64).to_be_bytes());
    buf[input_offset..input_offset + input.len()].copy_from_slice(input.as_ref());

    Bytes::from(buf)
}

/// Decodes the ABI-encoded `(bool allowed, uint64 gasLimit)` return of `getGaslessAllowance`.
///
/// The return is two 32-byte words: word 0 is the bool (`1` in its last byte), word 1 is the
/// uint64 (right-aligned). Any non-success result, a short buffer, or a non-canonical bool decodes
/// to `(false, 0)` — i.e. "not gasless".
fn decode_allowance<H>(result: ExecutionResult<H>) -> (bool, u64) {
    match result {
        ExecutionResult::Success { output: Output::Call(data), .. } if data.len() >= 64 => {
            let allowed = data[..31].iter().all(|b| *b == 0) && data[31] == 1;
            // uint64 occupies the low 8 bytes of the second word.
            let gas_limit = u64::from_be_bytes(data[56..64].try_into().expect("8 bytes"));
            (allowed, gas_limit)
        }
        _ => (false, 0),
    }
}

fn transact<E: Evm>(
    evm: &mut E,
    contract: Address,
    calldata: Bytes,
) -> Result<ExecutionResult<E::HaltReason>, BlockExecutionError> {
    let result = evm
        .transact_system_call(SYSTEM_ADDRESS, contract, calldata)
        .map_err(BlockExecutionError::other)?;

    Ok(result.result)
}

#[cfg(test)]
mod xlayer_test {
    use super::*;

    #[test]
    fn encodes_get_gasless_allowance_address_and_input() {
        let target = Address::from([0x11; 20]);
        let input = Bytes::copy_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        let encoded = encode_get_gasless_allowance(target, &input);

        assert_eq!(&encoded[..4], GET_GASLESS_ALLOWANCE_SELECTOR);
        assert!(encoded[4..16].iter().all(|b| *b == 0));
        assert_eq!(&encoded[16..36], target.as_slice());
        assert!(encoded[36..67].iter().all(|b| *b == 0));
        assert_eq!(encoded[67], 64);
        assert!(encoded[68..99].iter().all(|b| *b == 0));
        assert_eq!(encoded[99], input.len() as u8);
        assert_eq!(&encoded[100..104], input.as_ref());
        assert!(encoded[104..132].iter().all(|b| *b == 0));
        assert_eq!(encoded.len(), 132);
    }
}
