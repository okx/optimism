//! Background persistence service for the live trie engine.

#[cfg(feature = "metrics")]
use super::metrics::PersistenceMetrics;
use super::{error::PersistenceError, handle::PersistenceAction};
use crate::{BlockStateDiff, OpProofsStore, api::OpProofsProviderRw, prune::OpProofStoragePruner};
use alloy_eips::eip1898::BlockWithParent;
use crossbeam_channel::{Receiver, Sender};
use reth_provider::BlockHashReader;
use std::{sync::Arc, time::Instant};
use tracing::{debug, error, info};

/// Service that runs in a background thread to persist trie updates.
#[derive(Debug)]
pub struct PersistenceService<H, S> {
    /// Pruner that also owns the storage backend and block hash reader.
    pruner: OpProofStoragePruner<S, H>,
    storage: S,
    incoming: Receiver<PersistenceAction>,

    #[cfg(feature = "metrics")]
    metrics: PersistenceMetrics,
}

impl<H: BlockHashReader, S: OpProofsStore> PersistenceService<H, S> {
    /// Create a new persistence service.
    pub fn new(
        pruner: OpProofStoragePruner<S, H>,
        storage: S,
        incoming: Receiver<PersistenceAction>,
    ) -> Self {
        Self {
            pruner,
            storage,
            incoming,

            #[cfg(feature = "metrics")]
            metrics: PersistenceMetrics::new_with_labels(&[] as &[(&str, &str)]),
        }
    }

    /// Main loop for the service.
    /// Listens for incoming actions and processes them sequentially.
    pub fn run(self) {
        debug!(target: "trie::engine::persistence", "Service started");

        while let Ok(action) = self.incoming.recv() {
            match action {
                PersistenceAction::Unwind(to, reply_tx) => {
                    self.on_unwind(to, reply_tx);
                }
                PersistenceAction::SaveUpdates(updates, reply_tx) => {
                    self.on_save_updates(updates, reply_tx);
                }
            }
        }
        debug!(target: "trie::engine::persistence", "Service shutting down");
    }

    fn on_save_updates(
        &self,
        arc_updates: Vec<Arc<(BlockWithParent, BlockStateDiff)>>,
        reply_tx: Sender<Result<Option<u64>, PersistenceError>>,
    ) {
        if arc_updates.is_empty() {
            let _ = reply_tx.send(Ok(None));
            return;
        }

        let updates: Vec<(BlockWithParent, BlockStateDiff)> = arc_updates
            .into_iter()
            .map(|arc| Arc::try_unwrap(arc).unwrap_or_else(|arc| (*arc).clone()))
            .collect();

        let _ = reply_tx.send(self.try_save_updates(updates));
    }

    fn try_save_updates(
        &self,
        updates: Vec<(BlockWithParent, BlockStateDiff)>,
    ) -> Result<Option<u64>, PersistenceError> {
        let start = Instant::now();
        let count = updates.len();
        let first = updates.first().map(|u| u.0.block.number);
        let last = updates.last().map(|u| u.0.block.number);
        debug!(target: "trie::engine::persistence", ?count, ?first, ?last, "Writing batch to storage");

        let provider_rw_start = Instant::now();
        let provider = self.storage.provider_rw()?;
        let open_tx_duration = provider_rw_start.elapsed();

        let write_start = Instant::now();
        let write_counts = provider.store_trie_updates_batch(updates)?;
        let write_duration = write_start.elapsed();

        let prune_start = Instant::now();
        self.pruner.prune_with_provider(&provider)
            .inspect_err(|e| error!(target: "trie::engine::persistence", ?e, "Pruning failed during save, aborting transaction"))?;
        let prune_duration = prune_start.elapsed();

        let commit_start = Instant::now();
        provider.commit()?;
        let commit_duration = commit_start.elapsed();

        #[cfg(feature = "metrics")]
        {
            self.metrics.increment_write_counts(&write_counts);
            self.metrics.open_tx_duration_seconds.record(open_tx_duration);
            self.metrics.write_duration_seconds.record(write_duration);
            self.metrics.prune_duration_seconds.record(prune_duration);
            self.metrics.commit_duration_seconds.record(commit_duration);
        }

        let duration = start.elapsed();
        info!(
            target: "trie::engine::persistence",
            ?last,
            ?duration,
            ?open_tx_duration,
            ?write_duration,
            ?prune_duration,
            ?commit_duration,
            ?write_counts,
            blocks_count = count,
            "Batch write complete"
        );

        Ok(last)
    }

    fn on_unwind(&self, to: BlockWithParent, reply_tx: Sender<Result<(), PersistenceError>>) {
        let _ = reply_tx.send(self.try_unwind(to));
    }

    fn try_unwind(&self, to: BlockWithParent) -> Result<(), PersistenceError> {
        debug!(target: "trie::engine::persistence", to_block = ?to.block.number, "Unwinding storage");
        let provider = self.storage.provider_rw()?;
        provider.unwind_history(to)?;
        provider.commit()?;
        debug!(target: "trie::engine::persistence", "Unwind successful");
        Ok(())
    }
}
