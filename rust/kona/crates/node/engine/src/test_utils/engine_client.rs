//! Mock implementations for testing engine client functionality.

use crate::{EngineClient, EngineClientError, EngineForkchoiceVersion, EngineGetPayloadVersion};
use alloy_eips::{BlockId, eip1898::BlockNumberOrTag};
use alloy_network::{Ethereum, Network};
use alloy_primitives::{Address, StorageKey, B256, U256};
use alloy_provider::{EthGetBlock, ProviderCall, RpcWithBlock};
use alloy_rpc_types_engine::{ExecutionPayloadV1, ForkchoiceState, ForkchoiceUpdated, PayloadId, PayloadStatus};
use alloy_rpc_types_eth::{Block, EIP1186AccountProofResponse, Transaction as EthTransaction};
use alloy_transport::{TransportError, TransportErrorKind, TransportResult};
use async_trait::async_trait;
use kona_genesis::RollupConfig;
use kona_protocol::L2BlockInfo;
use op_alloy_network::Optimism;
use op_alloy_rpc_types::Transaction as OpTransaction;
use op_alloy_rpc_types_engine::{OpExecutionPayload, OpExecutionPayloadEnvelope, OpPayloadAttributes};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;

/// Builder for creating test `MockEngineClient` instances with sensible defaults
pub fn test_engine_client_builder() -> MockEngineClientBuilder {
    MockEngineClientBuilder::new().with_config(Arc::new(RollupConfig::default()))
}

/// Mock storage for engine client responses.
#[derive(Debug, Clone, Default)]
pub struct MockEngineStorage {
    /// Storage for block responses by tag.
    pub l2_blocks_by_label: HashMap<BlockNumberOrTag, Block<OpTransaction>>,
    /// Storage for block info responses by tag.
    pub block_info_by_tag: HashMap<BlockNumberOrTag, L2BlockInfo>,

    /// Response for `new_payload` calls.
    pub new_payload_response: Option<PayloadStatus>,
    /// Response for `fork_choice_updated` calls.
    pub fork_choice_updated_response: Option<ForkchoiceUpdated>,
    /// Response for `get_payload` calls.
    pub get_payload_response: Option<OpExecutionPayloadEnvelope>,

    // Storage for get_l1_block, get_l2_block, and get_proof
    /// Storage for L1 blocks by stringified `BlockId`.
    pub l1_blocks_by_id: HashMap<String, Block<EthTransaction>>,
    /// Storage for L2 blocks by stringified `BlockId`.
    pub l2_blocks_by_id: HashMap<String, Block<OpTransaction>>,
    /// Storage for proofs by (address, stringified `BlockId`) key.
    pub proofs_by_address: HashMap<(Address, String), EIP1186AccountProofResponse>,
}

/// Builder for constructing a [`MockEngineClient`] with pre-configured responses.
///
/// This builder allows you to set up mock responses before creating the client,
/// making it easier to write concise tests.
///
/// # Example
///
/// ```rust,ignore
/// use kona_engine::test_utils::{MockEngineClient};
/// use alloy_rpc_types_engine::{PayloadStatus, PayloadStatusEnum};
/// use std::sync::Arc;
///
/// let mock = MockEngineClient::builder()
///     .with_config(Arc::new(RollupConfig::default()))
///     .with_payload_status(PayloadStatus {
///         status: PayloadStatusEnum::Valid,
///         latest_valid_hash: Some(B256::ZERO),
///     })
///     .build();
/// ```
#[derive(Debug)]
pub struct MockEngineClientBuilder {
    cfg: Option<Arc<RollupConfig>>,
    storage: MockEngineStorage,
}

impl MockEngineClientBuilder {
    /// Creates a new builder with default values.
    pub fn new() -> Self {
        Self { cfg: None, storage: MockEngineStorage::default() }
    }

    /// Sets the rollup configuration.
    pub fn with_config(mut self, cfg: Arc<RollupConfig>) -> Self {
        self.cfg = Some(cfg);
        self
    }

    /// Sets a block response for a specific tag.
    pub fn with_l2_block_by_label(
        mut self,
        tag: BlockNumberOrTag,
        block: Block<OpTransaction>,
    ) -> Self {
        self.storage.l2_blocks_by_label.insert(tag, block);
        self
    }

    /// Sets a block info response for a specific tag.
    pub fn with_block_info_by_tag(mut self, tag: BlockNumberOrTag, info: L2BlockInfo) -> Self {
        self.storage.block_info_by_tag.insert(tag, info);
        self
    }

    /// Sets the `new_payload` response.
    pub fn with_new_payload_response(mut self, status: PayloadStatus) -> Self {
        self.storage.new_payload_response = Some(status);
        self
    }

    /// Sets the `fork_choice_updated` response.
    pub fn with_fork_choice_updated_response(mut self, response: ForkchoiceUpdated) -> Self {
        self.storage.fork_choice_updated_response = Some(response);
        self
    }

    /// Sets the `get_payload` response.
    pub fn with_get_payload_response(mut self, payload: OpExecutionPayloadEnvelope) -> Self {
        self.storage.get_payload_response = Some(payload);
        self
    }

    /// Sets an L1 block response for a specific `BlockId`.
    pub fn with_l1_block(mut self, block_id: BlockId, block: Block<EthTransaction>) -> Self {
        let key = block_id_to_key(&block_id);
        self.storage.l1_blocks_by_id.insert(key, block);
        self
    }

    /// Sets an L2 block response for a specific `BlockId`.
    pub fn with_l2_block(mut self, block_id: BlockId, block: Block<OpTransaction>) -> Self {
        let key = block_id_to_key(&block_id);
        self.storage.l2_blocks_by_id.insert(key, block);
        self
    }

    /// Sets a proof response for a specific address and `BlockId`.
    pub fn with_proof(
        mut self,
        address: Address,
        block_id: BlockId,
        proof: EIP1186AccountProofResponse,
    ) -> Self {
        let key = block_id_to_key(&block_id);
        self.storage.proofs_by_address.insert((address, key), proof);
        self
    }

    /// Builds the [`MockEngineClient`] with the configured values.
    ///
    /// # Panics
    ///
    /// Panics if any required fields (cfg) are not set.
    pub fn build(self) -> MockEngineClient {
        let cfg = self.cfg.expect("cfg must be set");

        MockEngineClient { cfg, storage: Arc::new(RwLock::new(self.storage)) }
    }
}

impl Default for MockEngineClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns a zeroed [`ExecutionPayloadV1`] for use in tests.
pub fn default_execution_payload_v1() -> ExecutionPayloadV1 {
    ExecutionPayloadV1 {
        parent_hash: B256::ZERO,
        fee_recipient: Address::ZERO,
        state_root: B256::ZERO,
        receipts_root: B256::ZERO,
        logs_bloom: Default::default(),
        prev_randao: B256::ZERO,
        block_number: 0,
        gas_limit: 0,
        gas_used: 0,
        timestamp: 0,
        extra_data: Default::default(),
        base_fee_per_gas: U256::ZERO,
        block_hash: B256::ZERO,
        transactions: vec![],
    }
}

/// Returns a minimal [`OpExecutionPayloadEnvelope`] for use in tests.
pub fn default_payload_envelope() -> OpExecutionPayloadEnvelope {
    OpExecutionPayloadEnvelope {
        parent_beacon_block_root: None,
        execution_payload: OpExecutionPayload::V1(default_execution_payload_v1()),
    }
}

/// Mock implementation of the `EngineClient` trait for testing.
/// Mock implementation of the `EngineClient` trait for testing.
///
/// This mock allows tests to configure expected responses for all `EngineClient`
/// and `OpEngineApi` methods. All responses are stored in a shared [`MockEngineStorage`]
/// protected by an `RwLock` for thread-safe access.
#[derive(Debug, Clone)]
pub struct MockEngineClient {
    /// The rollup configuration.
    cfg: Arc<RollupConfig>,
    /// Shared storage for mock responses.
    storage: Arc<RwLock<MockEngineStorage>>,
}

impl MockEngineClient {
    /// Creates a new mock engine client with the given config.
    pub fn new(cfg: Arc<RollupConfig>) -> Self {
        Self { cfg, storage: Arc::new(RwLock::new(MockEngineStorage::default())) }
    }

    /// Creates a builder for constructing a mock engine client.
    pub fn builder() -> MockEngineClientBuilder {
        MockEngineClientBuilder::new()
    }

    /// Returns a reference to the mock storage for configuring responses.
    pub fn storage(&self) -> Arc<RwLock<MockEngineStorage>> {
        Arc::clone(&self.storage)
    }

    /// Sets a block response for a specific tag.
    pub async fn set_l2_block_by_label(&self, tag: BlockNumberOrTag, block: Block<OpTransaction>) {
        self.storage.write().await.l2_blocks_by_label.insert(tag, block);
    }

    /// Sets a block info response for a specific tag.
    pub async fn set_block_info_by_tag(&self, tag: BlockNumberOrTag, info: L2BlockInfo) {
        self.storage.write().await.block_info_by_tag.insert(tag, info);
    }

    /// Sets the `new_payload` response.
    pub async fn set_new_payload_response(&self, status: PayloadStatus) {
        self.storage.write().await.new_payload_response = Some(status);
    }

    /// Sets the `fork_choice_updated` response.
    pub async fn set_fork_choice_updated_response(&self, response: ForkchoiceUpdated) {
        self.storage.write().await.fork_choice_updated_response = Some(response);
    }

    /// Sets the `get_payload` response.
    pub async fn set_get_payload_response(&self, payload: OpExecutionPayloadEnvelope) {
        self.storage.write().await.get_payload_response = Some(payload);
    }

    /// Sets an L1 block response for a specific `BlockId`.
    pub async fn set_l1_block(&self, block_id: BlockId, block: Block<EthTransaction>) {
        let key = block_id_to_key(&block_id);
        self.storage.write().await.l1_blocks_by_id.insert(key, block);
    }

    /// Sets an L2 block response for a specific `BlockId`.
    pub async fn set_l2_block(&self, block_id: BlockId, block: Block<OpTransaction>) {
        let key = block_id_to_key(&block_id);
        self.storage.write().await.l2_blocks_by_id.insert(key, block);
    }

    /// Sets a proof response for a specific address and `BlockId`.
    pub async fn set_proof(
        &self,
        address: Address,
        block_id: BlockId,
        proof: EIP1186AccountProofResponse,
    ) {
        let key = block_id_to_key(&block_id);
        self.storage.write().await.proofs_by_address.insert((address, key), proof);
    }
}

#[async_trait]
impl EngineClient for MockEngineClient {
    fn cfg(&self) -> &RollupConfig {
        self.cfg.as_ref()
    }

    fn get_l1_block(&self, block: BlockId) -> EthGetBlock<<Ethereum as Network>::BlockResponse> {
        let storage = Arc::clone(&self.storage);
        let block_key = block_id_to_key(&block);

        EthGetBlock::new_provider(
            block,
            Box::new(move |_kind| {
                let storage = Arc::clone(&storage);
                let block_key = block_key.clone();

                ProviderCall::BoxedFuture(Box::pin(async move {
                    let storage_guard = storage.read().await;
                    Ok(storage_guard.l1_blocks_by_id.get(&block_key).cloned())
                }))
            }),
        )
    }

    fn get_l2_block(&self, block: BlockId) -> EthGetBlock<<Optimism as Network>::BlockResponse> {
        let storage = Arc::clone(&self.storage);
        let block_key = block_id_to_key(&block);

        EthGetBlock::new_provider(
            block,
            Box::new(move |_kind| {
                let storage = Arc::clone(&storage);
                let block_key = block_key.clone();

                ProviderCall::BoxedFuture(Box::pin(async move {
                    let storage_guard = storage.read().await;
                    Ok(storage_guard.l2_blocks_by_id.get(&block_key).cloned())
                }))
            }),
        )
    }

    fn get_proof(
        &self,
        address: Address,
        _keys: Vec<StorageKey>,
    ) -> RpcWithBlock<(Address, Vec<StorageKey>), EIP1186AccountProofResponse> {
        let storage = Arc::clone(&self.storage);

        RpcWithBlock::new_provider(move |block_id| {
            let storage = Arc::clone(&storage);
            let block_key = block_id_to_key(&block_id);
            let address = address;

            ProviderCall::BoxedFuture(Box::pin(async move {
                let storage_guard = storage.read().await;
                storage_guard.proofs_by_address.get(&(address, block_key)).cloned().ok_or_else(
                    || {
                        TransportError::from(TransportErrorKind::custom_str(
                            "No proof configured for this address and block. \
                             Use with_proof() or set_proof() to set a response.",
                        ))
                    },
                )
            }))
        })
    }

    async fn new_payload(
        &self,
        _envelope: OpExecutionPayloadEnvelope,
    ) -> TransportResult<PayloadStatus> {
        let storage = self.storage.read().await;
        storage.new_payload_response.clone().ok_or_else(|| {
            TransportError::from(TransportErrorKind::custom_str(
                "new_payload called but no response configured. \
                 Use with_new_payload_response() or set_new_payload_response() to set a response.",
            ))
        })
    }

    async fn fork_choice_updated(
        &self,
        _version: EngineForkchoiceVersion,
        _fork_choice_state: ForkchoiceState,
        _payload_attributes: Option<OpPayloadAttributes>,
    ) -> TransportResult<ForkchoiceUpdated> {
        let storage = self.storage.read().await;
        storage.fork_choice_updated_response.clone().ok_or_else(|| {
            TransportError::from(TransportErrorKind::custom_str(
                "fork_choice_updated called but no response configured. \
                 Use with_fork_choice_updated_response() or set_fork_choice_updated_response() to set a response.",
            ))
        })
    }

    async fn get_payload(
        &self,
        _version: EngineGetPayloadVersion,
        _payload_id: PayloadId,
    ) -> TransportResult<OpExecutionPayloadEnvelope> {
        let storage = self.storage.read().await;
        storage.get_payload_response.clone().ok_or_else(|| {
            TransportError::from(TransportErrorKind::custom_str(
                "get_payload called but no response configured. \
                 Use with_get_payload_response() or set_get_payload_response() to set a response.",
            ))
        })
    }

    async fn l2_block_by_label(
        &self,
        numtag: BlockNumberOrTag,
    ) -> Result<Option<Block<OpTransaction>>, EngineClientError> {
        let storage = self.storage.read().await;
        Ok(storage.l2_blocks_by_label.get(&numtag).cloned())
    }

    async fn l2_block_info_by_label(
        &self,
        numtag: BlockNumberOrTag,
    ) -> Result<Option<L2BlockInfo>, EngineClientError> {
        let storage = self.storage.read().await;
        Ok(storage.block_info_by_tag.get(&numtag).copied())
    }
}

/// Helper function to convert `BlockId` to a string key for `HashMap` storage.
/// This is necessary because `BlockId` doesn't implement Hash.
fn block_id_to_key(block_id: &BlockId) -> String {
    match block_id {
        BlockId::Hash(hash) => format!("hash:{}", hash.block_hash),
        BlockId::Number(num) => format!("number:{num}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use alloy_rpc_types_engine::PayloadStatusEnum;
    use op_alloy_rpc_types_engine::OpExecutionPayload;

    #[tokio::test]
    async fn test_mock_engine_client_creation() {
        let cfg = Arc::new(RollupConfig::default());

        let mock = MockEngineClient::new(cfg.clone());

        // Verify the config was set correctly
        assert_eq!(mock.cfg().block_time, cfg.block_time);
    }

    #[tokio::test]
    async fn test_mock_payload_status() {
        let cfg = Arc::new(RollupConfig::default());
        let mock = MockEngineClient::new(cfg);

        let status =
            PayloadStatus { status: PayloadStatusEnum::Valid, latest_valid_hash: Some(B256::ZERO) };

        mock.set_new_payload_response(status.clone()).await;

        // The mock ignores the envelope value — only the pre-configured response matters.
        let result = mock.new_payload(OpExecutionPayloadEnvelope {
            parent_beacon_block_root: None,
            execution_payload: OpExecutionPayload::V1(default_execution_payload_v1()),
        }).await.unwrap();

        assert_eq!(result.status, status.status);
    }

    #[tokio::test]
    async fn test_mock_forkchoice_updated() {
        let cfg = Arc::new(RollupConfig::default());
        let mock = MockEngineClient::new(cfg);

        let fcu = ForkchoiceUpdated {
            payload_status: PayloadStatus {
                status: PayloadStatusEnum::Valid,
                latest_valid_hash: Some(B256::ZERO),
            },
            payload_id: None,
        };

        mock.set_fork_choice_updated_response(fcu.clone()).await;

        let result = mock
            .fork_choice_updated(EngineForkchoiceVersion::V2, ForkchoiceState::default(), None)
            .await
            .unwrap();

        assert_eq!(result.payload_status.status, fcu.payload_status.status);
    }

    #[tokio::test]
    async fn test_builder_pattern() {
        let cfg = Arc::new(RollupConfig::default());
        let status =
            PayloadStatus { status: PayloadStatusEnum::Valid, latest_valid_hash: Some(B256::ZERO) };

        let mock = MockEngineClient::builder()
            .with_config(cfg.clone())
            .with_new_payload_response(status.clone())
            .build();

        // Verify the config was set
        assert_eq!(mock.cfg().block_time, cfg.block_time);

        // The mock ignores the envelope value — only the pre-configured response matters.
        let result = mock.new_payload(OpExecutionPayloadEnvelope {
            parent_beacon_block_root: None,
            execution_payload: OpExecutionPayload::V1(default_execution_payload_v1()),
        }).await.unwrap();

        assert_eq!(result.status, status.status);
    }

    /// Verify that `EngineClient` is transport-agnostic: a struct with no HTTP types whatsoever
    /// can implement it and be used where `EngineClient` is required as a generic bound.
    ///
    /// This is the compile-time proof that removing `OpEngineApi<Optimism, Http<HyperAuthClient>>`
    /// as a supertrait achieved the intended goal.
    #[tokio::test]
    async fn test_engine_client_is_transport_agnostic() {
        // MockEngineClient has no HTTP types — it uses in-memory storage.
        // If EngineClient still required OpEngineApi<Optimism, Http<HyperAuthClient>>,
        // this would fail to compile.
        fn assert_engine_client<C: EngineClient>(_: &C) {}

        let mock = test_engine_client_builder().build();
        assert_engine_client(&mock);

        // Also verify it works as a trait object (dyn EngineClient).
        let boxed: Box<dyn EngineClient> = Box::new(mock);
        assert_eq!(boxed.cfg().block_time, RollupConfig::default().block_time);
    }
}
