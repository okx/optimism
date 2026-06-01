//! On-chain gasless contract handle for OP block execution.
//!
//! [`GaslessContract`] performs uncommitted system calls to a configured gasless contract before
//! the user tx is executed. A tx is treated as gasless only when `isGaslessEnabled()` and
//! `isWhitelisted(tx.to, tx.input)` both return true.

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
/// The contract at the returned address must expose `isGaslessEnabled()` and
/// `isWhitelisted(address,bytes)`.
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

/// 4-byte selector of `isGaslessEnabled()`.
///
/// `keccak256("isGaslessEnabled()")[..4] == 0x963a959e`
const IS_GASLESS_ENABLED_SELECTOR: [u8; 4] = [0x96, 0x3a, 0x95, 0x9e];

/// 4-byte selector of `isWhitelisted(address,bytes)`.
///
/// `keccak256("isWhitelisted(address,bytes)")[..4] == 0x1f0a8fa7`
const IS_WHITELISTED_SELECTOR: [u8; 4] = [0x1f, 0x0a, 0x8f, 0xa7];

/// Handle to the on-chain gasless contract.
///
/// Holds the address of the gasless contract. The contract must expose:
///
/// ```solidity
/// function isGaslessEnabled() external view returns (bool);
/// function isWhitelisted(address target, bytes calldata input) external view returns (bool);
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

    ///
    pub fn is_gasless<E: Evm>(
        &self,
        evm: &mut E,
        tx: &impl Transaction,
    ) -> Result<bool, BlockExecutionError> {
        let contract = self.contract;
        if !call_bool(evm, contract, encode_selector(IS_GASLESS_ENABLED_SELECTOR))? {
            return Ok(false);
        }
        if let (Some(target), input) = (tx.kind().into_to(), tx.input()) {
            return call_bool(evm, contract, encode_is_whitelisted(target, input));
        }
        Ok(false)
    }
}

fn encode_selector(selector: [u8; 4]) -> Bytes {
    Bytes::copy_from_slice(&selector)
}

fn encode_is_whitelisted(target: Address, input: &Bytes) -> Bytes {
    let input_words = input.len().div_ceil(32);
    let input_offset = 4 + 32 * 3;
    let mut buf = Vec::with_capacity(input_offset + input_words * 32);
    buf.resize(input_offset + input_words * 32, 0);

    buf[..4].copy_from_slice(&IS_WHITELISTED_SELECTOR);
    // ABI: left-pad address to 32 bytes.
    buf[16..36].copy_from_slice(target.as_slice());
    // ABI offset to dynamic bytes payload, measured from the start of the arguments block.
    buf[67] = 64;
    buf[92..100].copy_from_slice(&(input.len() as u64).to_be_bytes());
    buf[input_offset..input_offset + input.len()].copy_from_slice(input.as_ref());

    Bytes::from(buf)
}

fn decode_bool<H>(result: ExecutionResult<H>) -> bool {
    match result {
        ExecutionResult::Success { output: Output::Call(data), .. } => {
            data.len() == 32 && data[..31].iter().all(|b| *b == 0) && data[31] == 1
        }
        _ => false,
    }
}

fn call_bool<E: Evm>(
    evm: &mut E,
    contract: Address,
    calldata: Bytes,
) -> Result<bool, BlockExecutionError> {
    let result = evm
        .transact_system_call(SYSTEM_ADDRESS, contract, calldata)
        .map_err(BlockExecutionError::other)?;

    Ok(decode_bool(result.result))
}

#[cfg(test)]
mod xlayer_test {
    use super::*;

    #[test]
    fn encodes_is_whitelisted_address_and_input() {
        let target = Address::from([0x11; 20]);
        let input = Bytes::copy_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        let encoded = encode_is_whitelisted(target, &input);

        assert_eq!(&encoded[..4], IS_WHITELISTED_SELECTOR);
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
