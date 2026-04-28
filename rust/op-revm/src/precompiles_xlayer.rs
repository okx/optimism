//! XLayer (EIP-8130) AA system precompiles: NonceManager + TxContext.
use crate::{OpSpecId, transaction::OpTxTr};
use alloy_sol_types::{SolCall, SolValue, sol};
use revm::{
    Database,
    context_interface::{ContextTr, Transaction},
    primitives::{Address, B256, Bytes, U256, address, keccak256},
};
use std::{string::String, vec::Vec};

/// EIP-8130 transaction type byte.
pub(crate) const EIP8130_TX_TYPE: u8 = 0x7B;

/// NonceManager system precompile address.
pub const NONCE_MANAGER_ADDRESS: Address = address!("0x000000000000000000000000000000000000aa02");

/// TxContext system precompile address.
pub const TX_CONTEXT_ADDRESS: Address = address!("0x000000000000000000000000000000000000aa03");

/// Base storage slot for the NonceManager nonce mapping.
pub const NONCE_BASE_SLOT: U256 = U256::from_limbs([1u64, 0, 0, 0]);

/// Gas cost for TxContext precompile calls.
pub const TX_CONTEXT_GAS: u64 = 100;

/// Gas cost for NonceManager precompile calls.
pub const NONCE_MANAGER_GAS: u64 = 2_100;

/// Stack buffer size for AA precompile input. Sized to fit the largest valid
/// call layout (`INonceManager.getNonce`: 4 + 32 + 32 = 68 bytes) with margin.
pub(crate) const AA_PRECOMPILE_INPUT_BUF: usize = 96;

sol! {
    /// Solidity tuple representation of an [`Eip8130Call`] for ABI encoding.
    struct CallTuple {
        address target;
        bytes data;
    }

    /// NonceManager view interface.
    interface INonceManager {
        function getNonce(address account, uint256 nonceKey) external view returns (uint256);
    }

    /// TxContext view interface.
    interface ITxContext {
        function getSender() external view returns (address);
        function getPayer() external view returns (address);
        function getOwnerId() external view returns (bytes32);
        function getMaxCost() external view returns (uint256);
        function getGasLimit() external view returns (uint256);
        function getCalls() external view returns (CallTuple[][] memory);
    }
}

/// Computes the NonceManager storage slot for `nonce[account][nonce_key]`.
///
/// Mirrors the Solidity mapping layout: `keccak256(nonce_key . keccak256(account . NONCE_BASE_SLOT))`.
pub fn aa_nonce_slot(account: Address, nonce_key: U256) -> U256 {
    let inner = keccak256((account, NONCE_BASE_SLOT).abi_encode());
    U256::from_be_bytes(keccak256((nonce_key, inner).abi_encode()).0)
}

/// Computes the AA execution-only gas limit (`tx.gas_limit - parts.aa_intrinsic_gas`).
fn aa_execution_gas_limit(tx: impl OpTxTr) -> u64 {
    let parts = tx.eip8130_parts();
    tx.gas_limit().saturating_sub(parts.aa_intrinsic_gas)
}

/// Computes the AA `max_cost` available to verifier contracts via `getMaxCost()`.
///
/// `max_cost = (gas_limit - parts.aa_intrinsic_gas + parts.aa_intrinsic_gas
///              - parts.payer_intrinsic_gas + parts.custom_verifier_gas_cap)
///             * tx.max_fee_per_gas`
///
/// Equivalent to `(gas_limit - payer_intrinsic_gas + custom_verifier_gas_cap) * max_fee_per_gas`.
fn aa_max_cost(tx: impl OpTxTr) -> U256 {
    let parts = tx.eip8130_parts();
    let total_gas = U256::from(tx.gas_limit())
        .saturating_sub(U256::from(parts.payer_intrinsic_gas))
        .saturating_add(U256::from(parts.custom_verifier_gas_cap));
    total_gas.saturating_mul(U256::from(tx.max_fee_per_gas()))
}

/// Returns whether EIP-8130 system precompiles are available at the given spec.
pub(crate) fn eip8130_precompiles_enabled(spec: OpSpecId) -> bool {
    spec.is_enabled_in(OpSpecId::XLAYER_AA)
}

pub(crate) fn run_nonce_manager_precompile<CTX>(
    context: &mut CTX,
    input: &[u8],
) -> Result<(u64, Bytes), String>
where
    CTX: ContextTr<Cfg: revm::context::Cfg<Spec = OpSpecId>, Tx: OpTxTr>,
{
    if input.len() < 4 || input[0..4] != INonceManager::getNonceCall::SELECTOR {
        return Err("unknown nonce manager selector".into());
    }
    let call = INonceManager::getNonceCall::abi_decode(input)
        .map_err(|_| String::from("invalid nonce manager input"))?;

    let slot = aa_nonce_slot(call.account, call.nonceKey);

    let storage_value = context
        .db_mut()
        .storage(NONCE_MANAGER_ADDRESS, slot.into())
        .map_err(|_| String::from("nonce manager storage read failed"))?;

    let mut out = [0u8; 32];
    let storage_bytes = storage_value.to_be_bytes::<32>();
    out[24..32].copy_from_slice(&storage_bytes[24..32]);

    Ok((NONCE_MANAGER_GAS, Bytes::copy_from_slice(&out)))
}

pub(crate) fn run_tx_context_precompile<CTX>(
    context: &CTX,
    input: &[u8],
) -> Result<(u64, Bytes), String>
where
    CTX: ContextTr<Cfg: revm::context::Cfg<Spec = OpSpecId>, Tx: OpTxTr>,
{
    if input.len() < 4 {
        return Err("invalid tx context input".into());
    }

    let tx = context.tx();
    let parts = if tx.tx_type() == EIP8130_TX_TYPE { Some(tx.eip8130_parts()) } else { None };

    let selector: [u8; 4] = input[0..4].try_into().expect("checked length above");
    let output = match selector {
        ITxContext::getSenderCall::SELECTOR => {
            let sender = parts.map_or(Address::ZERO, |p| p.sender);
            ITxContext::getSenderCall::abi_encode_returns(&sender)
        }
        ITxContext::getPayerCall::SELECTOR => {
            let payer = parts.map_or(Address::ZERO, |p| p.payer);
            ITxContext::getPayerCall::abi_encode_returns(&payer)
        }
        ITxContext::getOwnerIdCall::SELECTOR => {
            let owner_id = parts.map_or(B256::ZERO, |p| p.owner_id);
            ITxContext::getOwnerIdCall::abi_encode_returns(&owner_id)
        }
        ITxContext::getMaxCostCall::SELECTOR => {
            let max_cost = if parts.is_some() { aa_max_cost(context.tx()) } else { U256::ZERO };
            ITxContext::getMaxCostCall::abi_encode_returns(&max_cost)
        }
        ITxContext::getGasLimitCall::SELECTOR => {
            let gas_limit = if parts.is_some() {
                U256::from(aa_execution_gas_limit(context.tx()))
            } else {
                U256::ZERO
            };
            ITxContext::getGasLimitCall::abi_encode_returns(&gas_limit)
        }
        ITxContext::getCallsCall::SELECTOR => {
            let phases: Vec<Vec<CallTuple>> = parts
                .map(|p| {
                    p.call_phases
                        .iter()
                        .map(|phase| {
                            phase
                                .iter()
                                .map(|c| CallTuple { target: c.to, data: c.data.clone() })
                                .collect()
                        })
                        .collect()
                })
                .unwrap_or_default();
            ITxContext::getCallsCall::abi_encode_returns(&phases)
        }
        _ => return Err("unknown tx context selector".into()),
    };

    Ok((TX_CONTEXT_GAS, Bytes::from(output)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aa_nonce_slot_matches_solidity_layout() {
        let account = Address::repeat_byte(0xAB);
        let key = U256::from(7u64);
        let inner = keccak256((account, NONCE_BASE_SLOT).abi_encode());
        let expected = U256::from_be_bytes(keccak256((key, inner).abi_encode()).0);
        assert_eq!(aa_nonce_slot(account, key), expected);
    }
}
