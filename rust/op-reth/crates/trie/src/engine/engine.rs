//! [`Engine`] — the thin event-loop dispatcher.

use super::state::EngineState as State;
use super::EngineAction;
use crate::{OpProofStoragePruner, OpProofsStore};
use crossbeam_channel::{Receiver, TryRecvError};
use reth_evm::ConfigureEvm;
use reth_primitives_traits::BlockTy;
use reth_provider::{
    BlockHashReader, BlockReader, DatabaseProviderFactory, StateProviderFactory, StateReader,
    TransactionVariant,
};
use tracing::{debug, error, info};

/// Number of blocks to process per sync catch-up batch before re-checking for new actions.
const SYNC_BLOCKS_BATCH_SIZE: u64 = 5;

/// The engine that runs on a dedicated thread, dispatching [`EngineAction`]
/// messages to self-contained task structs that operate on [`EngineState`].
#[allow(missing_debug_implementations)]
pub(super) struct Engine<Evm, Provider, Store>
where
    Evm: ConfigureEvm,
    Provider: StateReader + DatabaseProviderFactory + StateProviderFactory + BlockReader,
{
    state: State<Evm, Provider, Store>,
    incoming: Receiver<EngineAction<BlockTy<Evm::Primitives>>>,
}

impl<Evm, Provider, Store> Engine<Evm, Provider, Store>
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
    pub(super) fn new(
        evm_config: Evm,
        provider: Provider,
        storage: Store,
        pruner: OpProofStoragePruner<Store, Provider>,
        incoming: Receiver<EngineAction<BlockTy<Evm::Primitives>>>,
    ) -> Self {
        Self { state: State::new(evm_config, provider, storage, pruner), incoming }
    }

    pub(super) fn with_persistence_threshold(mut self, threshold: u64) -> Self {
        self.state = self.state.with_persistence_threshold(threshold);
        self
    }

    pub(super) fn with_backpressure_threshold(mut self, threshold: u64) -> Self {
        self.state = self.state.with_backpressure_threshold(threshold);
        self
    }

    /// Dispatch one action to the appropriate task and advance persistence.
    fn dispatch_action(&mut self, action: EngineAction<BlockTy<Evm::Primitives>>) {
        action.execute(&mut self.state);
        if let Err(e) = self.state.advance_persistence() {
            error!(target: "live-trie::engine", ?e, "Persistence error after action");
        }
    }

    /// Returns `true` if the engine is behind its sync target.
    fn needs_sync(&self) -> bool {
        let current_tip = self.state.get_tip().map(|t| t.number).unwrap_or(0);
        self.state.sync_target > current_tip
    }

    /// Execute one batch of sync catch-up blocks if behind the sync target.
    ///
    /// Called by the event loop after every action check so sync work is
    /// naturally interleaved with incoming actions.
    fn try_sync_step(&mut self) {
        let current_tip = match self.state.get_tip() {
            Ok(tip) => tip.number,
            Err(e) => {
                error!(target: "live-trie::engine", ?e, "Failed to get tip during sync");
                return;
            }
        };

        if self.state.sync_target <= current_tip {
            return;
        }

        let end = (current_tip + SYNC_BLOCKS_BATCH_SIZE).min(self.state.sync_target);
        info!(
            target: "live-trie::engine",
            start = current_tip + 1,
            end,
            "Processing sync catch-up batch"
        );

        for block_num in (current_tip + 1)..=end {
            let block = match self
                .state
                .provider
                .recovered_block(block_num.into(), TransactionVariant::NoHash)
            {
                Ok(Some(b)) => b,
                Ok(None) => {
                    error!(target: "live-trie::engine", block_num, "Block not found during sync");
                    return;
                }
                Err(e) => {
                    error!(target: "live-trie::engine", ?e, block_num, "Provider error during sync");
                    return;
                }
            };

            if let Err(e) = super::tasks::execute_block(&block, &mut self.state) {
                error!(target: "live-trie::engine", ?e, block_num, "Block execution failed during sync");
                return;
            }

            if let Err(e) = self.state.advance_persistence() {
                error!(target: "live-trie::engine", ?e, "Persistence error during sync");
                return;
            }
        }
    }

    /// Runs the main loop of the engine, processing incoming actions.
    ///
    /// When behind the sync target, actions are checked with a non-blocking
    /// `try_recv` so sync batches run back-to-back with only an action drain
    /// between them. When caught up, the engine blocks on `recv` — the sync
    /// target is only updated by incoming actions, so there is no reason to
    /// wake up on a timer.
    pub(super) fn run(mut self) {
        debug_assert!(
            self.state.persistence.threshold < self.state.persistence.backpressure_threshold,
            "backpressure_threshold ({}) must be greater than persistence_threshold ({})",
            self.state.persistence.backpressure_threshold,
            self.state.persistence.threshold,
        );
        debug!(target: "live-trie::engine", "Collector engine started");

        loop {
            if self.needs_sync() {
                match self.incoming.try_recv() {
                    Ok(action) => self.dispatch_action(action),
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => break,
                }
            } else {
                match self.incoming.recv() {
                    Ok(action) => self.dispatch_action(action),
                    Err(_) => break,
                }
            }
            self.try_sync_step();
        }

        debug!(target: "live-trie::engine", "Collector engine shutting down, draining in-flight persist");
        self.state.drain_persistence();
        debug!(target: "live-trie::engine", "Collector engine stopped");
    }
}
