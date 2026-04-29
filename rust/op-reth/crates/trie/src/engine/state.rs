//! [`EngineState`] — all mutable engine state in one place.

use super::{
    buffer::TrieBufferState,
    error::EngineError,
    persistence::{error::PersistenceError, PersistenceHandle},
    DEFAULT_BACKPRESSURE_THRESHOLD, DEFAULT_PERSISTENCE_THRESHOLD, DEFAULT_PERSISTENCE_TIMEOUT_SECS,
};
use crate::{OpProofStoragePruner, OpProofsProviderRO, OpProofsStorageError, OpProofsStore};
#[cfg(feature = "metrics")]
use super::metrics::EngineMetrics;
use alloy_eips::{eip1898::BlockWithParent, NumHash};
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError};
use reth_evm::ConfigureEvm;
use reth_primitives_traits::BlockTy;
use reth_provider::{
    BlockHashReader, BlockReader, DatabaseProviderFactory, StateProviderFactory, StateReader,
};
use std::time::Duration;
#[cfg(feature = "metrics")]
use std::time::Instant;
use tracing::{error, info};

/// Tracks all in-flight and threshold state for background persistence.
pub(crate) struct PersistenceState {
    /// Number of buffered blocks that triggers a background flush.
    pub(crate) threshold: u64,
    /// Number of buffered blocks at which the engine blocks waiting for persistence.
    pub(crate) backpressure_threshold: u64,
    /// Handle to the persistence service.
    handle: PersistenceHandle,
    /// Reply channel for the in-flight save. Present only while a save is running.
    in_flight: Option<Receiver<Result<Option<u64>, PersistenceError>>>,
}

impl PersistenceState {
    const fn new(handle: PersistenceHandle) -> Self {
        Self {
            threshold: DEFAULT_PERSISTENCE_THRESHOLD,
            backpressure_threshold: DEFAULT_BACKPRESSURE_THRESHOLD,
            handle,
            in_flight: None,
        }
    }

    /// Non-blocking: if a save has completed, collect the result and prune `memory`.
    pub(crate) fn poll(&mut self, memory: &TrieBufferState) {
        let Some(rx) = self.in_flight.take() else { return };

        match rx.try_recv() {
            Ok(Ok(Some(last_persisted))) => {
                info!(
                    target: "trie::engine",
                    block_number = last_persisted,
                    "Background persistence completed, pruning memory"
                );
                memory.prune(last_persisted + 1);
            }
            Ok(Ok(None)) => {}
            Ok(Err(e)) => {
                error!(target: "trie::engine", ?e, "Background persistence save failed");
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {
                self.in_flight = Some(rx);
            }
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                error!(target: "trie::engine", "Persistence service disconnected while in-flight");
            }
        }
    }

    /// Blocking: wait for the in-flight save to finish and prune `memory`.
    pub(crate) fn wait(&mut self, memory: &TrieBufferState) {
        let Some(rx) = self.in_flight.take() else { return };

        match rx.recv_timeout(Duration::from_secs(DEFAULT_PERSISTENCE_TIMEOUT_SECS)) {
            Ok(Ok(Some(last_persisted))) => {
                info!(
                    target: "trie::engine",
                    block_number = last_persisted,
                    "Persistence completed (waited), pruning memory"
                );
                memory.prune(last_persisted + 1);
            }
            Ok(Ok(None)) => {}
            Ok(Err(e)) => {
                error!(target: "trie::engine", ?e, "Persistence save failed while waiting");
            }
            Err(RecvTimeoutError::Timeout) => {
                error!(target: "trie::engine", "Persistence timeout while waiting");
            }
            Err(RecvTimeoutError::Disconnected) => {
                error!(target: "trie::engine", "Persistence service disconnected while waiting");
            }
        }
    }

    /// Check buffer size and trigger or gate a background save as needed.
    ///
    /// 1. Poll for any completed save (frees memory before threshold checks).
    /// 2. Block if buffer is above `backpressure_threshold` and a save is in-flight.
    /// 3. Kick off a new save if buffer is above `threshold` and nothing is in-flight.
    pub(crate) fn advance(&mut self, memory: &TrieBufferState) -> Result<(), EngineError> {
        self.poll(memory);

        let current_size = memory.len() as u64;

        if current_size >= self.backpressure_threshold && self.in_flight.is_some() {
            info!(
                target: "trie::engine",
                current_size,
                threshold = self.backpressure_threshold,
                "Backpressure triggered: waiting for persistence to complete"
            );
            self.wait(memory);
            info!(target: "trie::engine", "Backpressure released");
        }

        let current_size = memory.len() as u64;

        if current_size >= self.threshold && self.in_flight.is_none() {
            let blocks = memory.blocks_ordered();
            if blocks.is_empty() {
                return Ok(());
            }

            info!(
                target: "trie::engine",
                current_size,
                count = blocks.len(),
                start_block = blocks.first().map(|arc| arc.0.block.number),
                end_block = blocks.last().map(|arc| arc.0.block.number),
                threshold = self.threshold,
                "Persistence threshold reached: sending to persistence service"
            );

            let (tx, rx) = bounded(1);
            self.handle.save_updates(blocks, tx)?;
            self.in_flight = Some(rx);
        }

        Ok(())
    }

    /// Wait for any in-flight save, then send an unwind to the persistence service and
    /// block until it completes.
    pub(crate) fn unwind(
        &mut self,
        to: BlockWithParent,
        memory: &TrieBufferState,
    ) -> Result<(), EngineError> {
        if self.in_flight.is_some() {
            info!(target: "trie::engine", "Unwind waiting for in-flight persistence...");
            self.wait(memory);
        }

        let (tx, rx) = bounded(1);
        self.handle.unwind(to, tx)?;

        match rx.recv_timeout(Duration::from_secs(DEFAULT_PERSISTENCE_TIMEOUT_SECS)) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e.into()),
            Err(RecvTimeoutError::Timeout) => Err(EngineError::PersistenceTimeout),
            Err(RecvTimeoutError::Disconnected) => Err(EngineError::PersistenceDisconnected),
        }
    }
}

/// All mutable state owned by the engine.
pub(crate) struct EngineState<Evm, Provider, Store>
where
    Evm: ConfigureEvm,
    Provider: StateReader + DatabaseProviderFactory + StateProviderFactory + BlockReader,
{
    /// The highest block number the engine should sync to in its idle time.
    pub(crate) sync_target: u64,

    pub(crate) evm_config: Evm,
    pub(crate) provider: Provider,
    pub(crate) storage: Store,

    pub(crate) memory: TrieBufferState,
    pub(crate) persistence: PersistenceState,

    #[cfg(feature = "metrics")]
    pub(crate) metrics: EngineMetrics,
}

impl<Evm, Provider, Store> EngineState<Evm, Provider, Store>
where
    Evm: ConfigureEvm,
    Provider: BlockHashReader
        + StateReader
        + DatabaseProviderFactory
        + StateProviderFactory
        + BlockReader<Block = BlockTy<Evm::Primitives>>
        + Clone
        + 'static,
    Store: OpProofsStore + Clone + 'static,
{
    pub(crate) fn new(
        evm_config: Evm,
        provider: Provider,
        storage: Store,
        pruner: OpProofStoragePruner<Store, Provider>,
    ) -> Self {
        let persistence_handle = PersistenceHandle::spawn(pruner, storage.clone());
        Self {
            evm_config,
            provider,
            storage,
            memory: TrieBufferState::new(),
            persistence: PersistenceState::new(persistence_handle),
            sync_target: 0,
            #[cfg(feature = "metrics")]
            metrics: EngineMetrics::new_with_labels(&[] as &[(&str, &str)]),
        }
    }

    #[allow(dead_code)]
    pub(crate) const fn with_persistence_threshold(mut self, threshold: u64) -> Self {
        self.persistence.threshold = threshold;
        self
    }

    #[allow(dead_code)]
    pub(crate) const fn with_backpressure_threshold(mut self, threshold: u64) -> Self {
        self.persistence.backpressure_threshold = threshold;
        self
    }

    /// Poll for completed saves, apply backpressure, and kick off new saves at threshold.
    pub(crate) fn advance_persistence(&mut self) -> Result<(), EngineError> {
        self.persistence.advance(&self.memory)
    }

    /// Block until any in-flight background save finishes and the memory buffer is pruned.
    pub(crate) fn drain_persistence(&mut self) {
        self.persistence.wait(&self.memory);
    }

    /// Drain any in-flight save, unwind the persistence service to `to`, then
    /// unwind the in-memory buffer to match.
    pub(crate) fn unwind(&mut self, to: BlockWithParent) -> Result<(), EngineError> {
        #[cfg(feature = "metrics")]
        let start = Instant::now();
        self.persistence.unwind(to, &self.memory)?;
        self.memory.unwind(to.block.number);
        #[cfg(feature = "metrics")]
        self.metrics.unwind_duration_seconds.record(start.elapsed());
        Ok(())
    }

    pub(crate) fn get_tip(&self) -> Result<NumHash, OpProofsStorageError> {
        if let Some(tip) = self.memory.tip() {
            return Ok(tip);
        }

        self.storage
            .provider_ro()?
            .get_latest_block_number()?
            .map(|(n, h)| NumHash::new(n, h))
            .ok_or(OpProofsStorageError::NoBlocksFound)
    }
}
