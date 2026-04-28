//! The [`SequencerActor`].

use crate::{
    CancellableContext, NodeActor, SequencerAdminQuery, UnsafePayloadGossipClient,
    actors::{
        SequencerEngineClient,
        engine::EngineClientError,
        sequencer::{
            conductor::Conductor,
            error::SequencerActorError,
            metrics::{
                update_attributes_build_duration_metrics, update_block_build_duration_metrics,
                update_conductor_commitment_duration_metrics, update_seal_duration_metrics,
                update_total_transactions_sequenced,
            },
            origin_selector::OriginSelector,
        },
    },
};
use alloy_eips::{BlockNumHash, eip7685::EMPTY_REQUESTS_HASH};
use alloy_primitives::B256;
use alloy_rpc_types_engine::{CancunPayloadFields, PayloadId, PraguePayloadFields};
use async_trait::async_trait;
use kona_derive::{AttributesBuilder, PipelineErrorKind};
use kona_engine::{InsertTaskError, SealTaskError, SynchronizeTaskError};
use kona_genesis::RollupConfig;
use kona_protocol::{BlockInfo, L2BlockInfo, OpAttributesWithParent};
use op_alloy_consensus::{OpBlock, OpTxEnvelope};
use op_alloy_rpc_types_engine::{
    OpExecutionPayload, OpExecutionPayloadEnvelope, OpExecutionPayloadSidecar, OpPayloadAttributes,
};
use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{select, sync::mpsc};
use tokio_util::sync::{CancellationToken, WaitForCancellationFuture};

/// Default margin reserved before the target block timestamp for the seal operation
/// (`engine_getPayload` + import). Mirrors `op-node`'s `defaultSealingDuration` so that
/// the EL gets the full `block_time - SEALING_DURATION` window to fill transactions
/// instead of having sealing eat into the next block's build window.
const DEFAULT_SEALING_DURATION: Duration = Duration::from_millis(50);

/// Reconstruct a best-effort [`L2BlockInfo`] for the in-flight (started-but-not-yet-sealed)
/// block from the open [`UnsealedPayloadHandle`]. The block hash is unknown until
/// `engine_getPayload` returns and is therefore left as the default value; every other
/// field — `number`, `parent_hash`, `timestamp`, `l1_origin`, `seq_num` — is fully
/// determined by the parent block, the L1 origin used at build time, and the payload
/// attributes.
///
/// Used **only** to drive [`OriginSelector::next_l1_origin`] and
/// [`AttributesBuilder::prefetch_for_epoch`] in parallel with the seal RPCs. Callers must
/// NOT pass this into `prepare_payload_attributes` directly: the authoritative call needs
/// the actual sealed parent (with hash) so the L2 EL can resolve `system_config_by_number`
/// against canonical chain state.
fn predict_unsealed_block_info(handle: &UnsealedPayloadHandle) -> L2BlockInfo {
    let parent = handle.attributes_with_parent.parent();
    let attrs = handle.attributes_with_parent.attributes();
    let l1_origin_id =
        BlockNumHash { number: handle.l1_origin_used.number, hash: handle.l1_origin_used.hash };
    let seq_num = if l1_origin_id.number == parent.l1_origin.number &&
        l1_origin_id.hash == parent.l1_origin.hash
    {
        parent.seq_num.saturating_add(1)
    } else {
        0
    };
    L2BlockInfo {
        block_info: BlockInfo {
            // Hash unknown until seal completes. None of the consumers we use this for
            // (`OriginSelector::next_l1_origin`, `AttributesBuilder::prefetch_for_epoch`)
            // read this field.
            hash: B256::default(),
            number: parent.block_info.number.saturating_add(1),
            parent_hash: parent.block_info.hash,
            timestamp: attrs.payload_attributes.timestamp,
        },
        l1_origin: l1_origin_id,
        seq_num,
    }
}

/// The handle to a block that has been started but not sealed.
#[derive(Debug)]
pub(super) struct UnsealedPayloadHandle {
    /// The [`PayloadId`] of the unsealed payload.
    pub payload_id: PayloadId,
    /// The [`OpAttributesWithParent`] used to start block building.
    pub attributes_with_parent: OpAttributesWithParent,
    /// L1 origin block selected when this build job was started. Stored so the sequencer
    /// can ask the [`OriginSelector`] "what is the L1 origin for the block AFTER this
    /// one?" while sealing this block, without waiting for seal to finish (the answer only
    /// depends on this block's L1 origin and timestamp, both known here).
    pub l1_origin_used: BlockInfo,
}

/// The return payload of the `seal_last_and_start_next` function. This allows the sequencer
/// to make an informed decision about when to seal and build the next block.
#[derive(Debug)]
struct SealLastStartNextResult {
    /// The [`UnsealedPayloadHandle`] that was built.
    pub unsealed_payload_handle: Option<UnsealedPayloadHandle>,
    /// How long it took to execute the seal operation.
    pub seal_duration: Duration,
}

/// The [`SequencerActor`] is responsible for building L2 blocks on top of the current unsafe head
/// and scheduling them to be signed and gossipped by the P2P layer, extending the L2 chain with new
/// blocks.
#[derive(Debug)]
pub struct SequencerActor<
    AttributesBuilder_,
    Conductor_,
    OriginSelector_,
    SequencerEngineClient_,
    UnsafePayloadGossipClient_,
> where
    AttributesBuilder_: AttributesBuilder,
    Conductor_: Conductor,
    OriginSelector_: OriginSelector,
    SequencerEngineClient_: SequencerEngineClient,
    UnsafePayloadGossipClient_: UnsafePayloadGossipClient,
{
    /// Receiver for admin API requests.
    pub admin_api_rx: mpsc::Receiver<SequencerAdminQuery>,
    /// The attributes builder used for block building.
    pub attributes_builder: AttributesBuilder_,
    /// The cancellation token, shared between all tasks.
    pub cancellation_token: CancellationToken,
    /// The optional conductor RPC client.
    pub conductor: Option<Conductor_>,
    /// The struct used to interact with the engine.
    pub engine_client: SequencerEngineClient_,
    /// Whether the sequencer is active.
    pub is_active: bool,
    /// Whether the sequencer is in recovery mode.
    pub in_recovery_mode: bool,
    /// The struct used to determine the next L1 origin.
    pub origin_selector: OriginSelector_,
    /// The rollup configuration.
    pub rollup_config: Arc<RollupConfig>,
    /// A client to asynchronously sign and gossip built payloads to the network actor.
    pub unsafe_payload_gossip_client: UnsafePayloadGossipClient_,
}

impl<
    AttributesBuilder_,
    Conductor_,
    OriginSelector_,
    SequencerEngineClient_,
    UnsafePayloadGossipClient_,
>
    SequencerActor<
        AttributesBuilder_,
        Conductor_,
        OriginSelector_,
        SequencerEngineClient_,
        UnsafePayloadGossipClient_,
    >
where
    AttributesBuilder_: AttributesBuilder,
    Conductor_: Conductor,
    OriginSelector_: OriginSelector,
    SequencerEngineClient_: SequencerEngineClient,
    UnsafePayloadGossipClient_: UnsafePayloadGossipClient,
{
    /// Seals and commits the last pending block, if one exists and starts the build job for the
    /// next L2 block, on top of the current unsafe head.
    ///
    /// If a new block was started, it will return the associated [`UnsealedPayloadHandle`] so
    /// that it may be sealed and committed in a future call to this function.
    async fn seal_last_and_start_next(
        &mut self,
        payload_to_seal: Option<&UnsealedPayloadHandle>,
    ) -> Result<SealLastStartNextResult, SequencerActorError> {
        // First-iter / recovery path: nothing to seal, just kick off a build. Read the
        // parent from the watch (it cannot lag here, since no in-flight seal exists).
        let Some(to_seal) = payload_to_seal else {
            let unsealed_payload_handle = self.build_unsealed_payload(None).await?;
            return Ok(SealLastStartNextResult {
                unsealed_payload_handle,
                seal_duration: Duration::ZERO,
            });
        };

        // Steady-state path:
        //
        // 1. Seal + import the previous block (engine_client side).
        // 2. **In parallel** with (1), warm caches that the upcoming
        //    `prepare_payload_attributes` will read:
        //    - Pick the next L1 origin via `origin_selector.next_l1_origin` using the
        //      *predicted* L2BlockInfo of the in-flight build (origin selection only
        //      depends on `l1_origin` + `timestamp`, never the unknown block hash).
        //    - Ask `attributes_builder.prefetch_for_epoch` to populate the L1
        //      `header_by_hash` / `receipts_by_hash` caches for that epoch.
        // 3. After both halves complete, prime the L2 SystemConfig cache from the
        //    just-sealed payload so `system_config_by_number(parent.number)` skips its
        //    `eth_getBlockByNumber` RPC.
        // 4. Then call `build_unsealed_payload` as usual. With caches warmed, the L1/L2
        //    fetches inside `prepare_payload_attributes` are mostly cache hits, shrinking
        //    per-cycle CL processing and widening the EL's tx-packing window.
        //
        // This pipelining is sound because none of the parallel work touches the L2
        // canonical chain: the predicted L2BlockInfo is only fed to origin selection (uses
        // `l1_origin` + `timestamp`) and to L1 prefetches keyed by the epoch's L1 hash.
        // The authoritative `prepare_payload_attributes` still runs with the actual sealed
        // parent against the canonicalized L2 chain.
        let predicted_parent = predict_unsealed_block_info(to_seal);

        let (seal_duration, sealed_payload) = {
            let engine_client = &self.engine_client;
            let conductor = self.conductor.as_ref();
            let unsafe_payload_gossip_client = &self.unsafe_payload_gossip_client;
            let origin_selector = &mut self.origin_selector;
            let attributes_builder = &mut self.attributes_builder;
            let in_recovery_mode = self.in_recovery_mode;

            let seal_fut = async move {
                let seal_request_start = Instant::now();

                let payload = engine_client
                    .seal_and_canonicalize_block(
                        to_seal.payload_id,
                        to_seal.attributes_with_parent.clone(),
                    )
                    .await?;

                update_seal_duration_metrics(seal_request_start.elapsed());
                update_total_transactions_sequenced(
                    to_seal.attributes_with_parent.count_transactions(),
                );

                if let Some(conductor) = conductor {
                    let cc_start = Instant::now();
                    if let Err(err) = conductor.commit_unsafe_payload(&payload).await {
                        error!(target: "sequencer", ?err,
                            "Failed to commit unsafe payload to conductor");
                    }
                    update_conductor_commitment_duration_metrics(cc_start.elapsed());
                }

                // Hand the payload to the gossip client; keep a clone for cache priming.
                let cache_payload = payload.clone();
                unsafe_payload_gossip_client
                    .schedule_execution_payload_gossip(payload)
                    .await
                    .map_err(SequencerActorError::from)?;

                Ok::<(Duration, OpExecutionPayloadEnvelope), SequencerActorError>((
                    seal_request_start.elapsed(),
                    cache_payload,
                ))
            };

            let prefetch_fut = async move {
                // Best-effort: failures here just mean the authoritative call later will
                // re-fetch and surface errors. Never propagate.
                let l1_origin = match origin_selector
                    .next_l1_origin(predicted_parent, in_recovery_mode)
                    .await
                {
                    Ok(o) => o,
                    Err(err) => {
                        warn!(target: "sequencer", ?err,
                            "Best-effort next-L1-origin pre-selection failed");
                        return;
                    }
                };

                let is_new_epoch = l1_origin.number != predicted_parent.l1_origin.number ||
                    l1_origin.hash != predicted_parent.l1_origin.hash;
                attributes_builder
                    .prefetch_for_epoch(
                        BlockNumHash { number: l1_origin.number, hash: l1_origin.hash },
                        is_new_epoch,
                    )
                    .await;
            };

            let (seal_res, _) = tokio::join!(seal_fut, prefetch_fut);
            seal_res?
        };

        // Step 3: derive the new unsafe head and prime the L2 SystemConfig cache from
        // the just-sealed payload.
        //
        // Both halves are REQUIRED for correctness in the deferred-canonicalize-FCU
        // pipeline:
        //
        //   * `new_unsafe_head` is fed into `build_unsealed_payload` directly instead
        //     of re-reading `unsafe_head_rx`. The engine actor publishes the watch
        //     update from its top-level drain loop AFTER the `response_tx.send` that
        //     wakes us from `seal_and_canonicalize_block`, and tokio doesn't guarantee
        //     the engine task continues past that send before this task resumes —
        //     reading the watch here can return the pre-seal head and silently
        //     re-build the same block.
        //
        //   * `cache_sealed_block` derives `SystemConfig` locally and seeds the cache so
        //     the upcoming `prepare_payload_attributes` call's
        //     `system_config_by_number(parent.number)` lookup hits the cache. With the
        //     canonicalize FCU deferred, the EL's canonical chain is still at
        //     `parent.number - 1` until the next `FCU(attrs)` runs, so a fall-through
        //     to `eth_getBlockByNumber` would return null and abort the build.
        //
        // Therefore, conversion failures are propagated as
        // `SealedPayloadConversion` rather than silently falling back. In steady state
        // these conversions never fail — the EL just sealed the block and we hold the
        // full payload — so an error here genuinely indicates either a malformed
        // payload (invalid: should reset) or a programming bug.
        let parent_beacon_block_root =
            sealed_payload.parent_beacon_block_root.unwrap_or_default();
        let block: OpBlock = match sealed_payload.execution_payload {
            OpExecutionPayload::V4(_) => {
                let sidecar = OpExecutionPayloadSidecar::v4(
                    CancunPayloadFields::new(parent_beacon_block_root, Vec::new()),
                    PraguePayloadFields::new(EMPTY_REQUESTS_HASH),
                );
                sealed_payload
                    .execution_payload
                    .try_into_block_with_sidecar(&sidecar)
                    .map_err(|e| SequencerActorError::SealedPayloadConversion(e.to_string()))?
            }
            OpExecutionPayload::V3(_) => {
                let sidecar = OpExecutionPayloadSidecar::v3(CancunPayloadFields::new(
                    parent_beacon_block_root,
                    Vec::new(),
                ));
                sealed_payload
                    .execution_payload
                    .try_into_block_with_sidecar(&sidecar)
                    .map_err(|e| SequencerActorError::SealedPayloadConversion(e.to_string()))?
            }
            OpExecutionPayload::V1(_) | OpExecutionPayload::V2(_) => sealed_payload
                .execution_payload
                .try_into_block::<OpTxEnvelope>()
                .map_err(|e| SequencerActorError::SealedPayloadConversion(e.to_string()))?,
        };

        let new_unsafe_head = L2BlockInfo::from_block_and_genesis(
            &block,
            &self.rollup_config.genesis,
        )
        .map_err(|e| SequencerActorError::SealedPayloadConversion(e.to_string()))?;

        // Prime the SystemConfig cache before kicking off the next build.
        self.attributes_builder.cache_sealed_block(block).await;

        // Step 4: build the next payload, passing the freshly-derived parent so we
        // don't depend on the engine actor's watch update having propagated yet. With
        // caches warmed, the L1/L2 fetches inside `prepare_payload_attributes` are
        // mostly cache hits.
        let unsealed_payload_handle = self.build_unsealed_payload(Some(new_unsafe_head)).await?;

        Ok(SealLastStartNextResult { unsealed_payload_handle, seal_duration })
    }

    /// Starts building an L2 block by creating and populating payload attributes referencing the
    /// correct L1 origin block and sending them to the block engine.
    ///
    /// `unsafe_head` is the parent the new block builds on top of. Pass `None` to read it
    /// from the engine's watch channel (the safe default whenever the local sync state
    /// might lag the engine actor — e.g. on cold start). Pass `Some(_)` to bypass the
    /// watch and use a caller-supplied parent — used in the steady-state path right
    /// after `engine_client.seal_and_canonicalize_block` returns, where the watch
    /// channel update from the engine actor's drain loop has not yet propagated.
    pub(super) async fn build_unsealed_payload(
        &mut self,
        unsafe_head: Option<L2BlockInfo>,
    ) -> Result<Option<UnsealedPayloadHandle>, SequencerActorError> {
        let unsafe_head = match unsafe_head {
            Some(h) => h,
            None => self.engine_client.get_unsafe_head().await?,
        };

        let Some(l1_origin) = self.get_next_payload_l1_origin(unsafe_head).await? else {
            // Temporary error - retry on next tick.
            return Ok(None);
        };

        info!(
            target: "sequencer",
            parent_num = unsafe_head.block_info.number,
            l1_origin_num = l1_origin.number,
            "Started sequencing new block"
        );

        // Build the payload attributes for the next block.
        let attributes_build_start = Instant::now();

        let Some(attributes_with_parent) = self.build_attributes(unsafe_head, l1_origin).await?
        else {
            // Temporary error or reset - retry on next tick.
            return Ok(None);
        };

        update_attributes_build_duration_metrics(attributes_build_start.elapsed());

        // Send the built attributes to the engine to be built.
        let build_request_start = Instant::now();

        let payload_id =
            self.engine_client.start_build_block(attributes_with_parent.clone()).await?;

        update_block_build_duration_metrics(build_request_start.elapsed());

        Ok(Some(UnsealedPayloadHandle {
            payload_id,
            attributes_with_parent,
            l1_origin_used: l1_origin,
        }))
    }

    /// Determines and validates the L1 origin block for the provided L2 unsafe head.
    /// Returns `Ok(None)` for temporary errors that should be retried.
    async fn get_next_payload_l1_origin(
        &mut self,
        unsafe_head: L2BlockInfo,
    ) -> Result<Option<BlockInfo>, SequencerActorError> {
        let l1_origin = match self
            .origin_selector
            .next_l1_origin(unsafe_head, self.in_recovery_mode)
            .await
        {
            Ok(l1_origin) => l1_origin,
            Err(err) => {
                warn!(
                    target: "sequencer",
                    ?err,
                    "Temporary error occurred while selecting next L1 origin. Re-attempting on next tick."
                );
                return Ok(None);
            }
        };

        if unsafe_head.l1_origin.hash != l1_origin.parent_hash &&
            unsafe_head.l1_origin.hash != l1_origin.hash
        {
            warn!(
                target: "sequencer",
                l1_origin = ?l1_origin,
                unsafe_head_hash = %unsafe_head.l1_origin.hash,
                unsafe_head_l1_origin = ?unsafe_head.l1_origin,
                "Cannot build new L2 block on inconsistent L1 origin, resetting engine"
            );
            self.engine_client.reset_engine_forkchoice().await?;
            return Ok(None);
        }
        Ok(Some(l1_origin))
    }

    /// Builds the `OpAttributesWithParent` for the next block to build. If None is returned, it
    /// indicates that no attributes could be built at this time but future attempts may be made.
    async fn build_attributes(
        &mut self,
        unsafe_head: L2BlockInfo,
        l1_origin: BlockInfo,
    ) -> Result<Option<OpAttributesWithParent>, SequencerActorError> {
        let mut attributes = match self
            .attributes_builder
            .prepare_payload_attributes(unsafe_head, l1_origin.id())
            .await
        {
            Ok(attrs) => attrs,
            Err(PipelineErrorKind::Temporary(_)) => {
                // Temporary error - retry on next tick.
                return Ok(None);
            }
            Err(PipelineErrorKind::Reset(_)) => {
                if let Err(err) = self.engine_client.reset_engine_forkchoice().await {
                    error!(target: "sequencer", ?err, "Failed to reset engine");
                    return Err(SequencerActorError::ChannelClosed);
                }

                warn!(
                    target: "sequencer",
                    "Resetting engine due to pipeline error while preparing payload attributes"
                );
                return Ok(None);
            }
            Err(err @ PipelineErrorKind::Critical(_)) => {
                error!(target: "sequencer", ?err, "Failed to prepare payload attributes");
                return Err(err.into());
            }
        };

        attributes.no_tx_pool = Some(!self.should_use_tx_pool(l1_origin, &attributes));

        let attrs_with_parent = OpAttributesWithParent::new(attributes, unsafe_head, None, false);
        Ok(Some(attrs_with_parent))
    }

    /// Determines, for the provided L1 origin block and payload attributes being constructed, if
    /// transaction pool transactions should be enabled.
    fn should_use_tx_pool(&self, l1_origin: BlockInfo, attributes: &OpPayloadAttributes) -> bool {
        if self.in_recovery_mode {
            warn!(target: "sequencer", "Sequencer is in recovery mode, producing empty block");
            return false;
        }

        // If the next L2 block is beyond the sequencer drift threshold, we must produce an empty
        // block.
        if attributes.payload_attributes.timestamp >
            l1_origin.timestamp + self.rollup_config.max_sequencer_drift(l1_origin.timestamp)
        {
            return false;
        }

        // Do not include transactions in the first Ecotone block.
        if self.rollup_config.is_first_ecotone_block(attributes.payload_attributes.timestamp) {
            info!(target: "sequencer", "Sequencing ecotone upgrade block");
            return false;
        }

        // Do not include transactions in the first Fjord block.
        if self.rollup_config.is_first_fjord_block(attributes.payload_attributes.timestamp) {
            info!(target: "sequencer", "Sequencing fjord upgrade block");
            return false;
        }

        // Do not include transactions in the first Granite block.
        if self.rollup_config.is_first_granite_block(attributes.payload_attributes.timestamp) {
            info!(target: "sequencer", "Sequencing granite upgrade block");
            return false;
        }

        // Do not include transactions in the first Holocene block.
        if self.rollup_config.is_first_holocene_block(attributes.payload_attributes.timestamp) {
            info!(target: "sequencer", "Sequencing holocene upgrade block");
            return false;
        }

        // Do not include transactions in the first Isthmus block.
        if self.rollup_config.is_first_isthmus_block(attributes.payload_attributes.timestamp) {
            info!(target: "sequencer", "Sequencing isthmus upgrade block");
            return false;
        }

        // Do not include transactions in the first Jovian block.
        // See: `<https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/jovian/derivation.md#activation-block-rules>`
        if self.rollup_config.is_first_jovian_block(attributes.payload_attributes.timestamp) {
            info!(target: "sequencer", "Sequencing jovian upgrade block");
            return false;
        }

        // Do not include transactions in the first Karst block.
        // See: `<https://github.com/ethereum-optimism/specs/tree/main/specs/protocol/karst>`
        if self.rollup_config.is_first_karst_block(attributes.payload_attributes.timestamp) {
            info!(target: "sequencer", "Sequencing karst upgrade block");
            return false;
        }

        // Do not include transactions in the first Interop block.
        if self.rollup_config.is_first_interop_block(attributes.payload_attributes.timestamp) {
            info!(target: "sequencer", "Sequencing interop upgrade block");
            return false;
        }

        // Transaction pool transactions are enabled if none of the reasons to disable are satisfied
        // above.
        true
    }

    /// Schedules the initial engine reset request and waits for the unsafe head to be updated.
    async fn schedule_initial_reset(&self) -> Result<(), SequencerActorError> {
        // Reset the engine, in order to initialize the engine state.
        // NB: this call waits for confirmation that the reset succeeded and we can proceed with
        // post-reset logic.
        self.engine_client.reset_engine_forkchoice().await.map_err(|err| {
            error!(target: "sequencer", ?err, "Failed to send reset request to engine");
            err.into()
        })
    }
}

#[async_trait]
impl<
    AttributesBuilder_,
    Conductor_,
    OriginSelector_,
    SequencerEngineClient_,
    UnsafePayloadGossipClient_,
> NodeActor
    for SequencerActor<
        AttributesBuilder_,
        Conductor_,
        OriginSelector_,
        SequencerEngineClient_,
        UnsafePayloadGossipClient_,
    >
where
    AttributesBuilder_: AttributesBuilder + Sync + 'static,
    Conductor_: Conductor + Sync + 'static,
    OriginSelector_: OriginSelector + Sync + 'static,
    SequencerEngineClient_: SequencerEngineClient + Sync + 'static,
    UnsafePayloadGossipClient_: UnsafePayloadGossipClient + Sync + 'static,
{
    type Error = SequencerActorError;
    type StartData = ();

    async fn start(mut self, _: Self::StartData) -> Result<(), Self::Error> {
        let mut build_ticker =
            tokio::time::interval(Duration::from_secs(self.rollup_config.block_time));

        self.update_metrics();

        // Reset the engine state prior to beginning block building.
        self.schedule_initial_reset().await?;

        let mut next_payload_to_seal: Option<UnsealedPayloadHandle> = None;
        let mut last_seal_duration = Duration::from_secs(0);
        loop {
            select! {
                // We are using a biased select here to ensure that the admin queries are given priority over the block building task.
                // This is important to limit the occurrence of race conditions where a stopped query is received when a sequencer is building a new block.
                biased;
                _ = self.cancellation_token.cancelled() => {
                    info!(
                        target: "sequencer",
                        "Received shutdown signal. Exiting sequencer task."
                    );
                    return Ok(());
                }
                Some(query) = self.admin_api_rx.recv() => {
                    let active_before = self.is_active;

                    self.handle_admin_query(query).await;

                    // immediately attempt to build a block if the sequencer was just started
                    if !active_before && self.is_active {
                        build_ticker.reset_immediately();
                    }
                }
                // The sequencer must be active to build new blocks.
                _ = build_ticker.tick(), if self.is_active => {

                    match self.seal_last_and_start_next(next_payload_to_seal.as_ref()).await {
                        Ok(res) => {
                            next_payload_to_seal = res.unsealed_payload_handle;
                            last_seal_duration = res.seal_duration;
                        },
                        Err(SequencerActorError::EngineError(EngineClientError::SealError(err))) => {
                            if is_seal_task_err_fatal(&err) {
                                error!(target: "sequencer", err=?err, "Critical seal task error occurred");
                                self.cancellation_token.cancel();
                                return Err(SequencerActorError::EngineError(EngineClientError::SealError(err)));
                            }
                            next_payload_to_seal = None;
                        },
                        Err(other_err) => {
                            error!(target: "sequencer", err = ?other_err, "Unexpected error building or sealing payload");
                            self.cancellation_token.cancel();
                            return Err(other_err);
                        }
                    }

                    if let Some(ref payload) = next_payload_to_seal {
                        let next_block_seconds = payload.attributes_with_parent.parent().block_info.timestamp.saturating_add(self.rollup_config.block_time);
                        // Reserve a sealing margin before the target block timestamp so that
                        // `engine_getPayload` is dispatched in time to complete around the
                        // target time, leaving the EL the full `block_time - margin` window
                        // to fill transactions. We adapt to observed seal latency by taking
                        // the max of the constant default and the last measured seal duration:
                        // this matches op-node's `payloadTime - sealingDuration` schedule
                        // while also handling kona's heavier seal task (which includes
                        // newPayload + canonicalize FCU on top of getPayload).
                        let sealing_margin = std::cmp::max(DEFAULT_SEALING_DURATION, last_seal_duration);
                        let next_block_time = UNIX_EPOCH
                            + Duration::from_secs(next_block_seconds)
                            - sealing_margin;
                        match next_block_time.duration_since(SystemTime::now()) {
                            Ok(duration) => build_ticker.reset_after(duration),
                            Err(_) => build_ticker.reset_immediately(),
                        };
                    } else {
                        build_ticker.reset_immediately();
                    }
                }
            }
        }
    }
}

impl<
    AttributesBuilder_,
    Conductor_,
    OriginSelector_,
    SequencerEngineClient_,
    UnsafePayloadGossipClient_,
> CancellableContext
    for SequencerActor<
        AttributesBuilder_,
        Conductor_,
        OriginSelector_,
        SequencerEngineClient_,
        UnsafePayloadGossipClient_,
    >
where
    AttributesBuilder_: AttributesBuilder,
    Conductor_: Conductor,
    OriginSelector_: OriginSelector,
    SequencerEngineClient_: SequencerEngineClient,
    UnsafePayloadGossipClient_: UnsafePayloadGossipClient,
{
    fn cancelled(&self) -> WaitForCancellationFuture<'_> {
        self.cancellation_token.cancelled()
    }
}

// Determines whether the provided [`SealTaskError`] is fatal for the sequencer.
//
// NB: We could use `err.severity()`, but that gives EngineActor control over this classification.
// `SequencerActor` may have different interpretations of severity, and it is not clear when making
// a change in that area of the codebase that it will affect this area. When a new task error is
// added, this approach guarantees compilation will fail until it is handled here.
fn is_seal_task_err_fatal(err: &SealTaskError) -> bool {
    match err {
        SealTaskError::PayloadInsertionFailed(insert_err) => match &**insert_err {
            InsertTaskError::ForkchoiceUpdateFailed(synchronize_error) => match synchronize_error {
                SynchronizeTaskError::FinalizedAheadOfUnsafe(_, _) => true,
                SynchronizeTaskError::ForkchoiceUpdateFailed(_) |
                SynchronizeTaskError::InvalidForkchoiceState |
                SynchronizeTaskError::UnexpectedPayloadStatus(_) => false,
            },
            InsertTaskError::FromBlockError(_) | InsertTaskError::L2BlockInfoConstruction(_) => {
                true
            }
            InsertTaskError::InsertFailed(_) | InsertTaskError::UnexpectedPayloadStatus(_) => false,
        },
        SealTaskError::GetPayloadFailed(_) |
        SealTaskError::HoloceneInvalidFlush |
        SealTaskError::UnsafeHeadChangedSinceBuild => false,
        SealTaskError::DepositOnlyPayloadFailed |
        SealTaskError::DepositOnlyPayloadReattemptFailed |
        SealTaskError::FromBlock(_) |
        SealTaskError::MpscSend(_) |
        SealTaskError::ClockWentBackwards => true,
    }
}
