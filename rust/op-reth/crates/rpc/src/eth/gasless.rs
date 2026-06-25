//! Gasless-aware re-execution support for the OP `eth_`/`debug_` RPC paths.
//!
//! Block execution waives the base-fee check for zero-priced "gasless" transactions:
//! `OpEvm::transact_raw` zeroes the base fee for any tx flagged `is_gasless`. The default RPC
//! re-execution helpers (`Trace::inspect`, `Call::transact`) build the `tx_env` with
//! `is_gasless == false`, so re-running a gasless tx — e.g. via `debug_traceTransaction` — fails
//! with `max fee per gas less than block base fee`.
//!
//! This module mirrors the *exact* gasless detection the executor and the txpool validator use:
//!
//! 1. the tx is not a deposit and is zero-priced (`max_fee_per_gas() == 0`), and
//! 2. the on-chain gasless contract (`getGaslessAllowance(to, input)`) approves it and its gas
//!    limit is within the contract-reported allowance (`GaslessContract::is_gasless`).
//!
//! The gasless contract is derived from the chain id (`xlayer_gasless_contract`) — the same mapping
//! `OpEvmConfig::new` uses — so RPC re-execution stays consensus-uniform with block execution by
//! construction. Detection runs an *uncommitted* system call (`transact_system_call`), so the
//! database is left untouched and can be reused for the actual (possibly inspected) execution.

use crate::{OpEthApi, OpEthApiError};
use alloy_consensus::transaction::Transaction as ConsensusTransaction;
use alloy_eips::{eip2718::Typed2718, eip2930::AccessList, eip7702::SignedAuthorization};
use alloy_primitives::{Bytes, ChainId, TxKind, B256, U256};
use op_revm::transaction::OpTxTr;
use reth_chainspec::{ChainSpecProvider, EthChainSpec};
use reth_evm::{ConfigureEvm, Database, EvmEnvFor, TxEnvFor};
use reth_optimism_evm::{xlayer_gasless_contract, GaslessContract, OpTxEnv};
use reth_rpc_eth_api::{RpcConvert, RpcNodeCore};
use reth_rpc_eth_types::EthApiError;
use revm::context_interface::Transaction as RevmTransaction;

/// Owned read-only [`alloy_consensus::Transaction`] view of a revm `tx_env`.
///
/// [`GaslessContract::is_gasless`] is generic over `alloy_consensus::Transaction`, but the fresh
/// per-call RPC helpers (`inspect`/`transact`) only have the revm `tx_env` — not the consensus tx
/// the block executor passes. The two `Transaction` traits are distinct, and the consensus trait
/// requires `'static`, so this owns the only fields the gasless allowance query reads (`kind`/`to`,
/// `input`, `gas_limit`); the remaining methods return unused defaults. Built only on the
/// (zero-priced) detection path, so the single `input` clone is off the hot path.
#[derive(Debug)]
pub(crate) struct GaslessTxView {
    kind: TxKind,
    input: Bytes,
    gas_limit: u64,
}

impl GaslessTxView {
    fn from_tx_env(tx_env: &impl RevmTransaction) -> Self {
        Self { kind: tx_env.kind(), input: tx_env.input().clone(), gas_limit: tx_env.gas_limit() }
    }
}

impl Typed2718 for GaslessTxView {
    fn ty(&self) -> u8 {
        0
    }
}

impl ConsensusTransaction for GaslessTxView {
    fn chain_id(&self) -> Option<ChainId> {
        None
    }

    fn nonce(&self) -> u64 {
        0
    }

    // Load-bearing: gas limit is checked against the contract-reported allowance.
    fn gas_limit(&self) -> u64 {
        self.gas_limit
    }

    fn gas_price(&self) -> Option<u128> {
        Some(0)
    }

    fn max_fee_per_gas(&self) -> u128 {
        0
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        None
    }

    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        None
    }

    fn priority_fee_or_price(&self) -> u128 {
        0
    }

    fn effective_gas_price(&self, _base_fee: Option<u64>) -> u128 {
        0
    }

    fn is_dynamic_fee(&self) -> bool {
        false
    }

    // Load-bearing: `to` drives the gasless allowance lookup (create txs are never gasless).
    fn kind(&self) -> TxKind {
        self.kind
    }

    fn is_create(&self) -> bool {
        self.kind.is_create()
    }

    fn value(&self) -> U256 {
        U256::ZERO
    }

    // Load-bearing: the calldata prefix is hashed into the gasless allowance query.
    fn input(&self) -> &Bytes {
        &self.input
    }

    fn access_list(&self) -> Option<&AccessList> {
        None
    }

    fn blob_versioned_hashes(&self) -> Option<&[B256]> {
        None
    }

    fn authorization_list(&self) -> Option<&[SignedAuthorization]> {
        None
    }
}

impl<N, Rpc> OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = OpEthApiError, Evm = N::Evm>,
    TxEnvFor<N::Evm>: OpTxTr + OpTxEnv,
{
    /// Returns the gasless whitelist contract for the active chain, or `None` when the chain has no
    /// gasless contract. Derived from the chain id, matching `OpEvmConfig`/the executor.
    pub(crate) fn gasless_contract(&self) -> Option<GaslessContract> {
        xlayer_gasless_contract(self.provider().chain_spec().chain().id()).map(GaslessContract::new)
    }

    /// Runs the same gasless detection block execution uses, over `db`, without committing.
    ///
    /// Returns `Ok(false)` immediately for create txs, deposits, non-zero-priced txs, or chains
    /// with no gasless contract. Otherwise it issues an uncommitted system call to the gasless
    /// contract; because the call does not persist state, `db` is safe to reuse afterwards for the
    /// real (and possibly inspected) execution.
    pub(crate) fn detect_gasless<DB>(
        &self,
        db: DB,
        evm_env: EvmEnvFor<N::Evm>,
        tx_env: &TxEnvFor<N::Evm>,
    ) -> Result<bool, OpEthApiError>
    where
        DB: Database,
    {
        // Cheap, db-free pre-checks first — mirrors the executor: deposits are never gasless and
        // only zero-priced txs qualify.
        if tx_env.is_deposit() || RevmTransaction::max_fee_per_gas(tx_env) != 0 {
            return Ok(false);
        }
        let Some(contract) = self.gasless_contract() else {
            return Ok(false);
        };

        // Detection must not pollute a trace: run the whitelist system call on a plain
        // (non-inspector) EVM. The call is uncommitted, so the caller can reuse the same db.
        let view = GaslessTxView::from_tx_env(tx_env);
        let mut evm = self.evm_config().evm_with_env(db, evm_env);
        contract
            .is_gasless(&mut evm, &view)
            .map_err(|err| OpEthApiError::Eth(EthApiError::EvmCustom(err.to_string())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, bytes};
    use revm::context::TxEnv;

    /// The view stands in for the zero-priced gasless tx during the allowance query, which reads
    /// only `to`/`kind`, `input`, and `gas_limit`. Those must round-trip from the source `tx_env`,
    /// otherwise RPC detection diverges from block execution and gasless txs get misclassified.
    #[test]
    fn gasless_tx_view_preserves_load_bearing_fields() {
        let to = address!("00000000000000000000000000000000000000ab");
        let input = bytes!("deadbeef");
        let tx = TxEnv::builder()
            .kind(TxKind::Call(to))
            .gas_limit(21_000)
            .data(input.clone())
            .build()
            .unwrap();

        let view = GaslessTxView::from_tx_env(&tx);

        assert_eq!(view.kind(), TxKind::Call(to));
        assert_eq!(view.input(), &input);
        assert_eq!(view.gas_limit(), 21_000);
        assert!(!view.is_create());
    }

    /// Detection only builds the view for a zero-priced tx, so every fee accessor must report the
    /// zero-priced, non-dynamic shape. A non-zero value here would make the allowance lookup hash
    /// or fee-compare against the wrong inputs.
    #[test]
    fn gasless_tx_view_reports_zero_priced_shape() {
        let tx = TxEnv::builder()
            .kind(TxKind::Call(address!("00000000000000000000000000000000000000cd")))
            .build()
            .unwrap();
        let view = GaslessTxView::from_tx_env(&tx);

        assert_eq!(ConsensusTransaction::max_fee_per_gas(&view), 0);
        assert_eq!(view.gas_price(), Some(0));
        assert_eq!(view.max_priority_fee_per_gas(), None);
        assert_eq!(view.priority_fee_or_price(), 0);
        assert_eq!(view.effective_gas_price(Some(7)), 0);
        assert_eq!(view.value(), U256::ZERO);
        assert!(!view.is_dynamic_fee());
        assert_eq!(Typed2718::ty(&view), 0);
        assert_eq!(view.chain_id(), None);
        assert_eq!(view.nonce(), 0);
    }

    /// A create tx is never gasless; the view must surface `Create` so the `to`-keyed allowance
    /// lookup rejects it rather than hashing against a zero address.
    #[test]
    fn gasless_tx_view_surfaces_create_kind() {
        let tx = TxEnv::builder().kind(TxKind::Create).build().unwrap();
        let view = GaslessTxView::from_tx_env(&tx);

        assert_eq!(view.kind(), TxKind::Create);
        assert!(view.is_create());
    }
}
