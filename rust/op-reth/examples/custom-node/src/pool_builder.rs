//! A header-generic, non-gasless transaction pool builder for the custom node.
//!
//! The optimism `OpPoolBuilder` hard-binds its `OpTransactionValidator` to
//! `alloy_consensus::Header` (via the XLayer gasless admission check, which builds
//! `OpEvmConfig::optimism(chain_spec)`). The custom node uses [`CustomHeader`], so it
//! cannot use `OpPoolBuilder` anymore. This builder instead wires reth's header-generic
//! [`EthTransactionValidator`] into a plain transaction pool.
//!
//! [`CustomHeader`]: crate::primitives::CustomHeader

use crate::{pool::CustomPooledTransaction, primitives::CustomTransaction};
use reth_node_api::{ConfigureEvm, FullNodeTypes, NodeTypes, TxTy};
use reth_node_builder::{
    BuilderContext,
    components::{PoolBuilder, TxPoolBuilder, create_blob_store},
};
use reth_op::{
    chainspec::EthereumHardforks,
    node::txpool::{OpPooledTransaction, OpPooledTx},
    pool::{
        EthTransactionPool, PoolTransaction, TransactionValidationTaskExecutor,
        blobstore::DiskFileBlobStore,
    },
};

/// The pool transaction type used by the custom node.
type CustomPoolTransaction = OpPooledTransaction<CustomTransaction, CustomPooledTransaction>;

/// A header-generic, non-gasless [`PoolBuilder`] for the custom node.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct CustomPoolBuilder;

impl<Node, Evm> PoolBuilder<Node, Evm> for CustomPoolBuilder
where
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec: EthereumHardforks>>,
    Evm: ConfigureEvm<Primitives = <Node::Types as NodeTypes>::Primitives> + Clone + 'static,
    CustomPoolTransaction: PoolTransaction<Consensus = TxTy<Node::Types>> + OpPooledTx,
{
    type Pool = EthTransactionPool<Node::Provider, DiskFileBlobStore, Evm, CustomPoolTransaction>;

    async fn build_pool(
        self,
        ctx: &BuilderContext<Node>,
        evm_config: Evm,
    ) -> eyre::Result<Self::Pool> {
        let blob_store = create_blob_store(ctx)?;

        let validator =
            TransactionValidationTaskExecutor::eth_builder(ctx.provider().clone(), evm_config)
                .no_eip4844()
                .with_max_tx_input_bytes(ctx.config().txpool.max_tx_input_bytes)
                .kzg_settings(ctx.kzg_settings()?)
                .set_tx_fee_cap(ctx.config().rpc.rpc_tx_fee_cap)
                .with_max_tx_gas_limit(ctx.config().txpool.max_tx_gas_limit)
                .with_minimum_priority_fee(ctx.config().txpool.minimum_priority_fee)
                .with_additional_tasks(ctx.config().txpool.additional_validation_tasks)
                .build_with_tasks::<CustomPoolTransaction, _>(
                    ctx.task_executor().clone(),
                    blob_store.clone(),
                );

        let pool_config = ctx.pool_config();
        let pool = TxPoolBuilder::new(ctx)
            .with_validator(validator)
            .build_and_spawn_maintenance_task(blob_store, pool_config)?;

        Ok(pool)
    }
}
