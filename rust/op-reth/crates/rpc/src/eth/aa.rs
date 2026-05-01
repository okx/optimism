//! Account Abstraction RPC extensions for the `eth` namespace.
//!
//! Extends standard `eth_` methods per the EIP-8130 spec:
//! - `eth_getTransactionCount`: optional `nonceKey` for 2D nonce channels

use alloy_eips::BlockId;
use alloy_primitives::{Address, U256};
use jsonrpsee::core::RpcResult;
use jsonrpsee::proc_macros::rpc;
use op_alloy_consensus::transaction::eip8130::{NONCE_MANAGER_ADDRESS, nonce_slot};
use reth_storage_api::StateProviderFactory;

/// Reads the 2D nonce for `(address, nonce_key)` from the Nonce Manager
/// precompile's storage at the requested block state.
pub fn read_2d_nonce<P: StateProviderFactory>(
    provider: &P,
    address: Address,
    block_id: BlockId,
    nonce_key: U256,
) -> RpcResult<U256> {
    let slot = nonce_slot(address, nonce_key);
    let state = provider.state_by_block_id(block_id).map_err(|e| {
        jsonrpsee::types::ErrorObjectOwned::owned(
            jsonrpsee::types::error::INTERNAL_ERROR_CODE,
            format!("state access error: {e}"),
            None::<()>,
        )
    })?;

    let value = state.storage(NONCE_MANAGER_ADDRESS, slot.into()).map_err(|e| {
        jsonrpsee::types::ErrorObjectOwned::owned(
            jsonrpsee::types::error::INTERNAL_ERROR_CODE,
            format!("storage read error: {e}"),
            None::<()>,
        )
    })?;

    Ok(U256::from(value.unwrap_or_default().to::<u64>()))
}

/// Overrides `eth_getTransactionCount` with an optional `nonceKey` parameter
/// for 2D nonce channel queries.
#[rpc(server, namespace = "eth")]
pub trait TransactionCountOverride {
    /// Returns the transaction count (nonce) for an address.
    ///
    /// When `nonce_key` is provided, reads the 2D nonce from the Nonce Manager
    /// precompile at [`NONCE_MANAGER_ADDRESS`]. When omitted, returns the
    /// standard account nonce.
    #[method(name = "getTransactionCount")]
    async fn get_transaction_count(
        &self,
        address: Address,
        block_number: Option<BlockId>,
        nonce_key: Option<U256>,
    ) -> RpcResult<U256>;
}

/// Implements [`TransactionCountOverrideServer`] by delegating standard nonce
/// queries to the state provider and reading the Nonce Manager for 2D nonces.
#[derive(Debug)]
pub struct TransactionCountOverrideImpl<Provider> {
    provider: Provider,
}

impl<Provider> TransactionCountOverrideImpl<Provider> {
    /// Creates a new override wrapping the given state provider.
    pub fn new(provider: Provider) -> Self {
        Self { provider }
    }
}

#[async_trait::async_trait]
impl<Provider> TransactionCountOverrideServer for TransactionCountOverrideImpl<Provider>
where
    Provider: StateProviderFactory + Send + Sync + 'static,
{
    async fn get_transaction_count(
        &self,
        address: Address,
        block_number: Option<BlockId>,
        nonce_key: Option<U256>,
    ) -> RpcResult<U256> {
        match nonce_key {
            Some(key) => {
                let block_id = block_number.unwrap_or_default();
                read_2d_nonce(&self.provider, address, block_id, key)
            }
            None => {
                let block_id = block_number.unwrap_or_default();
                let state = self.provider.state_by_block_id(block_id).map_err(|e| {
                    jsonrpsee::types::ErrorObjectOwned::owned(
                        jsonrpsee::types::error::INTERNAL_ERROR_CODE,
                        format!("state access error: {e}"),
                        None::<()>,
                    )
                })?;
                let nonce = state
                    .basic_account(&address)
                    .map_err(|e| {
                        jsonrpsee::types::ErrorObjectOwned::owned(
                            jsonrpsee::types::error::INTERNAL_ERROR_CODE,
                            format!("account read error: {e}"),
                            None::<()>,
                        )
                    })?
                    .map(|a| a.nonce)
                    .unwrap_or_default();
                Ok(U256::from(nonce))
            }
        }
    }
}
