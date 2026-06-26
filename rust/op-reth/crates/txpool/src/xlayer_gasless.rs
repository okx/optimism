//! Gasless transaction support for the OP transaction pool.
//!
//! When gasless transactions are enabled, zero-priced transactions (legacy `gas_price == 0`, or
//! EIP-1559 `max_fee_per_gas == 0 && max_priority_fee_per_gas == 0`) are accepted into the pool
//! (see the pool's `minimal_protocol_basefee` override in the node builder) and assigned a *mock*
//! tip for ordering, so they compete with normal transactions by (tip, then enqueue
//! time). This module provides:
//!
//! - [`GaslessMockTip`]: the shared mock tip.
//! - [`XLayerGaslessOrdering`]: a [`TransactionOrdering`] that assigns the mock tip to gasless txs
//!   and otherwise behaves exactly like
//!   [`CoinbaseTipOrdering`](reth_transaction_pool::CoinbaseTipOrdering).
//! - [`maintain_gasless_mock_tip`]: a maintenance task that, on every new canonical block, records
//!   gasless observability metrics and recomputes the mock tip as a configured percentile of the
//!   block's transaction tips.
//!
//! The (tip, enqueue time) total order is completed by the pending sub-pool's existing
//! `submission_id` tiebreak (priority first, earlier submission second) — see
//! `reth_transaction_pool`'s `PendingTransaction` ordering — so no change to that logic is needed.

use alloy_consensus::{BlockHeader, Transaction, TxReceipt, Typed2718};
use futures_util::{Stream, StreamExt};
use metrics::{Counter, Gauge, Histogram};
use op_alloy_consensus::DEPOSIT_TX_TYPE_ID;
use reth_chain_state::CanonStateNotification;
use reth_metrics::Metrics;
use reth_primitives_traits::{BlockBody, NodePrimitives};
use reth_transaction_pool::{PoolTransaction, Priority, TransactionOrdering, TransactionPoolExt};
use std::{
    fmt,
    marker::PhantomData,
    sync::{
        atomic::{AtomicU64, Ordering as AtomicOrdering},
        Arc,
    },
    time::{Duration, Instant},
};
use tracing::{debug, warn};

/// Default maximum time a gasless (zero-priced) transaction may sit in the *pending* sub-pool
/// before the gasless maintenance task evicts it. Overridable via the `--rollup.gasless-pending-
/// lifetime` CLI flag (plumbed through [`maintain_gasless_mock_tip`]'s `pending_lifetime`).
///
/// Under normal load any pending tx is included within seconds (pending-pool cap / chain TPS — e.g.
/// ~50k cap at ~2k TPS drains in ~25s), so a gasless tx still pending after this long is
/// effectively stale.
pub const GASLESS_DEFAULT_PENDING_MAX_LIFETIME: Duration = Duration::from_secs(10 * 60);

/// Shared mock tip (in wei) assigned to gasless (zero-priced) transactions for pool ordering.
///
/// Updated on every new canonical block by [`maintain_gasless_mock_tip`] and read by
/// [`XLayerGaslessOrdering::priority`]. Cheap to clone (`Arc`-backed).
#[derive(Clone, Debug, Default)]
pub struct GaslessMockTip(Arc<AtomicU64>);

impl GaslessMockTip {
    /// Creates a new shared mock tip initialized to `initial` wei.
    #[inline]
    pub fn new(initial: u64) -> Self {
        Self(Arc::new(AtomicU64::new(initial)))
    }

    /// Returns the current mock tip (wei).
    #[inline]
    pub fn get(&self) -> u64 {
        self.0.load(AtomicOrdering::Relaxed)
    }

    /// Sets the mock tip (wei).
    #[inline]
    pub fn set(&self, price: u64) {
        self.0.store(price, AtomicOrdering::Relaxed);
    }
}

/// Returns the index into an ascending-sorted slice of length `len` for `percentile` in `[0.0,
/// 1.0]`.
fn percentile_index(len: usize, percentile: f64) -> usize {
    if len == 0 {
        return 0;
    }
    let p = percentile.clamp(0.0, 1.0);
    ((p * len as f64).floor() as usize).min(len - 1)
}

/// Returns the `percentile` (in `[0.0, 1.0]`) gas price of `prices`, or `None` if empty.
///
/// Sorts ascending and picks the element at `floor(percentile * len)`, clamped to the last index.
/// For example, with prices `[0, 1, .., 9]`: `0.1` → `1`, `0.9` → `9`.
pub fn percentile_gas_price(mut prices: Vec<u128>, percentile: f64) -> Option<u128> {
    if prices.is_empty() {
        return None;
    }
    prices.sort_unstable();
    Some(prices[percentile_index(prices.len(), percentile)])
}

/// Returns the percentile across `prices` with zero-priced entries excluded.
///
/// Gasless txs land in canonical blocks with `effective_gas_price == 0`. If they were left in the
/// sample, even a small share would drag `mock_tip` toward 0 at low percentiles, creating a
/// positive-feedback trap (`mock_tip=0` → all gasless ordered at priority 0 → next block samples
/// 0 again). Returns `None` when no paid tx remains so the caller keeps the previous mock tip.
fn percentile_paid_gas_price(prices: Vec<u128>, percentile: f64) -> Option<u128> {
    let paid: Vec<u128> = prices.into_iter().filter(|p| *p > 0).collect();
    percentile_gas_price(paid, percentile)
}

/// Transaction ordering that assigns a configurable mock tip to zero-priced ("gasless")
/// transactions.
///
/// Gasless txs are ordered by the shared [`GaslessMockTip`]; every other transaction is ordered
/// exactly like [`CoinbaseTipOrdering`](reth_transaction_pool::CoinbaseTipOrdering)
/// (`effective_tip_per_gas`), so the rest of the pool ordering is unchanged.
pub struct XLayerGaslessOrdering<T> {
    mock_tip: GaslessMockTip,
    // `T` is bound to `TransactionOrdering::Transaction` in the impl below; no field holds it.
    _pd: PhantomData<T>,
}

impl<T> XLayerGaslessOrdering<T> {
    /// Creates a new ordering backed by the shared `mock_tip`.
    pub const fn new(mock_tip: GaslessMockTip) -> Self {
        Self { mock_tip, _pd: PhantomData }
    }
}

impl<T> Clone for XLayerGaslessOrdering<T> {
    fn clone(&self) -> Self {
        Self { mock_tip: self.mock_tip.clone(), _pd: PhantomData }
    }
}

impl<T> Default for XLayerGaslessOrdering<T> {
    fn default() -> Self {
        Self::new(GaslessMockTip::default())
    }
}

impl<T> fmt::Debug for XLayerGaslessOrdering<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("XLayerGaslessOrdering").field("mock_tip", &self.mock_tip).finish()
    }
}

impl<T> TransactionOrdering for XLayerGaslessOrdering<T>
where
    T: PoolTransaction + 'static,
{
    type PriorityValue = u128;
    type Transaction = T;

    #[inline]
    fn priority(&self, transaction: &Self::Transaction, base_fee: u64) -> Priority<u128> {
        // A zero-priced (gasless) tx is assigned the configured mock tip so it competes with
        // normal txs by tip; the (tip, enqueue time) total order is completed by the
        // pending pool's `submission_id` tiebreak. `max_fee_per_gas() == 0` covers both a legacy
        // `gas_price == 0` and a 1559 `max_fee == 0 && max_priority == 0`.
        if transaction.max_fee_per_gas() == 0 {
            // `mock_tip` is maintained on the tip scale, so use it directly.
            return Priority::Value(u128::from(self.mock_tip.get()));
        }
        // Identical to `CoinbaseTipOrdering`.
        transaction.effective_tip_per_gas(base_fee).into()
    }
}

/// Metrics recorded for each new canonical block's transactions.
#[derive(Metrics, Clone)]
#[metrics(scope = "optimism_transaction_pool.gasless")]
pub(crate) struct GaslessBlockMetrics {
    /// Effective gas price (wei) of each transaction in the latest canonical block.
    block_tx_gas_price: Histogram,
    /// Current mock tip (wei) the gasless ordering assigns to zero-priced txs.
    mock_tip_wei: Gauge,
    /// Number of gasless (zero effective-gas-price) transactions per canonical block.
    ///
    /// A histogram, not a gauge: blocks arrive faster than Prometheus scrapes, so a gauge would
    /// only sample one block per scrape interval and drop the rest.
    gasless_txs_per_block: Histogram,
    /// Gas used by gasless transactions per canonical block, in millions of gas (Mgas) — i.e. how
    /// much block space gasless traffic occupies. Histogram for the same reason as above.
    gasless_mgas_per_block: Histogram,
    /// Number of gasless transactions currently sitting in the pending sub-pool.
    pending_pool_gasless_transactions: Gauge,
    /// Number of zero-priced txs rejected at admission because the on-chain gasless contract did
    /// not whitelist them.
    pub(crate) gasless_rejected_not_whitelisted: Counter,
    /// Number of zero-priced txs rejected at admission because their gas limit exceeded the
    /// contract's per-tx allowance.
    pub(crate) gasless_rejected_gas_limit_exceeded: Counter,
}

/// Evicts gasless transactions that have sat in the pending sub-pool longer than `max_lifetime`,
/// returning how many gasless txs were in pending *before* eviction (for the metric).
fn evict_stale_pending_gasless<P>(pool: &P, now: Instant, max_lifetime: Duration) -> usize
where
    P: TransactionPoolExt,
{
    let pending_gasless =
        pool.get_pending_transactions_with_predicate(|tx| tx.transaction.max_fee_per_gas() == 0);
    let total = pending_gasless.len();
    let stale: Vec<_> = pending_gasless
        .into_iter()
        .filter(|tx| now.duration_since(tx.timestamp) > max_lifetime)
        .map(|tx| *tx.hash())
        .collect();
    if !stale.is_empty() {
        // `remove_transactions` drops them from the pool entirely; they can be re-submitted if they
        // become valid again.
        debug!(
            target: "txpool::gasless",
            evicted = stale.len(),
            pending_before = total,
            ?max_lifetime,
            "evicting stale pending gasless transactions",
        );
        pool.remove_transactions(stale);
    }
    total
}

/// Maintenance task: on each new canonical block, record gasless observability metrics, update
/// `mock_tip`, and evict stale pending gasless txs.
///
/// `mock_tip` is set to the configured `percentile` of the block's paid transaction *tips*
/// (`effective_tip_per_gas`, ascending) — the same scale the gasless ordering competes on (see
/// [`XLayerGaslessOrdering::priority`]). Zero-tip txs (gasless, and base-fee-only txs) are excluded
/// from the sample so they don't drag `mock_tip` toward 0 (see [`percentile_paid_gas_price`]).
/// Empty blocks, or blocks with no paid-tip txs, leave the previous mock tip unchanged.
///
/// It also evicts gasless txs that have sat in the pending sub-pool longer than `pending_lifetime`
/// (the generic stale sweep only scans the queued sub-pool).
pub async fn maintain_gasless_mock_tip<N, St, P>(
    mock_tip: GaslessMockTip,
    percentile: f64,
    pending_lifetime: Duration,
    pool: P,
    mut events: St,
) where
    N: NodePrimitives,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
    P: TransactionPoolExt,
{
    let metrics = GaslessBlockMetrics::default();
    loop {
        let Some(event) = events.next().await else {
            warn!(
                target: "txpool::gasless",
                "canonical state stream closed; gasless mock-tip maintenance task exiting",
            );
            break;
        };
        if let CanonStateNotification::Commit { new } = event {
            let block = new.tip().sealed_block();
            let base_fee = block.header().base_fee_per_gas();
            let base_fee_u64 = base_fee.unwrap_or(0);
            // Receipts are index-aligned with the block body's transactions; we diff successive
            // `cumulative_gas_used` to recover each tx's own gas used (for the Mgas metric).
            let receipts = new.execution_outcome().receipts_by_block(block.header().number());

            // Sample paid-tx *tips* (not full gas prices): `mock_tip` is maintained on the tip
            // scale so the gasless ordering can use it directly (see `priority`).
            let mut tips: Vec<u128> = Vec::with_capacity(block.body().transactions().len());
            let mut gasless_txs: u64 = 0;
            let mut gasless_gas_used: u64 = 0;
            let mut prev_cumulative: u64 = 0;
            for (idx, tx) in block.body().transactions().iter().enumerate() {
                let price = tx.effective_gas_price(base_fee);
                metrics.block_tx_gas_price.record(price as f64);
                // Gasless and base-fee-only txs yield a 0 tip and are dropped by the percentile.
                tips.push(tx.effective_tip_per_gas(base_fee_u64).unwrap_or(0));

                let cumulative =
                    receipts.get(idx).map(|r| r.cumulative_gas_used()).unwrap_or(prev_cumulative);
                let gas_used = cumulative.saturating_sub(prev_cumulative);
                prev_cumulative = cumulative;

                // Gasless txs land in canonical blocks with `effective_gas_price == 0`. Deposit
                // (system/L1) txs are also zero-priced but
                // are not gasless pool txs, so exclude them.
                let is_deposit = tx.ty() == DEPOSIT_TX_TYPE_ID;
                if price == 0 && !is_deposit {
                    gasless_txs += 1;
                    gasless_gas_used = gasless_gas_used.saturating_add(gas_used);
                }
            }
            metrics.gasless_txs_per_block.record(gasless_txs as f64);
            metrics.gasless_mgas_per_block.record(gasless_gas_used as f64 / 1_000_000.0);

            if let Some(tip) = percentile_paid_gas_price(tips, percentile) {
                // Clamp into u64 (wei mock tip); real tips never approach u64::MAX.
                mock_tip.set(tip.min(u128::from(u64::MAX)) as u64);
            }
            metrics.mock_tip_wei.set(mock_tip.get() as f64);

            // Snapshot how many gasless txs are in the pending sub-pool, and evict any that have
            // lingered past the max lifetime (stale, e.g. de-whitelisted).
            let pending_gasless =
                evict_stale_pending_gasless(&pool, Instant::now(), pending_lifetime);
            metrics.pending_pool_gasless_transactions.set(pending_gasless as f64);
        }
    }
}

#[cfg(test)]
mod xlayer_test {
    use super::*;

    #[test]
    fn percentile_examples() {
        let prices: Vec<u128> = (0..10).collect(); // [0, 1, .., 9], len 10
        assert_eq!(percentile_gas_price(prices.clone(), 0.1), Some(1)); // idx floor(1.0) = 1
        assert_eq!(percentile_gas_price(prices.clone(), 0.9), Some(9)); // idx floor(9.0) = 9
        assert_eq!(percentile_gas_price(prices.clone(), 0.0), Some(0));
        assert_eq!(percentile_gas_price(prices, 1.0), Some(9)); // clamped to last index
        assert_eq!(percentile_gas_price(vec![], 0.5), None);
    }

    #[test]
    fn percentile_unsorted_input() {
        assert_eq!(percentile_gas_price(vec![9, 1, 5, 3, 7], 0.5), Some(5));
    }

    #[test]
    fn percentile_clamps_out_of_range() {
        assert_eq!(percentile_gas_price(vec![10, 20, 30], 2.0), Some(30));
        assert_eq!(percentile_gas_price(vec![10, 20, 30], -1.0), Some(10));
    }

    #[test]
    fn percentile_paid_excludes_zero_priced_gasless() {
        // Mixed block: 4 gasless (0) + 3 paid (10, 20, 30). With the raw sample a low percentile
        // would hit 0; after excluding gasless the sample is [10, 20, 30] and the percentile
        // reflects the real fee market.
        let mixed = vec![0u128, 0, 0, 0, 10, 20, 30];
        assert_eq!(percentile_paid_gas_price(mixed.clone(), 0.0), Some(10));
        assert_eq!(percentile_paid_gas_price(mixed.clone(), 0.5), Some(20));
        assert_eq!(percentile_paid_gas_price(mixed, 1.0), Some(30));

        // All-gasless block: nothing to sample; `None` tells the caller to keep the prior price.
        assert_eq!(percentile_paid_gas_price(vec![0, 0, 0], 0.5), None);

        // No gasless: identical to `percentile_gas_price`.
        assert_eq!(percentile_paid_gas_price(vec![10, 20, 30], 0.5), Some(20));

        // Sanity-check the regression that motivated this filter: without the filter, a low
        // percentile on a mostly-gasless block would return 0.
        assert_eq!(percentile_gas_price(vec![0, 0, 0, 0, 10, 20, 30], 0.1), Some(0));
    }

    #[test]
    fn mock_tip_get_set() {
        let p = GaslessMockTip::new(7);
        assert_eq!(p.get(), 7);
        let q = p.clone();
        q.set(42);
        assert_eq!(p.get(), 42); // shared
    }

    #[test]
    fn ordering_assigns_mock_tip_to_gasless_tx() {
        use reth_transaction_pool::test_utils::MockTransaction;

        let mock = GaslessMockTip::new(500);
        let ordering = XLayerGaslessOrdering::<MockTransaction>::new(mock.clone());

        // Zero-priced (gasless) tx -> the configured mock tip.
        let gasless_tx = MockTransaction::legacy().with_gas_price(0);
        assert_eq!(ordering.priority(&gasless_tx, 0), Priority::Value(500));

        // Normal tx -> effective tip, identical to `CoinbaseTipOrdering` (base_fee 0 => tip ==
        // price).
        let normal_tx = MockTransaction::legacy().with_gas_price(100);
        assert_eq!(ordering.priority(&normal_tx, 0), Priority::Value(100));

        // Mock-price updates are reflected immediately (shared state).
        mock.set(777);
        assert_eq!(ordering.priority(&gasless_tx, 0), Priority::Value(777));
    }

    // Pool-mechanics for gasless (`PoolConfig::allow_gasless`, served by the forked reth pool):
    // a zero fee-cap tx is admitted to the *pending* sub-pool and yielded by the best iterator even
    // when the block base fee is > 0, and it stays pending across a base-fee rise. This is what
    // lets whitelisted 0-price txs be built on a chain whose base fee never reaches 0. Black-box:
    // base fee is set via the public `set_block_info`, membership via
    // `pending/queued_transactions`.
    #[tokio::test]
    async fn gasless_zero_price_is_pending_and_best_with_nonzero_basefee() {
        use alloy_consensus::Transaction;
        use reth_transaction_pool::{
            test_utils::{MockTransaction, TestPool, TestPoolBuilder},
            BestTransactionsAttributes, BlockInfo, PoolConfig, TransactionOrigin, TransactionPool,
            TransactionPoolExt,
        };

        let pool: TestPool = TestPoolBuilder::default()
            .with_config(PoolConfig { allow_gasless: true, ..Default::default() })
            .into();
        let base_fee = 100u64;
        pool.set_block_info(BlockInfo {
            pending_basefee: base_fee,
            block_gas_limit: 30_000_000,
            ..Default::default()
        });

        let tx = MockTransaction::eip1559().with_max_fee(0).with_priority_fee(0);
        assert_eq!(tx.max_fee_per_gas(), 0);
        pool.add_transaction(TransactionOrigin::Local, tx).await.unwrap();

        // (1) insert-time classification: pending despite base_fee > 0
        assert_eq!(pool.pending_transactions().len(), 1, "gasless tx should be pending");
        assert!(pool.queued_transactions().is_empty(), "gasless tx must not be parked");

        // (2) yielded by the best iterator at the tracked base fee
        assert_eq!(
            pool.best_transactions_with_attributes(BestTransactionsAttributes::new(base_fee, None))
                .count(),
            1,
            "gasless tx should be yielded at base_fee > 0",
        );

        // (3) yielded even when a higher base fee is requested (WithFees filter relaxation)
        assert_eq!(
            pool.best_transactions_with_attributes(BestTransactionsAttributes::new(
                base_fee * 5,
                None
            ))
            .count(),
            1,
            "gasless tx should pass the WithFees filter",
        );

        // (4) a base-fee rise must not demote it out of pending
        pool.set_block_info(BlockInfo {
            pending_basefee: base_fee * 5,
            block_gas_limit: 30_000_000,
            ..Default::default()
        });
        assert_eq!(
            pool.pending_transactions().len(),
            1,
            "gasless tx should stay pending after a base-fee rise",
        );
    }

    // Control: without `allow_gasless`, the same 0-price tx (admitted only because the protocol
    // floor was lowered to 0 here) is parked in the basefee sub-pool and never yielded — proving
    // the flag is what changes the behavior.
    #[tokio::test]
    async fn gasless_disabled_zero_price_is_parked_and_not_best() {
        use reth_transaction_pool::{
            test_utils::{MockTransaction, TestPool, TestPoolBuilder},
            BestTransactionsAttributes, BlockInfo, PoolConfig, TransactionOrigin, TransactionPool,
            TransactionPoolExt,
        };

        let pool: TestPool = TestPoolBuilder::default()
            .with_config(PoolConfig {
                allow_gasless: false,
                minimal_protocol_basefee: 0,
                ..Default::default()
            })
            .into();
        let base_fee = 100u64;
        pool.set_block_info(BlockInfo {
            pending_basefee: base_fee,
            block_gas_limit: 30_000_000,
            ..Default::default()
        });

        let tx = MockTransaction::eip1559().with_max_fee(0).with_priority_fee(0);
        pool.add_transaction(TransactionOrigin::Local, tx).await.unwrap();

        assert!(pool.pending_transactions().is_empty(), "without gasless, must not be pending");
        assert_eq!(pool.queued_transactions().len(), 1, "without gasless, 0-price tx is parked");
        assert_eq!(
            pool.best_transactions_with_attributes(BestTransactionsAttributes::new(base_fee, None))
                .count(),
            0,
            "without gasless, 0-price tx must not be yielded",
        );
    }

    // Stale-pending gasless eviction: a pending gasless tx is removed once it has sat in pending
    // longer than the max lifetime, but kept while fresh. `now` is advanced past the threshold to
    // exercise `GASLESS_DEFAULT_PENDING_MAX_LIFETIME` deterministically (the insert timestamp can't
    // be aged in a unit test). Non-gasless txs are structurally untouched — the helper only
    // queries `max_fee_per_gas == 0`.
    #[tokio::test]
    async fn evict_stale_pending_gasless_respects_lifetime() {
        use reth_transaction_pool::{
            test_utils::{MockTransaction, TestPool, TestPoolBuilder},
            BlockInfo, PoolConfig, TransactionOrigin, TransactionPool, TransactionPoolExt,
        };

        let pool: TestPool = TestPoolBuilder::default()
            .with_config(PoolConfig { allow_gasless: true, ..Default::default() })
            .into();
        pool.set_block_info(BlockInfo {
            pending_basefee: 100,
            block_gas_limit: 30_000_000,
            ..Default::default()
        });

        let tx = MockTransaction::eip1559().with_max_fee(0).with_priority_fee(0);
        pool.add_transaction(TransactionOrigin::Local, tx).await.unwrap();
        assert_eq!(pool.pending_transactions().len(), 1, "gasless tx should be pending");

        // Fresh: within the lifetime, so kept. The return value is the pending gasless count.
        let total = evict_stale_pending_gasless(
            &pool,
            Instant::now(),
            GASLESS_DEFAULT_PENDING_MAX_LIFETIME,
        );
        assert_eq!(total, 1);
        assert_eq!(pool.pending_transactions().len(), 1, "fresh gasless tx must not be evicted");

        // Past the lifetime (simulated via a future `now`): evicted.
        let later = Instant::now() + GASLESS_DEFAULT_PENDING_MAX_LIFETIME + Duration::from_secs(1);
        let total = evict_stale_pending_gasless(&pool, later, GASLESS_DEFAULT_PENDING_MAX_LIFETIME);
        assert_eq!(total, 1, "still counted before eviction");
        assert!(pool.pending_transactions().is_empty(), "stale gasless tx must be evicted");
    }
}
