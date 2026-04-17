use crate::error::ChannelEngineError;
use alloy_eips::{BlockId, eip1898::BlockNumberOrTag, eip7685::EMPTY_REQUESTS_HASH};
use alloy_network::{Ethereum, Network};
use alloy_primitives::{Address, StorageKey};
use alloy_provider::{EthGetBlock, Provider, RootProvider, RpcWithBlock};
use alloy_rpc_types_engine::{
    CancunPayloadFields, ForkchoiceState, ForkchoiceUpdated, PayloadId, PayloadStatus,
    PraguePayloadFields,
};
use alloy_rpc_types_eth::{Block, EIP1186AccountProofResponse};
use alloy_transport::TransportErrorKind;
use async_trait::async_trait;
use kona_engine::{
    EngineClient, EngineClientError, EngineForkchoiceVersion, EngineGetPayloadVersion,
};
use kona_genesis::RollupConfig;
use kona_protocol::L2BlockInfo;
use op_alloy_network::Optimism;
use op_alloy_rpc_types::Transaction;
use op_alloy_rpc_types_engine::{
    OpExecutionData, OpExecutionPayload, OpExecutionPayloadEnvelope, OpExecutionPayloadSidecar,
    OpPayloadAttributes,
};
use reth_engine_primitives::ConsensusEngineHandle;
use reth_optimism_node::engine::OpEngineTypes;
use reth_payload_builder::{PayloadBuilderHandle, PayloadKind};
use reth_payload_primitives::EngineApiMessageVersion;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, warn};

// Replaces the HTTP-based OpEngineClient for in-process use.
// FCU and new_payload go directly to reth via channel handles — no JWT, no HTTP.
// L1/L2 block queries still use HTTP providers since they read external chain state.
pub struct ChannelEngineClient {
    // FCU + new_payload — talks to reth engine tree over an in-memory mpsc channel
    engine_handle: ConsensusEngineHandle<OpEngineTypes>,
    // get_payload — talks to the payload builder service directly
    payload_handle: PayloadBuilderHandle<OpEngineTypes>,
    cfg: Arc<RollupConfig>,
    // used only for L1 block lookups during sync (not on the block building hot path)
    l1_provider: RootProvider,
    // used for L2 block/proof queries — points at reth's own RPC port
    l2_provider: RootProvider<Optimism>,
}

impl ChannelEngineClient {
    pub fn new(
        engine_handle: ConsensusEngineHandle<OpEngineTypes>,
        payload_handle: PayloadBuilderHandle<OpEngineTypes>,
        cfg: Arc<RollupConfig>,
        l1_provider: RootProvider,
        l2_provider: RootProvider<Optimism>,
    ) -> Self {
        Self { engine_handle, payload_handle, cfg, l1_provider, l2_provider }
    }
}

#[async_trait]
impl EngineClient for ChannelEngineClient {
    fn cfg(&self) -> &RollupConfig {
        self.cfg.as_ref()
    }

    // L1 block reads during derivation. Not on the block-building hot path.
    fn get_l1_block(&self, block: BlockId) -> EthGetBlock<<Ethereum as Network>::BlockResponse> {
        self.l1_provider.get_block(block)
    }

    // L2 block reads — goes to reth's own HTTP port, not the engine channel.
    // No in-process shortcut here yet; these aren't latency-sensitive.
    fn get_l2_block(&self, block: BlockId) -> EthGetBlock<<Optimism as Network>::BlockResponse> {
        self.l2_provider.get_block(block)
    }

    // eth_getProof for storage slot validation during derivation.
    // Still HTTP — no direct handle into reth's DB layer exposed yet.
    fn get_proof(
        &self,
        address: Address,
        keys: Vec<StorageKey>,
    ) -> RpcWithBlock<(Address, Vec<StorageKey>), EIP1186AccountProofResponse> {
        self.l2_provider.get_proof(address, keys)
    }

    // Over HTTP, engine_newPayloadV3 takes 3 positional params:
    //   [0] ExecutionPayloadV3  [1] versioned_hashes (always [])  [2] parent_beacon_block_root
    // kona's envelope wraps all three into one struct. We split them back out here —
    // same work reth's HTTP handler was doing after JSON deserialization.
    // Sidecar carries the beacon root for EIP-4788; blobs are always empty on OP.
    async fn new_payload(
        &self,
        envelope: OpExecutionPayloadEnvelope,
    ) -> alloy_transport::TransportResult<PayloadStatus> {
        let pbr = envelope.parent_beacon_block_root;
        let sidecar = match &envelope.execution_payload {
            OpExecutionPayload::V1(_) | OpExecutionPayload::V2(_) => {
                OpExecutionPayloadSidecar::default()
            }
            OpExecutionPayload::V3(_) => {
                let pbr = pbr.unwrap_or_default();
                // blobs are always empty on OP
                OpExecutionPayloadSidecar::v3(CancunPayloadFields {
                    parent_beacon_block_root: pbr,
                    versioned_hashes: vec![],
                })
            }
            OpExecutionPayload::V4(_) => {
                let pbr = pbr.unwrap_or_default();
                // Isthmus (V4): must pass Prague sidecar with requests_hash so the validator
                // sets header.requests_hash = EMPTY_REQUESTS_HASH when reconstructing the block.
                // Without it the block hash diverges from the one stored in payload.block_hash().
                // EL requests are always empty on OP per the spec.
                OpExecutionPayloadSidecar::v4(
                    CancunPayloadFields { parent_beacon_block_root: pbr, versioned_hashes: vec![] },
                    PraguePayloadFields::new(EMPTY_REQUESTS_HASH),
                )
            }
        };

        let data = OpExecutionData::new(envelope.execution_payload, sidecar);

        let start = Instant::now();
        let result = self.engine_handle.new_payload(data).await;
        let elapsed = start.elapsed();

        match &result {
            Ok(status) => debug!(target: "engine_bridge", ?elapsed, ?status, "new_payload ok"),
            Err(e) => warn!(target: "engine_bridge", ?elapsed, %e, "new_payload err"),
        }

        result.map_err(|e| TransportErrorKind::custom(ChannelEngineError::NewPayload(e.to_string())))
    }

    // ForkchoiceState and OpPayloadAttributes are shared alloy types — both kona and reth
    // import them from the same crate, so no struct conversion needed here.
    // The only mapping is the version enum: kona uses EngineForkchoiceVersion,
    // reth uses EngineApiMessageVersion — different names, same V2/V3 concept.
    // Two awaits when attrs are present: one for the engine tree to process FCU,
    // a second for the payload builder to register the job and return a payload_id.
    async fn fork_choice_updated(
        &self,
        version: EngineForkchoiceVersion,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<OpPayloadAttributes>,
    ) -> alloy_transport::TransportResult<ForkchoiceUpdated> {
        let api_version = match version {
            EngineForkchoiceVersion::V2 => EngineApiMessageVersion::V2,
            EngineForkchoiceVersion::V3 => EngineApiMessageVersion::V3,
        };

        debug!(
            target: "engine_bridge",
            ?version,
            head = %fork_choice_state.head_block_hash,
            attrs = payload_attributes.is_some(),
            "FCU"
        );

        let start = Instant::now();
        let result = self
            .engine_handle
            .fork_choice_updated(fork_choice_state, payload_attributes, api_version)
            .await;
        let elapsed = start.elapsed();

        match &result {
            Ok(fcu) => debug!(target: "engine_bridge", ?elapsed, payload_id = ?fcu.payload_id, "FCU ok"),
            Err(e) => warn!(target: "engine_bridge", ?elapsed, %e, "FCU err"),
        }

        result.map_err(|e| TransportErrorKind::custom(ChannelEngineError::Fcu(e.to_string())))
    }

    // Inverse of new_payload: reth has a built OpBuiltPayload, kona wants OpExecutionPayloadEnvelope.
    // Over HTTP reth would serialize it to JSON and kona would deserialize — here we convert directly.
    // The From impls on OpBuiltPayload handle the versioned field mapping.
    // WaitForPending: if the build job is still running when kona asks, block until it finishes.
    async fn get_payload(
        &self,
        version: EngineGetPayloadVersion,
        payload_id: PayloadId,
    ) -> alloy_transport::TransportResult<OpExecutionPayloadEnvelope> {
        debug!(target: "engine_bridge", ?version, ?payload_id, "get_payload");

        let start = Instant::now();
        let built = self
            .payload_handle
            .resolve_kind(payload_id, PayloadKind::WaitForPending)
            .await
            .ok_or_else(|| {
                warn!(target: "engine_bridge", ?payload_id, "get_payload: not found");
                TransportErrorKind::custom(ChannelEngineError::PayloadNotFound(payload_id))
            })?
            .map_err(|e| {
                warn!(target: "engine_bridge", ?payload_id, %e, "get_payload err");
                TransportErrorKind::custom(ChannelEngineError::PayloadBuilder(e.to_string()))
            })?;
        debug!(target: "engine_bridge", elapsed = ?start.elapsed(), ?payload_id, "get_payload ok");

        let envelope = match version {
            EngineGetPayloadVersion::V2 => {
                let env = alloy_rpc_types_engine::ExecutionPayloadEnvelopeV2::from(built);
                use alloy_rpc_types_engine::ExecutionPayload;
                let payload = env.execution_payload.into_payload();
                OpExecutionPayloadEnvelope {
                    parent_beacon_block_root: None,
                    execution_payload: match payload {
                        ExecutionPayload::V1(p) => OpExecutionPayload::V1(p),
                        ExecutionPayload::V2(p) => OpExecutionPayload::V2(p),
                        _ => unreachable!("V2 envelope must be V1 or V2"),
                    },
                }
            }
            EngineGetPayloadVersion::V3 => {
                let env = op_alloy_rpc_types_engine::OpExecutionPayloadEnvelopeV3::from(built);
                OpExecutionPayloadEnvelope {
                    parent_beacon_block_root: Some(env.parent_beacon_block_root),
                    execution_payload: OpExecutionPayload::V3(env.execution_payload),
                }
            }
            EngineGetPayloadVersion::V4 => {
                let env = op_alloy_rpc_types_engine::OpExecutionPayloadEnvelopeV4::from(built);
                OpExecutionPayloadEnvelope {
                    parent_beacon_block_root: Some(env.parent_beacon_block_root),
                    execution_payload: OpExecutionPayload::V4(env.execution_payload),
                }
            }
        };

        Ok(envelope)
    }

    // L2 block fetch by tag (latest/safe/finalized). Used by kona to track chain head.
    async fn l2_block_by_label(
        &self,
        numtag: BlockNumberOrTag,
    ) -> Result<Option<Block<Transaction>>, EngineClientError> {
        Ok(self.l2_provider.get_block_by_number(numtag).full().await?)
    }

    // Same as above but returns L2BlockInfo — kona's enriched view with L1 origin,
    // seq number, etc. Derived from the raw block + genesis config.
    async fn l2_block_info_by_label(
        &self,
        numtag: BlockNumberOrTag,
    ) -> Result<Option<L2BlockInfo>, EngineClientError> {
        let block = self.l2_provider.get_block_by_number(numtag).full().await?;
        let Some(block) = block else { return Ok(None) };
        Ok(Some(L2BlockInfo::from_block_and_genesis(
            &block.into_consensus(),
            &self.cfg.genesis,
        )?))
    }
}
