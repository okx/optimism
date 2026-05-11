//! Support for maintaining the state of the transaction pool

/// Offset before deadline expiry at which a tx becomes "stale" and triggers revalidation.
const OFFSET_TIME: u64 = 60;
/// Maximum number of supervisor requests at the same time
const MAX_SUPERVISOR_QUERIES: usize = 10;

/// Sweep AA-pool expiries with a small grace buffer past `tip_timestamp`
/// so a tx that would expire on the next block-builder slot is dropped
/// before it gets re-broadcast. Mirrors tempo's
/// `EVICTION_BUFFER_SECS` (`crates/transaction-pool/src/maintain.rs:35`)
/// and is paired with the admission-side
/// [`crate::eip8130_xlayer::EXPIRY_ADMISSION_BUFFER_SECS`] so the two
/// horizons match.
pub(crate) const AA_EVICTION_BUFFER_SECS: u64 = 3;

/// Default interval for AA pending-staleness checks (30 minutes). A tx
/// observed pending across two consecutive ticks is assumed stuck and
/// evicted. Mirrors tempo's `DEFAULT_PENDING_STALENESS_INTERVAL`
/// (`crates/transaction-pool/src/maintain.rs:294`).
const AA_PENDING_STALENESS_INTERVAL_SECS: u64 = 30 * 60;

use crate::{
    Eip8130PoolTx, OpDualPool,
    conditional::MaybeConditionalTransaction,
    dual_pool::NotifyAaLifecycle,
    interop::{MaybeInteropTransaction, is_interop_tx, is_stale_interop, is_valid_interop},
    supervisor::SupervisorClient,
    validator::CHECK_ACCESS_LIST_TIMEOUT_SECS,
};
use alloy_consensus::{BlockHeader, conditional::BlockConditionalAttributes};
use alloy_primitives::TxHash;
use futures_util::{FutureExt, Stream, StreamExt, future::BoxFuture};
use metrics::{Gauge, Histogram};
use reth_chain_state::CanonStateNotification;
use reth_metrics::{Metrics, metrics::Counter};
use reth_primitives_traits::NodePrimitives;
use reth_transaction_pool::{PoolTransaction, TransactionPool, error::PoolTransactionError};
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};
use tracing::{debug, info, warn};

/// Transaction pool maintenance metrics
#[derive(Metrics)]
#[metrics(scope = "transaction_pool")]
struct MaintainPoolConditionalMetrics {
    /// Counter indicating the number of conditional transactions removed from
    /// the pool because of exceeded block attributes.
    removed_tx_conditional: Counter,
}

impl MaintainPoolConditionalMetrics {
    #[inline]
    fn inc_removed_tx_conditional(&self, count: usize) {
        self.removed_tx_conditional.increment(count as u64);
    }
}

/// Transaction pool maintenance metrics
#[derive(Metrics)]
#[metrics(scope = "transaction_pool")]
struct MaintainPoolInteropMetrics {
    /// Counter indicating the number of conditional transactions removed from
    /// the pool because of exceeded block attributes.
    removed_tx_interop: Counter,
    /// Number of interop transactions currently in the pool
    pooled_interop_transactions: Gauge,

    /// Counter for interop transactions that became stale and need revalidation
    stale_interop_transactions: Counter,
    // TODO: we also should add metric for (hash, counter) to check number of validation per tx
    /// Histogram for measuring supervisor revalidation duration (congestion metric)
    supervisor_revalidation_duration_seconds: Histogram,
}

impl MaintainPoolInteropMetrics {
    #[inline]
    fn inc_removed_tx_interop(&self, count: usize) {
        self.removed_tx_interop.increment(count as u64);
    }
    #[inline]
    fn set_interop_txs_in_pool(&self, count: usize) {
        self.pooled_interop_transactions.set(count as f64);
    }

    #[inline]
    fn inc_stale_tx_interop(&self, count: usize) {
        self.stale_interop_transactions.increment(count as u64);
    }

    /// Record supervisor revalidation duration
    #[inline]
    fn record_supervisor_duration(&self, duration: std::time::Duration) {
        self.supervisor_revalidation_duration_seconds.record(duration.as_secs_f64());
    }
}

/// Loop-side metrics for [`maintain_eip8130_state`]. Keyed under the
/// AA scope alongside [`crate::Eip8130PoolMetrics`] so dashboards can
/// pivot pool-state and maintenance-loop telemetry off a common prefix.
#[derive(Metrics, Clone)]
#[metrics(scope = "transaction_pool.aa.maintenance")]
struct Eip8130MaintainMetrics {
    /// Wall-clock time spent processing one canonical-commit tick
    /// (`on_bundle_state` + expiry sweep + staleness eviction).
    block_update_duration_seconds: Histogram,
    /// Wall-clock time spent in [`OpDualPool::sweep_aa_expired`].
    expired_eviction_duration_seconds: Histogram,
    /// Wall-clock time spent in the staleness-tracker eviction path.
    staleness_eviction_duration_seconds: Histogram,
    /// Number of AA txs evicted by the staleness tracker (txs pending
    /// across the full
    /// [`AA_PENDING_STALENESS_INTERVAL_SECS`] window).
    stale_pending_evicted: Counter,
}

/// Tracks AA pending tx hashes across snapshots to detect stuck
/// (long-pending) transactions. Mirrors tempo's
/// `PendingStalenessTracker` (`crates/transaction-pool/src/maintain.rs:302-356`).
///
/// Mechanic: every interval, snapshot the set of pending AA tx hashes;
/// any hash present in the prior snapshot AND the new one has been
/// pending across the full interval and is evicted. The previous
/// snapshot retains only "newly seen" hashes so a one-time stuck tx
/// can't be re-evicted on the next tick.
#[derive(Debug)]
pub(crate) struct PendingStalenessTracker {
    previous_pending: HashSet<TxHash>,
    last_snapshot_time: Option<u64>,
    interval_secs: u64,
}

impl PendingStalenessTracker {
    pub(crate) fn new(interval_secs: u64) -> Self {
        Self { previous_pending: HashSet::new(), last_snapshot_time: None, interval_secs }
    }

    /// Returns true if the staleness check interval has elapsed since
    /// the last snapshot (or no snapshot has been taken yet).
    pub(crate) fn should_check(&self, now: u64) -> bool {
        self.last_snapshot_time.is_none_or(|last| now.saturating_sub(last) >= self.interval_secs)
    }

    /// Returns the txs to evict (intersection of previous and current
    /// pending), then advances the snapshot. Stale txs are excluded
    /// from the new snapshot so the next tick starts clean for those
    /// hashes.
    pub(crate) fn check_and_update(
        &mut self,
        current_pending: HashSet<TxHash>,
        now: u64,
    ) -> Vec<TxHash> {
        let stale: Vec<TxHash> =
            self.previous_pending.intersection(&current_pending).copied().collect();
        self.previous_pending =
            current_pending.into_iter().filter(|h| !stale.contains(h)).collect();
        self.last_snapshot_time = Some(now);
        stale
    }
}

impl Default for PendingStalenessTracker {
    fn default() -> Self {
        Self::new(AA_PENDING_STALENESS_INTERVAL_SECS)
    }
}
/// Returns a spawnable future that drains the canonical-state stream and
/// drives [`OpDualPool::on_bundle_state`] for every committed block.
///
/// Loop-closer for the EIP-8130 (XLayer AA) side pool: admission routes
/// AA txs into the side pool via `add_transaction*`, and inclusion /
/// state-driven invalidation flows back here as canonical commits land.
/// Reorgs surface the new tip's `BundleState` exactly the same way;
/// downstream, the pool's `on_state_updates` is signature-symmetric for
/// both directions.
pub fn maintain_eip8130_state_future<N, P, Client, AaV, St>(
    pool: OpDualPool<P, Client, AaV>,
    events: St,
) -> BoxFuture<'static, ()>
where
    N: NodePrimitives,
    P: TransactionPool + Clone + Send + Sync + 'static,
    P::Transaction: Eip8130PoolTx,
    P: NotifyAaLifecycle<<P as TransactionPool>::Transaction>,
    Client: reth_storage_api::StateProviderFactory + Send + Sync + 'static,
    AaV: crate::dual_pool::AaPoolPreValidator<<P as TransactionPool>::Transaction>,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
{
    async move { maintain_eip8130_state(pool, events).await }.boxed()
}

/// Loop body of [`maintain_eip8130_state_future`].
///
/// `OpDualPool::on_bundle_state` already fires `Pending` events for
/// `outcome.promoted` and `Discarded` events for `outcome.mined ∪
/// outcome.invalidated` through the protocol pool's listener fan-out, so
/// downstream consumers (`transaction_event_listener`,
/// `pending_transactions_listener_for`) see AA lifecycle without us
/// doing extra work here. `outcome.demoted` is the only bucket we don't
/// surface — see [`OpDualPool::fire_aa_lifecycle_events`] for the
/// rationale (the tx is still in the pool, just queued).
///
/// Per-tick maintenance: drives `on_bundle_state`, runs
/// [`OpDualPool::sweep_aa_expired`] with a small grace buffer
/// ([`AA_EVICTION_BUFFER_SECS`]) so peer-rejected near-expiry txs don't
/// linger, and drives the staleness tracker on a fixed interval to
/// evict txs that have been pending across a full
/// [`AA_PENDING_STALENESS_INTERVAL_SECS`] window. Removals via the
/// staleness path go through [`OpDualPool::remove_aa_transactions`] so
/// `Discarded` lifecycle events fire on the protocol pool's listener
/// fan-out.
pub async fn maintain_eip8130_state<N, P, Client, AaV, St>(
    pool: OpDualPool<P, Client, AaV>,
    mut events: St,
) where
    N: NodePrimitives,
    P: TransactionPool + Clone + Send + Sync + 'static,
    P::Transaction: Eip8130PoolTx,
    P: NotifyAaLifecycle<<P as TransactionPool>::Transaction>,
    Client: reth_storage_api::StateProviderFactory + Send + Sync + 'static,
    AaV: crate::dual_pool::AaPoolPreValidator<<P as TransactionPool>::Transaction>,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
{
    let metrics = Eip8130MaintainMetrics::default();
    let mut staleness = PendingStalenessTracker::default();
    while let Some(event) = events.next().await {
        let chain = event.committed();
        let block_update_start = Instant::now();
        let bundle = chain.execution_outcome().state();
        let tip = chain.tip();
        let tip_timestamp = tip.timestamp();
        let tip_hash = tip.hash();
        // Forward the canonical tip's block_hash so the AA pool can
        // emit `TransactionEvent::Mined(block_hash)` distinct from
        // `Discarded`.
        let outcome = pool.on_bundle_state(bundle, tip_timestamp, tip_hash);
        let tip_number = tip.number();

        // Sweep with a small grace buffer past the tip timestamp: a tx
        // expiring inside the buffer would be rejected by peers before
        // it could land in another block, so dropping it locally avoids
        // wasted gossip. Mirrors tempo's `EVICTION_BUFFER_SECS`.
        let sweep_horizon = tip_timestamp.saturating_add(AA_EVICTION_BUFFER_SECS);
        let expired_start = Instant::now();
        let expired = pool.sweep_aa_expired(sweep_horizon);
        metrics.expired_eviction_duration_seconds.record(expired_start.elapsed());

        // Staleness eviction: only runs once per interval. The tracker's
        // first observation of a hash never evicts (`previous_pending`
        // empty); the second observation across the interval boundary
        // does.
        let mut stale_evicted_count: usize = 0;
        if staleness.should_check(tip_timestamp) {
            let staleness_start = Instant::now();
            let current_pending = pool.aa_pending_tx_hashes();
            let stale = staleness.check_and_update(current_pending, tip_timestamp);
            if !stale.is_empty() {
                let removed = pool.remove_aa_transactions(&stale);
                stale_evicted_count = removed.len();
                if stale_evicted_count > 0 {
                    metrics.stale_pending_evicted.increment(stale_evicted_count as u64);
                    info!(
                        target: "txpool::aa",
                        block_number = tip_number,
                        count = stale_evicted_count,
                        "evicted stale-pending AA transactions"
                    );
                }
            }
            metrics.staleness_eviction_duration_seconds.record(staleness_start.elapsed());
        }

        if !outcome.mined.is_empty() ||
            !outcome.invalidated.is_empty() ||
            !outcome.promoted.is_empty() ||
            !outcome.demoted.is_empty() ||
            !expired.is_empty() ||
            stale_evicted_count > 0
        {
            debug!(
                target: "txpool::aa",
                block_number = tip_number,
                mined = outcome.mined.len(),
                invalidated = outcome.invalidated.len(),
                promoted = outcome.promoted.len(),
                demoted = outcome.demoted.len(),
                expired = expired.len(),
                stale_pending_evicted = stale_evicted_count,
                "AA pool maintenance step"
            );
        }
        metrics.block_update_duration_seconds.record(block_update_start.elapsed());
    }
}

/// Returns a spawnable future for maintaining the state of the conditional txs in the transaction
/// pool.
pub fn maintain_transaction_pool_conditional_future<N, Pool, St>(
    pool: Pool,
    events: St,
) -> BoxFuture<'static, ()>
where
    N: NodePrimitives,
    Pool: TransactionPool + 'static,
    Pool::Transaction: MaybeConditionalTransaction,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
{
    async move {
        maintain_transaction_pool_conditional(pool, events).await;
    }
    .boxed()
}

/// Maintains the state of the conditional tx in the transaction pool by handling new blocks and
/// reorgs.
///
/// This listens for any new blocks and reorgs and updates the conditional txs in the
/// transaction pool's state accordingly
pub async fn maintain_transaction_pool_conditional<N, Pool, St>(pool: Pool, mut events: St)
where
    N: NodePrimitives,
    Pool: TransactionPool,
    Pool::Transaction: MaybeConditionalTransaction,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
{
    let metrics = MaintainPoolConditionalMetrics::default();
    loop {
        let Some(event) = events.next().await else { break };
        if let CanonStateNotification::Commit { new } = event {
            let block_attr = BlockConditionalAttributes {
                number: new.tip().number(),
                timestamp: new.tip().timestamp(),
            };
            let mut to_remove = Vec::new();
            for tx in &pool.pooled_transactions() {
                if tx.transaction.has_exceeded_block_attributes(&block_attr) {
                    to_remove.push(*tx.hash());
                }
            }
            if !to_remove.is_empty() {
                let removed = pool.remove_transactions(to_remove);
                metrics.inc_removed_tx_conditional(removed.len());
            }
        }
    }
}

/// Returns a spawnable future for maintaining the state of the interop tx in the transaction pool.
pub fn maintain_transaction_pool_interop_future<N, Pool, St>(
    pool: Pool,
    events: St,
    supervisor_client: SupervisorClient,
) -> BoxFuture<'static, ()>
where
    N: NodePrimitives,
    Pool: TransactionPool + 'static,
    Pool::Transaction: MaybeInteropTransaction,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
{
    async move {
        maintain_transaction_pool_interop(pool, events, supervisor_client).await;
    }
    .boxed()
}

/// Maintains the state of the interop tx in the transaction pool by handling new blocks and reorgs.
///
/// This listens for any new blocks and reorgs and updates the interop tx in the transaction pool's
/// state accordingly
pub async fn maintain_transaction_pool_interop<N, Pool, St>(
    pool: Pool,
    mut events: St,
    supervisor_client: SupervisorClient,
) where
    N: NodePrimitives,
    Pool: TransactionPool,
    Pool::Transaction: MaybeInteropTransaction,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
{
    let metrics = MaintainPoolInteropMetrics::default();

    loop {
        let Some(event) = events.next().await else { break };
        if let CanonStateNotification::Commit { new } = event {
            let timestamp = new.tip().timestamp();
            let mut to_remove = Vec::new();
            let mut to_revalidate = Vec::new();
            let mut interop_count = 0;

            // If failsafe is active, evict ALL interop txs and skip revalidation.
            // Belt-and-suspenders with poll_failsafe: catches any tx that raced past
            // the ingress check or was added between poll_failsafe transition ticks.
            if supervisor_client.is_failsafe_enabled() {
                let interop_hashes: Vec<_> = pool
                    .pooled_transactions()
                    .iter()
                    .filter(|tx| is_interop_tx(&tx.transaction))
                    .map(|tx| *tx.hash())
                    .collect();
                if !interop_hashes.is_empty() {
                    info!(
                        target: "txpool::interop",
                        count = interop_hashes.len(),
                        "failsafe active on block event: evicting all interop transactions"
                    );
                    let removed = pool.remove_transactions(interop_hashes);
                    metrics.inc_removed_tx_interop(removed.len());
                }
                continue;
            }

            // scan all pooled interop transactions
            for pooled_tx in pool.pooled_transactions() {
                if let Some(interop_deadline_val) = pooled_tx.transaction.interop_deadline() {
                    interop_count += 1;
                    if !is_valid_interop(interop_deadline_val, timestamp) {
                        to_remove.push(*pooled_tx.transaction.hash());
                    } else if is_stale_interop(interop_deadline_val, timestamp, OFFSET_TIME) {
                        to_revalidate.push(pooled_tx.transaction.clone());
                    }
                }
            }

            metrics.set_interop_txs_in_pool(interop_count);

            if !to_revalidate.is_empty() {
                metrics.inc_stale_tx_interop(to_revalidate.len());

                let revalidation_start = Instant::now();
                let revalidation_stream = supervisor_client.revalidate_interop_txs_stream(
                    to_revalidate,
                    timestamp,
                    CHECK_ACCESS_LIST_TIMEOUT_SECS,
                    MAX_SUPERVISOR_QUERIES,
                );

                futures_util::pin_mut!(revalidation_stream);

                while let Some((tx_item_from_stream, validation_result)) =
                    revalidation_stream.next().await
                {
                    match validation_result {
                        Some(Ok(())) => {
                            tx_item_from_stream
                                .set_interop_deadline(timestamp + CHECK_ACCESS_LIST_TIMEOUT_SECS);
                        }
                        Some(Err(err)) => {
                            if err.is_bad_transaction() {
                                to_remove.push(*tx_item_from_stream.hash());
                            }
                        }
                        None => {
                            warn!(
                                target: "txpool",
                                hash = %tx_item_from_stream.hash(),
                                "Interop transaction no longer considered cross-chain during revalidation; removing."
                            );
                            to_remove.push(*tx_item_from_stream.hash());
                        }
                    }
                }

                metrics.record_supervisor_duration(revalidation_start.elapsed());
            }

            if !to_remove.is_empty() {
                let removed = pool.remove_transactions(to_remove);
                metrics.inc_removed_tx_interop(removed.len());
            }
        }
    }
}

/// Background task that polls the supervisor for failsafe state every second.
/// When failsafe transitions from disabled to enabled, evicts all interop txs
/// from the pool immediately (does not wait for the next block event).
/// Matches op-geth's `startBackgroundInteropFailsafeDetection` (miner/miner.go:140-165).
pub async fn poll_failsafe<Pool>(supervisor_client: SupervisorClient, pool: Pool)
where
    Pool: TransactionPool,
    Pool::Transaction: MaybeInteropTransaction,
{
    let metrics = MaintainPoolInteropMetrics::default();
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    let mut was_enabled = false;
    loop {
        interval.tick().await;
        match supervisor_client.query_failsafe().await {
            Ok(enabled) => {
                // On transition to enabled: evict all interop txs immediately
                if enabled && !was_enabled {
                    let interop_hashes: Vec<_> = pool
                        .pooled_transactions()
                        .iter()
                        .filter(|tx| is_interop_tx(&tx.transaction))
                        .map(|tx| *tx.hash())
                        .collect();
                    if !interop_hashes.is_empty() {
                        info!(
                            target: "txpool::interop",
                            count = interop_hashes.len(),
                            "failsafe enabled: evicting all interop transactions"
                        );
                        let removed = pool.remove_transactions(interop_hashes);
                        metrics.inc_removed_tx_interop(removed.len());
                    }
                }
                was_enabled = enabled;
            }
            Err(err) => {
                warn!(
                    target: "txpool::interop",
                    %err,
                    "failed to query failsafe state"
                );
            }
        }
    }
}

/// Creates a boxed future for the failsafe polling task.
pub fn poll_failsafe_future<Pool>(
    supervisor_client: SupervisorClient,
    pool: Pool,
) -> BoxFuture<'static, ()>
where
    Pool: TransactionPool + 'static,
    Pool::Transaction: MaybeInteropTransaction,
{
    Box::pin(poll_failsafe(supervisor_client, pool))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::TxHash;

    /// First snapshot must never evict — there is no prior snapshot for
    /// the intersection. Reproduces tempo's
    /// `no_eviction_on_first_snapshot` invariant.
    #[test]
    fn staleness_tracker_does_not_evict_first_seen() {
        let mut tracker = PendingStalenessTracker::new(100);
        let tx = TxHash::random();
        let stale = tracker.check_and_update(std::iter::once(tx).collect(), 0);
        assert!(stale.is_empty(), "first snapshot must yield zero stale txs");
        assert!(tracker.previous_pending.contains(&tx), "tx must be carried into next snapshot");
    }

    /// A hash present across two consecutive snapshots is evicted on the
    /// second snapshot. The newly-seen hash on the second snapshot is
    /// retained for the next round.
    #[test]
    fn staleness_tracker_evicts_persistent_pending() {
        let mut tracker = PendingStalenessTracker::new(100);
        let stuck = TxHash::random();
        let new = TxHash::random();

        tracker.check_and_update(std::iter::once(stuck).collect(), 0);
        let stale = tracker.check_and_update([stuck, new].into_iter().collect(), 100);

        assert_eq!(stale.len(), 1);
        assert!(stale.contains(&stuck));
        assert!(tracker.previous_pending.contains(&new), "newly-seen tx tracked for next round");
        assert!(
            !tracker.previous_pending.contains(&stuck),
            "evicted tx must not re-fire on the next round"
        );
    }

    /// A hash present on round 1 but absent on round 2 is forgotten —
    /// not evicted, not retained. A future re-appearance only evicts
    /// after surviving its own full interval.
    #[test]
    fn staleness_tracker_does_not_evict_after_disappearance() {
        let mut tracker = PendingStalenessTracker::new(100);
        let x = TxHash::random();

        tracker.check_and_update(std::iter::once(x).collect(), 0);
        // Round 2: x is gone (mined / replaced / reorged out).
        let stale = tracker.check_and_update(HashSet::new(), 100);
        assert!(stale.is_empty(), "absent tx must not be evicted");
        assert!(tracker.previous_pending.is_empty(), "tracker forgets the absent tx");

        // Round 3: x reappears. Still must not evict — this is a fresh
        // observation from the tracker's perspective.
        let stale = tracker.check_and_update(std::iter::once(x).collect(), 200);
        assert!(stale.is_empty(), "reappearance after a gap is not stale");
    }

    /// `should_check` gates the snapshot interval. Within the interval
    /// it returns false so we don't burn the snapshot budget on every
    /// block.
    #[test]
    fn staleness_tracker_should_check_respects_interval() {
        let mut tracker = PendingStalenessTracker::new(100);
        assert!(tracker.should_check(0), "first call always returns true");
        tracker.check_and_update(HashSet::new(), 0);
        assert!(!tracker.should_check(50));
        assert!(tracker.should_check(100), "interval boundary is inclusive");
    }

    /// Pin the eviction-buffer constant value so a future tempo bump
    /// that drifts the constant is visibly noisy in code review.
    #[test]
    fn eviction_buffer_matches_tempo() {
        // tempo `crates/transaction-pool/src/maintain.rs:35`
        assert_eq!(AA_EVICTION_BUFFER_SECS, 3);
    }

    /// Pin the staleness interval default (30 minutes).
    #[test]
    fn staleness_interval_matches_tempo() {
        // tempo `crates/transaction-pool/src/maintain.rs:294`
        assert_eq!(AA_PENDING_STALENESS_INTERVAL_SECS, 30 * 60);
    }
}
