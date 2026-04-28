//! Contains traits for working with payload attributes and their providers.

use core::fmt::Debug;

use crate::PipelineResult;
use alloc::boxed::Box;
use alloy_eips::BlockNumHash;
use async_trait::async_trait;
use kona_protocol::{L2BlockInfo, OpAttributesWithParent, SingleBatch};
use op_alloy_consensus::OpBlock;
use op_alloy_rpc_types_engine::OpPayloadAttributes;

/// [`AttributesProvider`] is a trait abstraction that generalizes the [`BatchQueue`] stage.
///
/// [`BatchQueue`]: crate::stages::BatchQueue
#[async_trait]
pub trait AttributesProvider {
    /// Returns the next valid batch upon the given safe head.
    async fn next_batch(&mut self, parent: L2BlockInfo) -> PipelineResult<SingleBatch>;

    /// Returns whether the current batch is the last in its span.
    fn is_last_in_span(&self) -> bool;
}

/// [`NextAttributes`] defines the interface for pulling attributes from
/// the top level `AttributesQueue` stage of the pipeline.
#[async_trait]
pub trait NextAttributes {
    /// Returns the next [`OpAttributesWithParent`] from the current batch.
    async fn next_attributes(
        &mut self,
        parent: L2BlockInfo,
    ) -> PipelineResult<OpAttributesWithParent>;
}

/// The [`AttributesBuilder`] is responsible for preparing [`OpPayloadAttributes`]
/// that can be used to construct an L2 Block containing only deposits.
#[async_trait]
pub trait AttributesBuilder: Debug + Send {
    /// Prepares a template [`OpPayloadAttributes`] that is ready to be used to build an L2
    /// block. The block will contain deposits only, on top of the given L2 parent, with the L1
    /// origin set to the given epoch.
    /// By default, the [`OpPayloadAttributes`] template will have `no_tx_pool` set to true,
    /// and no sequencer transactions. The caller has to modify the template to add transactions.
    /// This can be done by either setting the `no_tx_pool` to false as sequencer, or by appending
    /// batch transactions as the verifier.
    async fn prepare_payload_attributes(
        &mut self,
        l2_parent: L2BlockInfo,
        epoch: BlockNumHash,
    ) -> PipelineResult<OpPayloadAttributes>;

    /// Best-effort prefetch of L1 data needed for `prepare_payload_attributes` for the given
    /// `epoch`. Implementations that internally cache L1 lookups can use this to warm the
    /// cache concurrently with other CL work (for example, sealing the previous block), so
    /// the upcoming `prepare_payload_attributes` call hits cache instead of issuing fresh
    /// L1 RPCs.
    ///
    /// `is_new_epoch` should be `true` when the upcoming block starts a new sequencing
    /// epoch (`epoch.number != parent.l1_origin.number`), in which case L1 receipts are
    /// also needed; otherwise only the L1 header is fetched.
    ///
    /// Errors are intentionally swallowed: a failed prefetch is harmless because the
    /// authoritative call in `prepare_payload_attributes` will re-fetch and surface the
    /// error.
    #[allow(unused_variables)]
    async fn prefetch_for_epoch(&mut self, epoch: BlockNumHash, is_new_epoch: bool) {}

    /// Best-effort cache-priming for the just-sealed L2 block. Implementations that cache
    /// `SystemConfig` (or the underlying L2 block) by number can populate it from the
    /// sealed payload, so the next `prepare_payload_attributes` call avoids the
    /// `eth_getBlockByNumber` RPC against the L2 EL.
    ///
    /// Errors are intentionally swallowed; a failed prime simply means the next
    /// `prepare_payload_attributes` will pay an RPC.
    #[allow(unused_variables)]
    async fn cache_sealed_block(&mut self, block: OpBlock) {}
}
