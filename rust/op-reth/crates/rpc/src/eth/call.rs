use crate::{OpEthApi, OpEthApiError, eth::RpcNodeCore};
use op_revm::transaction::OpTxTr;
use reth_evm::{ConfigureEvm, Evm, EvmEnvFor, HaltReasonFor, TxEnvFor};
use reth_optimism_evm::OpTxEnv;
use reth_revm::db::bal::EvmDatabaseError;
use reth_rpc_eth_api::{
    FromEvmError, RpcConvert,
    helpers::{Call, EthCall, estimate::EstimateCall},
};
use reth_storage_api::errors::ProviderError;
use revm::{Database, context_interface::result::ResultAndState};

impl<N, Rpc> EthCall for OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    OpEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = OpEthApiError, Evm = N::Evm>,
    TxEnvFor<N::Evm>: OpTxTr + OpTxEnv,
{
}

impl<N, Rpc> EstimateCall for OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    OpEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = OpEthApiError, Evm = N::Evm>,
    TxEnvFor<N::Evm>: OpTxTr + OpTxEnv,
{
}

impl<N, Rpc> Call for OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    OpEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = OpEthApiError, Evm = N::Evm>,
    TxEnvFor<N::Evm>: OpTxTr + OpTxEnv,
{
    #[inline]
    fn call_gas_limit(&self) -> u64 {
        self.inner.eth_api.gas_cap()
    }

    #[inline]
    fn max_simulate_blocks(&self) -> u64 {
        self.inner.eth_api.max_simulate_blocks()
    }

    #[inline]
    fn evm_memory_limit(&self) -> u64 {
        self.inner.eth_api.evm_memory_limit()
    }

    /// Gasless-aware override of the default [`Call::transact`].
    ///
    /// Builds a fresh EVM per call, so (like `inspect`) it detects gasless on this same plain
    /// EVM and, when gasless, marks the `tx_env` gasless before building the EVM. The flagged tx
    /// then runs through `OpEvm::transact_raw`, which zeroes the base fee for it. There is no
    /// inspector here, so detection cannot pollute a trace. The non-gasless path is byte-for-byte
    /// the default.
    fn transact<DB>(
        &self,
        mut db: DB,
        evm_env: EvmEnvFor<Self::Evm>,
        mut tx_env: TxEnvFor<Self::Evm>,
    ) -> Result<ResultAndState<HaltReasonFor<Self::Evm>>, Self::Error>
    where
        DB: Database<Error = EvmDatabaseError<ProviderError>> + core::fmt::Debug,
    {
        // `&mut DB: Database`, so detection borrows the db and leaves it for the real execution.
        if self.detect_gasless(&mut db, evm_env.clone(), &tx_env)? {
            tx_env.set_gasless(true);
        }

        let mut evm = self.evm_config().evm_with_env(db, evm_env);
        evm.transact(tx_env).map_err(Self::Error::from_evm_err)
    }
}
