//! [`OpDualPool`]: a routing wrapper composing the regular OP pool with
//! the EIP-8130 (XLayer AA) [`Eip8130Pool`] side pool.
//!
//! # Architecture
//!
//! Mirrors tempo's `transaction-pool/src/tempo_pool.rs::TempoTransactionPool`:
//! a single composing type that owns both an `OpPool<P>` (for non-AA txs)
//! and an `Arc<RwLock<Eip8130Pool<T>>>` (for AA-only). All
//! [`TransactionPool`] methods are routed:
//!
//! * **Admission** (`add_transaction*`): peeks `tx.is_eip8130()`. AA goes through
//!   [`crate::eip8130_xlayer::validate_eip8130_transaction`] then into the side pool; non-AA
//!   delegates to the inner OP pool.
//! * **Iteration** (`best_transactions*`): merges the inner pool's priority iterator with the side
//!   pool's `independent_pending` snapshot via [`crate::MergeBestTransactions`].
//! * **Aggregation** (`pool_size`, `pending_transactions`, `pooled_transactions`, `unique_senders`,
//!   `get*`, ...): extends or sums results from both pools.
//! * **Cross-pool listener delivery**: AA tx admissions and lifecycle updates are converted into
//!   reth pool events and handed to the protocol pool's listener fan-out. Subscribers of
//!   `pending_transactions_listener_for(...)`, `new_transactions_listener_for(...)`, and
//!   `all_transactions_event_listener()` see AA txs without double-storing them in the protocol
//!   pool's nonce/order/capacity indices.
//! * **Lifecycle** (`on_canonical_state_change`, `maintain_eip8130_state_future`): the
//!   canonical-state hook drives the side pool's nonce / invalidation sweep via
//!   [`Self::on_bundle_state`].
//!
//! # Why we don't double-store AA txs
//!
//! Reth's standard pool keys ordering by `(sender, account.nonce)`. AA
//! txs use `(sender, nonce_key, nonce_seq)` and `nonce_key=0` does **not**
//! share counter with `account.nonce` (it lives at
//! `aa_nonce_slot(sender, 0)` in `NONCE_MANAGER_ADDRESS`). Putting AA txs
//! in the inner pool would conflate the counters: two AA txs with
//! different `nonce_key` but same `nonce_sequence=0` would look like
//! replacements of each other to the inner pool. We route to keep nonce
//! semantics correct.

use std::{fmt, sync::Arc};

// `parking_lot::RwLock` mirrors tempo (`tt_2d_pool.rs:42`): guards return
// directly (no `Result`), no thread-poisoning. Keeps the AA-side wrapper
// free of `.unwrap()` noise on every read/write.
use parking_lot::RwLock;

use alloy_consensus::Transaction;
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{Address, B256, TxHash, U256};
use op_alloy_consensus::AA_TX_TYPE_ID;
use op_revm::constants::ACCOUNT_CONFIG_ADDRESS;
use reth_chainspec::EthChainSpec;
use reth_storage_api::StateProviderFactory;
use reth_transaction_pool::{
    AllPoolTransactions, AllTransactionsEvents, BestTransactions, BestTransactionsAttributes,
    BlobStoreError, BlockInfo, GetPooledTransactionLimit, NewTransactionEvent, Pool, PoolResult,
    PoolSize, PoolTransaction, PropagatedTransactions, SubPool, TransactionEvent,
    TransactionEvents, TransactionListenerKind, TransactionOrdering, TransactionOrigin,
    TransactionPool, TransactionPoolExt, TransactionValidator, ValidPoolTransaction,
    blobstore::BlobStore,
    error::{InvalidPoolTransactionError, PoolError, PoolErrorKind, PoolTransactionError},
    pool::{AddedPendingTransaction, AddedTransaction, AddedTransactionOutcome, QueuedReason},
};
use tokio::sync::mpsc::Receiver;

use crate::{
    Eip8130Pool, Eip8130PoolConfig, Eip8130PoolTx, Eip8130SeqId, Eip8130StateUpdateOutcome,
    MergeBestTransactions, OpPool, eip8130_pool::STATE_DIFF_ADDRESS,
};
use reth_transaction_pool::{
    TransactionValidationOutcome,
    identifier::{SenderId, TransactionId},
    validate::ValidTransaction,
};
use std::{future::Future, time::Instant};

/// Focused contract for the validator handle [`OpDualPool`] uses to
/// pre-validate AA transactions before side-pool insertion. Implemented
/// by [`crate::OpAaTransactionValidator`] for production wiring; a small
/// stub satisfies it for routing-only unit tests in this module.
///
/// The dual pool only needs the async validate call — the full
/// [`reth_transaction_pool::TransactionValidator`] surface (including
/// `Block` associated type, `on_new_head_block`, batch helpers) is
/// owned by the protocol pool's task executor; routing through this
/// narrower trait keeps test stubs free of `Block` plumbing.
///
/// Per the user correction (2026-05-11), `validate` returns a single
/// [`TransactionValidationOutcome`]; the AA `state_nonce` reaches the
/// side pool by overwriting the `Valid::state_nonce` field, and the
/// admission-time payer-balance predicate is recomputed at the call site
/// from tx fields plus [`Self::compute_l1_data_fee`].
pub trait AaPoolPreValidator<T>: Send + Sync + 'static
where
    T: PoolTransaction + Eip8130PoolTx,
{
    /// Validate a single transaction. Returns the same shape as
    /// [`reth_transaction_pool::TransactionValidator::validate_transaction`].
    /// For a `Valid` AA outcome, the `state_nonce` field is the AA-derived
    /// on-chain sequence (read from `NONCE_MANAGER`), not the inner
    /// validator's ETH-style `account.nonce`.
    fn validate(
        &self,
        origin: TransactionOrigin,
        tx: T,
    ) -> impl Future<Output = TransactionValidationOutcome<T>> + Send;

    /// L1 data fee for `tx` against the validator's cached `OpL1BlockInfo`.
    /// Returns `U256::ZERO` for test stubs / dev mode (no L1 info attached).
    /// The dual pool folds this into the admission-time payer-balance
    /// predicate so the side pool's `on_balance_updates` re-validation
    /// matches what the validator just enforced. F5.
    fn compute_l1_data_fee(&self, tx: &T) -> U256;
}

/// `OpPool<P>` + a 2D-nonce-aware side pool for EIP-8130 transactions.
///
/// `P` is the inner reth pool (typically `Pool<TransactionValidationTaskExecutor<...>, ...>`).
/// `Client` provides the `latest()` state read used during AA admission to
/// fetch `aa_nonce_slot(sender, key)` etc. `AaV` is the pre-validator
/// handle used for AA admission; in production it's
/// `Arc<OpAaTransactionValidator<...>>` shared with the protocol pool's
/// task executor so validator state (e.g. `on_new_head_block` cache) is
/// not duplicated.
pub struct OpDualPool<P, Client, AaV = NoopAaPreValidator<<P as TransactionPool>::Transaction>>
where
    P: TransactionPool,
    P::Transaction: Eip8130PoolTx,
    Client: StateProviderFactory + Send + Sync + 'static,
{
    protocol_pool: OpPool<P>,
    aa_pool: Arc<RwLock<Eip8130Pool<P::Transaction>>>,
    aa_validator: Arc<AaV>,
    client: Arc<Client>,
}

impl<P, Client, AaV> Clone for OpDualPool<P, Client, AaV>
where
    P: TransactionPool + Clone,
    P::Transaction: Eip8130PoolTx,
    Client: StateProviderFactory + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            protocol_pool: self.protocol_pool.clone(),
            aa_pool: Arc::clone(&self.aa_pool),
            aa_validator: Arc::clone(&self.aa_validator),
            client: Arc::clone(&self.client),
        }
    }
}

impl<P, Client, AaV> fmt::Debug for OpDualPool<P, Client, AaV>
where
    P: TransactionPool + fmt::Debug,
    P::Transaction: Eip8130PoolTx,
    Client: StateProviderFactory + Send + Sync + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpDualPool")
            .field("protocol_pool", &self.protocol_pool)
            .field("aa_pool_len", &self.aa_pool.read().len())
            .finish()
    }
}

impl<P, Client, AaV> OpDualPool<P, Client, AaV>
where
    P: TransactionPool,
    P::Transaction: Eip8130PoolTx,
    Client: StateProviderFactory + Send + Sync + 'static,
    AaV: AaPoolPreValidator<P::Transaction>,
{
    /// Constructs a new dual pool with default [`Eip8130PoolConfig`].
    /// `chain_id` is captured upfront so admission validation doesn't have
    /// to read it from the chain spec on every tx. `aa_validator` should
    /// be the same `Arc<AaV>` shared with the protocol pool's task
    /// executor so validator state is not duplicated.
    pub fn new(protocol_pool: OpPool<P>, client: Arc<Client>, aa_validator: Arc<AaV>) -> Self
    where
        Client: ChainSpecHolder,
    {
        Self::with_config(protocol_pool, client, aa_validator, Eip8130PoolConfig::default())
    }

    /// Returns the configured client.
    pub fn client(&self) -> &Client {
        self.client.as_ref()
    }

    /// Same as [`Self::new`] but with explicit AA pool tuning.
    pub fn with_config(
        protocol_pool: OpPool<P>,
        client: Arc<Client>,
        aa_validator: Arc<AaV>,
        eip8130_config: Eip8130PoolConfig,
    ) -> Self {
        Self {
            protocol_pool,
            aa_pool: Arc::new(RwLock::new(Eip8130Pool::new(eip8130_config))),
            aa_validator,
            client,
        }
    }

    /// Builds the dual pool with the AA side pool's per-subpool
    /// limits (count + bytes) inherited from the protocol pool's
    /// [`reth_transaction_pool::PoolConfig`]. Operators that tune
    /// `--txpool.pending-max-count` / `--txpool.pending-max-size` (etc.)
    /// expect the AA side pool to honor the same caps; pre-fix it fell
    /// back to reth's defaults via [`Eip8130PoolConfig::default`].
    pub fn with_node_config(
        protocol_pool: OpPool<P>,
        client: Arc<Client>,
        aa_validator: Arc<AaV>,
        node_pool_config: &reth_transaction_pool::PoolConfig,
    ) -> Self {
        Self::with_config(
            protocol_pool,
            client,
            aa_validator,
            Eip8130PoolConfig::from_node_config(node_pool_config),
        )
    }

    /// Returns a reference to the wrapped protocol pool. Use for any
    /// non-AA-specific operation that you want to bypass routing.
    pub fn protocol_pool(&self) -> &OpPool<P> {
        &self.protocol_pool
    }

    /// Returns the side pool's tx count (sequenced + expiring).
    pub fn aa_pool_size(&self) -> usize {
        self.aa_pool.read().len()
    }

    /// Inherent passthrough; prefer the trait form
    /// [`Eip8130PoolView::highest_consecutive_aa_pending_seq_in_lane`] from
    /// generic downstream code (RPC handlers) so the call site doesn't have
    /// to name the concrete `OpDualPool` type.
    pub fn highest_consecutive_aa_pending_seq_in_lane(
        &self,
        seq: Eip8130SeqId,
        on_chain_seq: u64,
    ) -> Option<u64> {
        self.aa_pool.read().highest_consecutive_pending_seq_in_lane(seq, on_chain_seq)
    }

    /// Subscribes to the side pool's new-pending broadcast channel. Used
    /// by the listener-forwarder task to feed AA tx hashes into the
    /// protocol pool's `pending_transactions_listener_for(...)` channels.
    pub fn subscribe_aa_pending(&self) -> tokio::sync::broadcast::Receiver<TxHash> {
        self.aa_pool.read().subscribe_pending()
    }

    /// Builds the AA-side cursor iterator for merging into
    /// [`Self::best_transactions`]. Snapshots under the read lock so the
    /// returned iterator is decoupled from subsequent pool mutations.
    fn aa_best_transactions(
        &self,
        base_fee: u64,
    ) -> crate::best::BestAaTransactions<P::Transaction> {
        self.aa_pool.read().best_aa_transactions(base_fee)
    }

    /// Drives the side pool's per-block state-update sweep from a
    /// `BundleState`. Two eviction passes run in order:
    ///
    /// 1. **Evict on nonce / owner_config / lock / sequence diffs.** Storage-slot deltas at watched
    ///    addresses are extracted via [`bundle_state_aa_diffs`] and routed through
    ///    [`Eip8130Pool::on_state_updates`]: nonce-manager advances mark txs as `mined` / promote
    ///    successors / demote gap heads, while invalidation-rule hits (owner_config revoke /
    ///    verifier mismatch / scope drop, account_state lock window, change_sequence advance) drop
    ///    affected txs.
    /// 2. **Evict on payer balance.** Per-account post-state balances are extracted via
    ///    [`bundle_state_aa_balances`]; the side pool re-runs the admission predicate
    ///    `balance(payer) >= gas_limit * max_fee_per_gas` and drops any pending tx whose payer can
    ///    no longer fund execution. Mirrors tempo's `Check 3b` (`tempo_pool.rs:267-307`).
    ///
    /// Both passes funnel into `fire_aa_lifecycle_events`, which surfaces
    /// `Pending` / `Discarded` through the protocol pool's existing
    /// listener channels — consumers that subscribe via
    /// `transaction_event_listener` see AA tx lifecycle events in the
    /// same stream as regular tx events.
    // TODO: payer pending tx limit. Cap the number of pending txs sponsored
    // by a single payer (orthogonal to `max_txs_per_sender`) so a wealthy
    // sponsor cannot saturate pool capacity by spreading across N senders.
    // Tempo doesn't have this — see investigation in earlier session — so
    // this is a forward-looking hardening once sponsored payer admission is
    // exercised in production.
    pub fn on_bundle_state(
        &self,
        bundle: &op_revm::revm::database::states::BundleState,
        block_timestamp: u64,
        block_hash: B256,
    ) -> Eip8130StateUpdateOutcome<P::Transaction>
    where
        P: NotifyAaLifecycle<P::Transaction>,
    {
        // Pass 1: evict on nonce / owner_config / lock / sequence diffs.
        let mut outcome =
            self.on_state_diffs(bundle_state_aa_diffs(bundle), block_timestamp, block_hash);

        // Pass 2: evict on payer balance. Run after pass 1 so any tx
        // already removed by `mined` / `invalidated` is no longer walked,
        // and so we don't double-count evictions.
        let balance_changes = bundle_state_aa_balances(bundle);
        let insolvent = self.aa_pool.write().on_balance_updates(balance_changes);
        // Surface as `invalidated` so `fire_aa_lifecycle_events` routes
        // them to the same `Discarded` lifecycle hook used by rule-based
        // evictions; subscribers don't need to distinguish the cause.
        outcome.invalidated.extend(insolvent);

        self.fire_aa_lifecycle_events(&outcome, block_hash);
        outcome
    }

    /// Direct entry to [`Eip8130Pool::on_state_updates`] for callers that
    /// already have `(addr, slot, value)` triples in hand. Does **not**
    /// fire lifecycle events — use [`Self::on_bundle_state`] (or call
    /// [`Self::fire_aa_lifecycle_events`] manually) when the caller
    /// wants protocol-pool listeners to see AA transitions.
    ///
    /// `block_timestamp` is forwarded to
    /// [`Eip8130Pool::on_state_updates`] so account_state lock-window
    /// rules can compare against the canonical tip time. `block_hash` is
    /// forwarded so mined txs get a `TransactionEvent::Mined(block_hash)`
    /// emission distinct from generic `Discarded` (F7).
    pub fn on_state_diffs<I>(
        &self,
        diffs: I,
        block_timestamp: u64,
        block_hash: B256,
    ) -> Eip8130StateUpdateOutcome<P::Transaction>
    where
        I: IntoIterator<Item = (Address, U256, U256)>,
    {
        self.aa_pool.write().on_state_updates(diffs, block_timestamp, block_hash)
    }

    /// Drops every AA tx whose `tx.expiry` has elapsed and fires
    /// `Discarded` lifecycle events for them. Returns the evicted set so
    /// the caller can attach metrics or logging.
    ///
    /// Maintenance calls this on every canonical commit using the new
    /// tip's timestamp; admission already rejects stale-expiry txs, but
    /// txs admitted just before their deadline can still linger if no
    /// state-diff references them, so a periodic sweep is required to
    /// keep `expiring/by_hash/slot_to_expiring` from accumulating dead
    /// entries until capacity-driven eviction.
    pub fn sweep_aa_expired(&self, now: u64) -> Vec<Arc<ValidPoolTransaction<P::Transaction>>>
    where
        P: NotifyAaLifecycle<P::Transaction>,
    {
        let expired = self.aa_pool.write().sweep_expired(now);
        if !expired.is_empty() {
            self.protocol_pool.notify_aa_lifecycle(Vec::new(), expired.clone());
        }
        expired
    }

    /// Snapshot of currently pending AA tx hashes. Used by the staleness
    /// tracker in maintenance to detect long-pending entries without
    /// holding the AA pool lock for the duration of the comparison.
    pub fn aa_pending_tx_hashes(&self) -> std::collections::HashSet<TxHash> {
        // Use the hash-only accessor so we don't `Arc::clone` every entry
        // just to drop it (cf PERF-9).
        self.aa_pool.read().pending_hashes().collect()
    }

    /// Removes `hashes` from the AA side pool and fires `Discarded`
    /// lifecycle events through the protocol pool's listener fan-out so
    /// gossip / RPC consumers learn about the eviction. Used by the
    /// staleness tracker in maintenance.
    pub fn remove_aa_transactions(
        &self,
        hashes: &[TxHash],
    ) -> Vec<Arc<ValidPoolTransaction<P::Transaction>>>
    where
        P: NotifyAaLifecycle<P::Transaction>,
    {
        if hashes.is_empty() {
            return Vec::new();
        }
        let mut removed = Vec::with_capacity(hashes.len());
        {
            let mut pool = self.aa_pool.write();
            for hash in hashes {
                if let Some(tx) = pool.remove_by_hash(hash) {
                    removed.push(tx);
                }
            }
        }
        self.notify_aa_discarded(&removed);
        removed
    }

    /// Fans `Discarded` events for AA txs out through the protocol
    /// pool's listener channels (`transaction_event_listener` /
    /// `pending_transactions_listener_for`) when the AA pool's own
    /// `discarded_tx_broadcaster` is not the right reach. Generic
    /// removal call paths (`remove_transactions*` / `prune_transactions`)
    /// route through this so subscribers via the standard reth API see
    /// AA evictions identically to non-AA ones.
    pub(crate) fn notify_aa_discarded(&self, removed: &[Arc<ValidPoolTransaction<P::Transaction>>])
    where
        P: NotifyAaLifecycle<P::Transaction>,
    {
        if removed.is_empty() {
            return;
        }
        self.protocol_pool.notify_aa_lifecycle(Vec::new(), removed.to_vec());
    }

    /// Fires `Pending` events for `outcome.promoted`, `Mined(block_hash)`
    /// for `outcome.mined`, and `Discarded` for `outcome.invalidated`
    /// through the protocol pool's listener fan-out.
    ///
    /// `outcome.demoted` is intentionally not surfaced: a demoted tx is
    /// still in the side pool (just queued behind a new gap), so neither
    /// `Pending` nor `Discarded` semantically applies. Block builders
    /// that care will see the tx absent from `best_transactions` until a
    /// later state update re-promotes it.
    pub fn fire_aa_lifecycle_events(
        &self,
        outcome: &Eip8130StateUpdateOutcome<P::Transaction>,
        block_hash: B256,
    ) where
        P: NotifyAaLifecycle<P::Transaction>,
    {
        if outcome.promoted.is_empty() && outcome.mined.is_empty() && outcome.invalidated.is_empty()
        {
            return;
        }
        if !outcome.mined.is_empty() {
            self.protocol_pool.notify_aa_mined(outcome.mined.clone(), block_hash);
        }
        // Promoted + invalidated still flow through the existing
        // notify_aa_lifecycle path. Mined no longer participates here.
        self.protocol_pool
            .notify_aa_lifecycle(outcome.promoted.clone(), outcome.invalidated.clone());
    }

    // -------- routing helpers --------

    /// Routes an AA admission *with* a per-tx event subscription. The
    /// subscription is installed before admission so synchronous
    /// `Queued` / `Pending` / `Replaced` / `Discarded` events emitted by
    /// the side pool are not missed.
    async fn admit_aa_with_subscribe(
        &self,
        origin: TransactionOrigin,
        transaction: P::Transaction,
    ) -> PoolResult<(TxHash, TransactionEvents)>
    where
        P: NotifyAaLifecycle<P::Transaction>,
    {
        let events_rx = self.aa_pool.read().subscribe_events();
        let added = self.admit_aa_added(origin, transaction).await?;
        let hash = *added.hash();
        let events = spawn_aa_event_adapter(hash, events_rx);
        Ok((hash, events))
    }

    /// Routes an AA admission: pre-validates through the wrapper validator
    /// (which layers EIP-8130 spec checks on top of reth's standard mempool
    /// gates: encoded size, block gas limit, intrinsic gas, EIP-3860,
    /// chain_id, fee cap, ...), builds a [`ValidPoolTransaction`], and
    /// pushes into the side pool. Returns `Ok(AddedTransactionOutcome)` on
    /// success or a translated [`PoolError`] on validation / capacity
    /// failure.
    async fn admit_aa(
        &self,
        origin: TransactionOrigin,
        transaction: P::Transaction,
    ) -> PoolResult<AddedTransactionOutcome>
    where
        P: NotifyAaLifecycle<P::Transaction>,
    {
        let added = self.admit_aa_added(origin, transaction).await?;
        let hash = *added.hash();
        let state = added.transaction_state();
        Ok(AddedTransactionOutcome { hash, state })
    }

    async fn admit_aa_added(
        &self,
        origin: TransactionOrigin,
        transaction: P::Transaction,
    ) -> PoolResult<AddedTransaction<P::Transaction>>
    where
        P: NotifyAaLifecycle<P::Transaction>,
    {
        let prevalidated = self.prevalidate_aa(origin, transaction).await?;
        self.finalize_admit_aa(prevalidated)
    }

    /// Batch admission for AA txs. Runs the state-IO-heavy validation phase
    /// concurrently across all inputs (via `join_all`), then serializes the
    /// side-pool insert. The serial bottleneck is the side-pool write lock
    /// — concurrent inserts would just contend on it. F6.
    ///
    /// Returns `(idx, PoolResult<AddedTransactionOutcome>)` pairs in input
    /// order so the caller can re-merge with non-AA results.
    async fn admit_aa_batch<I>(
        &self,
        tagged: I,
    ) -> Vec<(usize, PoolResult<AddedTransactionOutcome>)>
    where
        I: IntoIterator<Item = (usize, TransactionOrigin, P::Transaction)>,
        P: NotifyAaLifecycle<P::Transaction>,
    {
        // Step 1 (parallel): pre-validate every tx. State-provider reads
        // (NONCE_MANAGER / ACCOUNT_CONFIG / sender) interleave naturally
        // because `prevalidate_aa` is purely async — no shared mutex. The
        // pattern mirrors `OpAaTransactionValidator::validate_transactions`
        // (`aa_validator.rs:331`).
        let items: Vec<(usize, TransactionOrigin, P::Transaction)> = tagged.into_iter().collect();
        let prevalidations =
            futures_util::future::join_all(items.into_iter().map(|(idx, origin, tx)| async move {
                let result = self.prevalidate_aa(origin, tx).await;
                (idx, result)
            }))
            .await;

        // Step 2 (sequential): finalize each admission against the side
        // pool. The `aa_pool` write lock would serialize this anyway, and
        // sequential insert keeps ordering deterministic for replacements /
        // capacity eviction.
        let mut results = Vec::with_capacity(prevalidations.len());
        for (idx, prevalidated) in prevalidations {
            let outcome = prevalidated.and_then(|p| {
                let added = self.finalize_admit_aa(p)?;
                let hash = *added.hash();
                let state = added.transaction_state();
                Ok(AddedTransactionOutcome { hash, state })
            });
            results.push((idx, outcome));
        }
        results
    }

    /// State-IO half of AA admission: routes the tx through the AA validator
    /// (which itself fans out to NONCE_MANAGER / ACCOUNT_CONFIG / sender state
    /// reads) and translates the validation outcome into a successful
    /// [`AaPrevalidated`] or a terminal [`PoolError`].
    ///
    /// Pulled out of `admit_aa_added` so batch admission paths
    /// (`add_transactions*`) can run this stage concurrently via
    /// `join_all` and only serialize the side-pool insert that the
    /// `aa_pool` write-lock would serialize anyway. F6.
    ///
    /// Caveat: keeps the per-tx `required_balance` computation here so the
    /// sequential insert phase only does pool mutation — see F5 at the
    /// previous call site.
    async fn prevalidate_aa(
        &self,
        origin: TransactionOrigin,
        transaction: P::Transaction,
    ) -> PoolResult<AaPrevalidated<P::Transaction>> {
        let hash = *transaction.hash();
        if !transaction.is_eip8130() {
            return Err(PoolError::new(
                hash,
                PoolErrorKind::InvalidTransaction(InvalidPoolTransactionError::Other(Box::new(
                    NotAaTxError {},
                ))),
            ));
        }

        let outcome = self.aa_validator.validate(origin, transaction).await;

        let (transaction, state_nonce, propagate) = match outcome {
            TransactionValidationOutcome::Valid {
                transaction: ValidTransaction::Valid(tx),
                state_nonce,
                propagate,
                ..
            } => {
                // The AA layer overwrote `state_nonce` with the
                // NONCE_MANAGER-derived value before producing `Valid`; no
                // side-channel field needed (user correction 2026-05-11).
                (tx, state_nonce, propagate)
            }
            TransactionValidationOutcome::Valid {
                transaction: ValidTransaction::ValidWithSidecar { transaction: tx, .. },
                ..
            } => {
                // AA txs do not carry blob sidecars; defensive — treat the
                // sidecar as ignorable and continue.
                return Err(PoolError::new(
                    *tx.hash(),
                    PoolErrorKind::InvalidTransaction(InvalidPoolTransactionError::Other(
                        Box::new(NotAaTxError {}),
                    )),
                ));
            }
            TransactionValidationOutcome::Invalid(_, err) => {
                return Err(PoolError::new(hash, PoolErrorKind::InvalidTransaction(err)));
            }
            TransactionValidationOutcome::Error(_, err) => {
                return Err(PoolError::new(
                    hash,
                    PoolErrorKind::InvalidTransaction(InvalidPoolTransactionError::Other(
                        Box::new(WrappedValidatorError { inner: err.to_string() }),
                    )),
                ));
            }
        };

        // Recompute the admission-time payer-balance predicate at the
        // call site from tx fields plus the validator's cached L1 fee, so
        // the side pool's `on_balance_updates` re-validation matches the
        // threshold the validator just enforced. Computed here (not in
        // `finalize_admit_aa`) so batch admission gets this work in
        // parallel.
        let l1_data_fee = self.aa_validator.compute_l1_data_fee(&transaction);
        let required_balance = U256::from(transaction.gas_limit())
            .saturating_mul(U256::from(transaction.max_fee_per_gas()))
            .saturating_add(l1_data_fee);

        Ok(AaPrevalidated { hash, transaction, origin, state_nonce, propagate, required_balance })
    }

    /// Pool-mutation half of AA admission: inserts the pre-validated tx into
    /// the side pool and emits the lifecycle notification. The write-lock
    /// on `aa_pool` serializes admissions here; the validation phase already
    /// produced everything the insert needs.
    fn finalize_admit_aa(
        &self,
        prevalidated: AaPrevalidated<P::Transaction>,
    ) -> PoolResult<AddedTransaction<P::Transaction>>
    where
        P: NotifyAaLifecycle<P::Transaction>,
    {
        let AaPrevalidated { hash, transaction, origin, state_nonce, propagate, required_balance } =
            prevalidated;

        let valid = Arc::new(ValidPoolTransaction {
            transaction,
            transaction_id: TransactionId::new(SenderId::from(0), state_nonce),
            propagate,
            timestamp: Instant::now(),
            origin,
            authority_ids: None,
        });
        let base_fee = self.protocol_pool.block_info().pending_basefee;
        let side_added = self
            .aa_pool
            .write()
            .add_transaction_with_required(valid, state_nonce, base_fee, required_balance)
            .map_err(|err| match err.kind {
                PoolErrorKind::AlreadyImported => {
                    PoolError::new(hash, PoolErrorKind::AlreadyImported)
                }
                other => PoolError::new(hash, other),
            })?;
        let added = into_reth_added(side_added);

        if let Some(pending) = added.as_pending() {
            if pending.discarded.iter().any(|tx| tx.hash() == &hash) {
                return Err(PoolError::new(hash, PoolErrorKind::DiscardedOnInsert));
            }
        }

        self.protocol_pool.notify_aa_added(added.clone());
        Ok(added)
    }
}

/// AA admission state handed from the (async, parallelizable) validation
/// stage to the (sync, write-lock-serialized) side-pool insert stage.
/// See [`OpDualPool::prevalidate_aa`] / [`OpDualPool::finalize_admit_aa`].
struct AaPrevalidated<T> {
    hash: TxHash,
    transaction: T,
    origin: TransactionOrigin,
    state_nonce: u64,
    propagate: bool,
    required_balance: U256,
}

/// Read-only view onto the AA side pool exposed for generic downstream
/// consumers (e.g. RPC handlers) so they can call per-lane queries
/// without naming the concrete [`OpDualPool`] type.
///
/// Implemented for [`OpDualPool`] as a thin delegation to the inner
/// [`Eip8130Pool`]. Callers that already have an `&OpDualPool` can call
/// the inherent methods directly; this trait is the opt-in for
/// trait-bound generic code that holds an `impl TransactionPool` and
/// wants AA-aware lane queries.
pub trait Eip8130PoolView {
    /// See [`Eip8130Pool::highest_consecutive_pending_seq_in_lane`].
    fn highest_consecutive_aa_pending_seq_in_lane(
        &self,
        seq: Eip8130SeqId,
        on_chain_seq: u64,
    ) -> Option<u64>;
}

impl<P, Client, AaV> Eip8130PoolView for OpDualPool<P, Client, AaV>
where
    P: TransactionPool,
    P::Transaction: Eip8130PoolTx,
    Client: StateProviderFactory + Send + Sync + 'static,
    AaV: AaPoolPreValidator<P::Transaction>,
{
    fn highest_consecutive_aa_pending_seq_in_lane(
        &self,
        seq: Eip8130SeqId,
        on_chain_seq: u64,
    ) -> Option<u64> {
        Self::highest_consecutive_aa_pending_seq_in_lane(self, seq, on_chain_seq)
    }
}

/// Trait alias bound used by [`OpDualPool::new`] to ensure the supplied
/// `Client` knows how to surface the chain ID without us having to plumb
/// it as a separate constructor argument. [`Self::new`] uses this; the
/// looser [`Self::with_config`] does not (the caller already supplies
/// `chain_id` explicitly).
pub trait ChainSpecHolder: StateProviderFactory + Send + Sync + 'static {
    /// The chain id surfaced by the embedded chain spec.
    fn chain_id(&self) -> u64;
}

impl<C> ChainSpecHolder for C
where
    C: StateProviderFactory
        + reth_chainspec::ChainSpecProvider<ChainSpec: EthChainSpec>
        + Send
        + Sync
        + 'static,
{
    fn chain_id(&self) -> u64 {
        self.chain_spec().chain_id()
    }
}

#[derive(Debug, thiserror::Error)]
#[error("transaction routed to AA pool but is not an EIP-8130 envelope")]
struct NotAaTxError;

impl PoolTransactionError for NotAaTxError {
    fn is_bad_transaction(&self) -> bool {
        true
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Wraps a validator-side error (state read failure, service unreachable)
/// so the dual pool can surface it as a `PoolError` without losing the
/// human-readable message. Treated as transient (`!is_bad_transaction`).
#[derive(Debug, thiserror::Error)]
#[error("AA validator error: {inner}")]
struct WrappedValidatorError {
    inner: String,
}

impl PoolTransactionError for WrappedValidatorError {
    fn is_bad_transaction(&self) -> bool {
        false
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Stub [`AaPoolPreValidator`] used as the default `AaV` parameter for
/// [`OpDualPool`]. Returns
/// [`reth_transaction_pool::TransactionValidationOutcome::Error`] for any
/// AA tx — production wiring must supply
/// [`crate::OpAaTransactionValidator`] via [`OpDualPool::new`] /
/// [`OpDualPool::with_config`]. Routing-only unit tests in this module
/// substitute a small custom stub via the same constructors.
#[derive(Debug, Default)]
pub struct NoopAaPreValidator<T>(std::marker::PhantomData<T>);

impl<T> AaPoolPreValidator<T> for NoopAaPreValidator<T>
where
    T: PoolTransaction + Eip8130PoolTx,
{
    async fn validate(&self, _origin: TransactionOrigin, tx: T) -> TransactionValidationOutcome<T> {
        let hash = *tx.hash();
        TransactionValidationOutcome::Error(hash, Box::new(NoopAaPreValidatorError {}))
    }

    fn compute_l1_data_fee(&self, _tx: &T) -> U256 {
        // Noop wiring never admits, so the value is unobservable.
        U256::ZERO
    }
}

#[derive(Debug, thiserror::Error)]
#[error("AA pre-validator not configured (NoopAaPreValidator)")]
struct NoopAaPreValidatorError;

// Production wiring: `OpAaTransactionValidator` implements
// `TransactionValidator`; the `validator: Arc<V>` field is constructed
// directly with `Arc::new(wrapper)` and the dual pool dereferences it via
// `(*self.aa_validator).validate(...)`. We bridge from
// `TransactionValidator` to `AaPoolPreValidator` per concrete production
// type to avoid a blanket impl that would conflict with future upstream
// `Arc<V>: TransactionValidator` blankets.
impl<V, Client> AaPoolPreValidator<<V as reth_transaction_pool::TransactionValidator>::Transaction>
    for crate::OpAaTransactionValidator<V, Client>
where
    Client: reth_chainspec::ChainSpecProvider<
            ChainSpec: reth_optimism_forks::OpHardforks + reth_chainspec::EthChainSpec,
        > + StateProviderFactory
        + Send
        + Sync
        + 'static,
    V: reth_transaction_pool::TransactionValidator + 'static,
    V::Transaction: Eip8130PoolTx + crate::OpPooledTx,
{
    fn validate(
        &self,
        origin: TransactionOrigin,
        tx: <V as reth_transaction_pool::TransactionValidator>::Transaction,
    ) -> impl Future<
        Output = TransactionValidationOutcome<
            <V as reth_transaction_pool::TransactionValidator>::Transaction,
        >,
    > + Send {
        // `validate_one` runs the inner validator + apply_aa_layer; the AA
        // layer overwrites `Valid::state_nonce` with the NONCE_MANAGER
        // read, which is the only AA-specific datum the side pool needs.
        self.validate_one(origin, tx)
    }

    fn compute_l1_data_fee(
        &self,
        tx: &<V as reth_transaction_pool::TransactionValidator>::Transaction,
    ) -> U256 {
        // Delegate to the inherent method on `OpAaTransactionValidator`.
        // Errors only surface on transient state-provider failures mid-fee
        // calculation; the admission path will hit the same condition on
        // its own state read. Fall back to ZERO (matches dev-mode wiring
        // where no `OpL1BlockInfo` is attached).
        crate::OpAaTransactionValidator::<V, Client>::compute_l1_data_fee(self, tx)
            .unwrap_or(U256::ZERO)
    }
}

// ---------------------------------------------------------------------------
// TransactionPool impl — routing + aggregation
// ---------------------------------------------------------------------------

impl<P, Client, AaV> TransactionPool for OpDualPool<P, Client, AaV>
where
    P: TransactionPool + NotifyAaLifecycle<P::Transaction>,
    P::Transaction: Eip8130PoolTx + Transaction,
    Client: StateProviderFactory + Send + Sync + 'static,
    AaV: AaPoolPreValidator<P::Transaction>,
{
    type Transaction = P::Transaction;

    async fn add_transaction_and_subscribe(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> PoolResult<TransactionEvents> {
        if transaction.is_eip8130() {
            // AA path: side-pool admission + per-tx event subscription
            // wired to the AA pool's pending/discarded broadcasts. See
            // `admit_aa_with_subscribe` for the adapter contract.
            let (_, events) = self.admit_aa_with_subscribe(origin, transaction).await?;
            return Ok(events);
        }
        self.protocol_pool.add_transaction_and_subscribe(origin, transaction).await
    }

    async fn add_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> PoolResult<AddedTransactionOutcome> {
        if transaction.is_eip8130() {
            return self.admit_aa(origin, transaction).await;
        }
        // carries validation inner
        self.protocol_pool.add_transaction(origin, transaction).await
    }

    async fn add_transactions(
        &self,
        origin: TransactionOrigin,
        transactions: Vec<Self::Transaction>,
    ) -> Vec<PoolResult<AddedTransactionOutcome>> {
        // Callers pair input position N with result position N (the trait
        // contract is implicit but assumed throughout reth — see
        // `Pool::add_transactions` / tempo's `add_validated_transaction`
        // dispatch). Preserve that by tagging each input with its index,
        // splitting routes, then reassembling in the original order.
        // Partition straight off the iterator so we don't allocate a
        // throwaway `tagged` Vec just to drain it (cf PERF-5).
        let (aa, non_aa): (Vec<_>, Vec<_>) =
            transactions.into_iter().enumerate().partition(|(_, tx)| tx.is_eip8130());
        let aa_results =
            self.admit_aa_batch(aa.into_iter().map(|(idx, tx)| (idx, origin, tx))).await;
        let non_aa_idx: Vec<usize> = non_aa.iter().map(|(i, _)| *i).collect();
        let non_aa_txs: Vec<P::Transaction> = non_aa.into_iter().map(|(_, tx)| tx).collect();
        let inner_results = self.protocol_pool.add_transactions(origin, non_aa_txs).await;
        merge_routed_results(non_aa_idx.into_iter().zip(inner_results), aa_results)
    }

    async fn add_external_transactions(
        &self,
        transactions: Vec<Self::Transaction>,
    ) -> Vec<PoolResult<AddedTransactionOutcome>> {
        // Partition straight off the iterator (cf PERF-5).
        let (aa, non_aa): (Vec<_>, Vec<_>) =
            transactions.into_iter().enumerate().partition(|(_, tx)| tx.is_eip8130());
        let aa_results = self
            .admit_aa_batch(aa.into_iter().map(|(idx, tx)| (idx, TransactionOrigin::External, tx)))
            .await;
        let non_aa_idx: Vec<usize> = non_aa.iter().map(|(i, _)| *i).collect();
        let non_aa_txs: Vec<P::Transaction> = non_aa.into_iter().map(|(_, tx)| tx).collect();
        let inner_results = self.protocol_pool.add_external_transactions(non_aa_txs).await;
        merge_routed_results(non_aa_idx.into_iter().zip(inner_results), aa_results)
    }

    async fn add_transactions_with_origins(
        &self,
        transactions: Vec<(TransactionOrigin, Self::Transaction)>,
    ) -> Vec<PoolResult<AddedTransactionOutcome>> {
        // Partition straight off the iterator (cf PERF-5).
        let (aa, non_aa): (Vec<_>, Vec<_>) =
            transactions.into_iter().enumerate().partition(|(_, (_, tx))| tx.is_eip8130());
        let aa_results =
            self.admit_aa_batch(aa.into_iter().map(|(idx, (origin, tx))| (idx, origin, tx))).await;
        let non_aa_idx: Vec<usize> = non_aa.iter().map(|(i, _)| *i).collect();
        let non_aa_txs: Vec<(TransactionOrigin, P::Transaction)> =
            non_aa.into_iter().map(|(_, t)| t).collect();
        let inner_results = self.protocol_pool.add_transactions_with_origins(non_aa_txs).await;
        merge_routed_results(non_aa_idx.into_iter().zip(inner_results), aa_results)
    }

    fn pool_size(&self) -> PoolSize {
        let mut size = self.protocol_pool.pool_size();
        let aa = self.aa_pool.read();
        let aa_pending = aa.all_pending().count();
        let aa_queued = aa.all_queued().count();
        size.total = size.total.saturating_add(aa.len());
        size.pending = size.pending.saturating_add(aa_pending);
        size.queued = size.queued.saturating_add(aa_queued);
        size
    }

    fn block_info(&self) -> BlockInfo {
        self.protocol_pool.block_info()
    }

    fn transaction_event_listener(&self, tx_hash: TxHash) -> Option<TransactionEvents> {
        if let Some(events) = self.protocol_pool.transaction_event_listener(tx_hash) {
            return Some(events);
        }
        let aa = self.aa_pool.read();
        aa.contains(&tx_hash).then(|| spawn_aa_event_adapter(tx_hash, aa.subscribe_events()))
    }

    fn all_transactions_event_listener(&self) -> AllTransactionsEvents<Self::Transaction> {
        self.protocol_pool.all_transactions_event_listener()
    }

    fn pending_transactions_listener_for(&self, kind: TransactionListenerKind) -> Receiver<TxHash> {
        // AA admission/promotions are now injected into the protocol
        // pool's standard listener fan-out via `notify_aa_added` /
        // `notify_aa_lifecycle`, so the protocol receiver is the single
        // canonical stream. Its own listener kind filtering handles
        // `propagate == false` for propagate-only consumers.
        self.protocol_pool.pending_transactions_listener_for(kind)
    }

    fn blob_transaction_sidecars_listener(
        &self,
    ) -> Receiver<reth_transaction_pool::NewBlobSidecar> {
        self.protocol_pool.blob_transaction_sidecars_listener()
    }

    fn new_transactions_listener_for(
        &self,
        kind: TransactionListenerKind,
    ) -> Receiver<NewTransactionEvent<Self::Transaction>> {
        self.protocol_pool.new_transactions_listener_for(kind)
    }

    fn pooled_transaction_hashes(&self) -> Vec<TxHash> {
        let mut hashes = self.protocol_pool.pooled_transaction_hashes();
        let aa = self.aa_pool.read();
        for tx in aa.all_pending().chain(aa.all_queued()) {
            if !tx.propagate {
                continue;
            }
            hashes.push(*tx.hash());
        }
        hashes
    }

    fn pooled_transaction_hashes_max(&self, max: usize) -> Vec<TxHash> {
        let mut hashes = self.protocol_pool.pooled_transaction_hashes_max(max);
        if hashes.len() >= max {
            return hashes;
        }
        let aa = self.aa_pool.read();
        for tx in aa.all_pending().chain(aa.all_queued()) {
            if !tx.propagate {
                continue;
            }
            if hashes.len() >= max {
                break;
            }
            hashes.push(*tx.hash());
        }
        hashes
    }

    fn pooled_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        let mut txs = self.protocol_pool.pooled_transactions();
        let aa = self.aa_pool.read();
        txs.extend(aa.all_pending().chain(aa.all_queued()).filter(|tx| tx.propagate));
        txs
    }

    fn pooled_transactions_max(
        &self,
        max: usize,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        let mut txs = self.protocol_pool.pooled_transactions_max(max);
        if txs.len() >= max {
            return txs;
        }
        let aa = self.aa_pool.read();
        for tx in aa.all_pending().chain(aa.all_queued()) {
            if !tx.propagate {
                continue;
            }
            if txs.len() >= max {
                break;
            }
            txs.push(tx);
        }
        txs
    }

    fn get_pooled_transaction_elements(
        &self,
        tx_hashes: Vec<TxHash>,
        limit: GetPooledTransactionLimit,
    ) -> Vec<<Self::Transaction as PoolTransaction>::Pooled> {
        // Cumulative size accounting across both pools. Mirrors tempo's
        // `append_pooled_transaction_elements` which threads
        // `accumulated_size` through both branches so the response stays
        // within `GetPooledTransactionLimit::ResponseSizeSoftLimit`.
        let mut out = self.protocol_pool.get_pooled_transaction_elements(tx_hashes.clone(), limit);
        let mut accumulated: usize = out.iter().map(|p| p.encode_2718_len()).sum();
        if limit.exceeds(accumulated) {
            return out;
        }
        let aa = self.aa_pool.read();
        for hash in &tx_hashes {
            let Some(tx) = aa.get(hash) else { continue };
            if !tx.propagate {
                continue;
            }
            let candidate_size = tx.transaction.encoded_length();
            if limit.exceeds(accumulated.saturating_add(candidate_size)) {
                break;
            }
            if let Ok(pooled) = tx.transaction.clone_into_pooled() {
                accumulated = accumulated.saturating_add(candidate_size);
                out.push(pooled.into_inner());
            }
        }
        out
    }

    fn get_pooled_transaction_element(
        &self,
        tx_hash: TxHash,
    ) -> Option<reth_primitives_traits::Recovered<<Self::Transaction as PoolTransaction>::Pooled>>
    {
        if let Some(tx) = self.protocol_pool.get_pooled_transaction_element(tx_hash) {
            return Some(tx);
        }
        let aa = self.aa_pool.read();
        let tx = aa.get(&tx_hash)?;
        if !tx.propagate {
            return None;
        }
        tx.transaction.clone_into_pooled().ok()
    }

    fn best_transactions(
        &self,
    ) -> Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<Self::Transaction>>>> {
        let base_fee = self.protocol_pool.block_info().pending_basefee;
        let aa = self.aa_best_transactions(base_fee);
        Box::new(MergeBestTransactions::new(
            self.protocol_pool.best_transactions(),
            Box::new(aa),
            base_fee,
        ))
    }

    fn best_transactions_with_attributes(
        &self,
        attrs: BestTransactionsAttributes,
    ) -> Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<Self::Transaction>>>> {
        let base_fee = attrs.basefee;
        let aa = self.aa_best_transactions(base_fee);
        Box::new(MergeBestTransactions::new(
            self.protocol_pool.best_transactions_with_attributes(attrs),
            Box::new(aa),
            base_fee,
        ))
    }

    fn pending_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        let mut txs = self.protocol_pool.pending_transactions();
        txs.extend(self.aa_pool.read().all_pending());
        txs
    }

    fn pending_transactions_max(
        &self,
        max: usize,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        let mut txs = self.protocol_pool.pending_transactions_max(max);
        if txs.len() >= max {
            return txs;
        }
        for tx in self.aa_pool.read().all_pending() {
            if txs.len() >= max {
                break;
            }
            txs.push(tx);
        }
        txs
    }

    fn queued_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        let mut txs = self.protocol_pool.queued_transactions();
        txs.extend(self.aa_pool.read().all_queued());
        txs
    }

    fn pending_and_queued_txn_count(&self) -> (usize, usize) {
        let (pp, pq) = self.protocol_pool.pending_and_queued_txn_count();
        let aa = self.aa_pool.read();
        let aa_pending = aa.all_pending().count();
        let aa_queued = aa.all_queued().count();
        (pp.saturating_add(aa_pending), pq.saturating_add(aa_queued))
    }

    fn all_transactions(&self) -> AllPoolTransactions<Self::Transaction> {
        let mut all = self.protocol_pool.all_transactions();
        let aa = self.aa_pool.read();
        all.pending.extend(aa.all_pending());
        all.queued.extend(aa.all_queued());
        all
    }

    fn all_transaction_hashes(&self) -> Vec<TxHash> {
        let mut hashes = self.protocol_pool.all_transaction_hashes();
        let aa = self.aa_pool.read();
        for tx in aa.all_pending().chain(aa.all_queued()) {
            hashes.push(*tx.hash());
        }
        hashes
    }

    fn remove_transactions(
        &self,
        hashes: Vec<TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        let aa_removed: Vec<_> =
            hashes.iter().filter_map(|h| self.aa_pool.write().remove_by_hash(h)).collect();
        // Fan AA discards out through the protocol pool's listener
        // channels so RPC / gossip subscribers learn of the eviction.
        // Pre-fix the AA pool's own `Discarded` broadcast fired but the
        // protocol pool's `pending_transactions_listener_for` /
        // `transaction_event_listener` consumers never saw it.
        self.notify_aa_discarded(&aa_removed);
        let mut removed = aa_removed;
        removed.extend(self.protocol_pool.remove_transactions(hashes));
        removed
    }

    fn remove_transactions_and_descendants(
        &self,
        hashes: Vec<TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        // AA pool: match tempo semantics by removing the requested tx and
        // every later nonce in the same `(sender, nonce_key)` lane.
        let mut aa_removed = Vec::new();
        {
            let mut aa = self.aa_pool.write();
            for hash in &hashes {
                aa_removed.extend(aa.remove_by_hash_and_descendants(hash));
            }
        }
        self.notify_aa_discarded(&aa_removed);
        let mut removed = aa_removed;
        removed.extend(self.protocol_pool.remove_transactions_and_descendants(hashes));
        removed
    }

    fn remove_transactions_by_sender(
        &self,
        sender: Address,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        // Side pool: collect all hashes of txs owned by `sender` across
        // pending lanes + queued lanes + expiring set. Read lock first
        // to scan, drop, take write lock to remove.
        let aa_hashes: Vec<TxHash> = {
            let pool = self.aa_pool.read();
            pool.all_pending()
                .chain(pool.all_queued())
                .filter(|tx| tx.sender() == sender)
                .map(|tx| *tx.hash())
                .collect()
        };
        let aa_removed: Vec<_> =
            aa_hashes.into_iter().filter_map(|h| self.aa_pool.write().remove_by_hash(&h)).collect();
        self.notify_aa_discarded(&aa_removed);
        let mut removed = aa_removed;
        removed.extend(self.protocol_pool.remove_transactions_by_sender(sender));
        removed
    }

    fn prune_transactions(
        &self,
        hashes: Vec<TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        let aa_removed: Vec<_> =
            hashes.iter().filter_map(|h| self.aa_pool.write().remove_by_hash(h)).collect();
        self.notify_aa_discarded(&aa_removed);
        let mut removed = aa_removed;
        removed.extend(self.protocol_pool.prune_transactions(hashes));
        removed
    }

    fn retain_unknown<A>(&self, announcement: &mut A)
    where
        A: reth_eth_wire_types::HandleMempoolData,
    {
        // Filter via protocol pool first; remaining ones we cross-check
        // against the AA pool by hash.
        self.protocol_pool.retain_unknown(announcement);
        let aa = self.aa_pool.read();
        announcement.retain_by_hash(|hash| !aa.contains(hash));
    }

    fn get(&self, tx_hash: &TxHash) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>> {
        if let Some(tx) = self.protocol_pool.get(tx_hash) {
            return Some(tx);
        }
        // Side pool's `get` indexes by hash and returns both pending and
        // queued txs (the inner `by_hash` map covers all admitted txs
        // regardless of lifecycle bucket).
        self.aa_pool.read().get(tx_hash).cloned()
    }

    fn get_all(&self, txs: Vec<TxHash>) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        let mut out = self.protocol_pool.get_all(txs.clone());
        let aa = self.aa_pool.read();
        for hash in &txs {
            if let Some(tx) = aa.get(hash) {
                out.push(tx.clone());
            }
        }
        out
    }

    fn on_propagated(&self, txs: PropagatedTransactions) {
        // AA-pool propagation is delivered via the broadcast forwarder
        // task; this hook only matters for the protocol pool.
        self.protocol_pool.on_propagated(txs);
    }

    fn get_transactions_by_sender(
        &self,
        sender: Address,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        let mut txs = self.protocol_pool.get_transactions_by_sender(sender);
        let aa = self.aa_pool.read();
        for tx in aa.all_pending().chain(aa.all_queued()) {
            if tx.sender() == sender {
                txs.push(tx);
            }
        }
        txs
    }

    fn get_pending_transactions_with_predicate(
        &self,
        mut predicate: impl FnMut(&ValidPoolTransaction<Self::Transaction>) -> bool,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        let mut txs = self.protocol_pool.get_pending_transactions_with_predicate(&mut predicate);
        for tx in self.aa_pool.read().all_pending() {
            if predicate(&tx) {
                txs.push(tx);
            }
        }
        txs
    }

    fn get_pending_transactions_by_sender(
        &self,
        sender: Address,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        let mut txs = self.protocol_pool.get_pending_transactions_by_sender(sender);
        for tx in self.aa_pool.read().all_pending() {
            if tx.sender() == sender {
                txs.push(tx);
            }
        }
        txs
    }

    fn get_queued_transactions_by_sender(
        &self,
        sender: Address,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        let mut txs = self.protocol_pool.get_queued_transactions_by_sender(sender);
        for tx in self.aa_pool.read().all_queued() {
            if tx.sender() == sender {
                txs.push(tx);
            }
        }
        txs
    }

    fn get_highest_transaction_by_sender(
        &self,
        sender: Address,
    ) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>> {
        // AA pool's "highest" is per-lane; the abstraction doesn't
        // generalize across nonce_keys. Delegate.
        self.protocol_pool.get_highest_transaction_by_sender(sender)
    }

    /// Standard 1D-nonce semantics; 2D-nonce EIP-8130 transactions don't
    /// participate. AA pool entries are not surfaced through this method by
    /// design — the legacy nonce concept doesn't apply to
    /// `(sender, nonce_key, nonce_seq)` lanes.
    fn get_highest_consecutive_transaction_by_sender(
        &self,
        sender: Address,
        on_chain_nonce: u64,
    ) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.protocol_pool.get_highest_consecutive_transaction_by_sender(sender, on_chain_nonce)
    }

    /// Standard 1D-nonce semantics; 2D-nonce EIP-8130 transactions don't
    /// participate. AA pool entries are not surfaced through this method by
    /// design — the legacy nonce concept doesn't apply to
    /// `(sender, nonce_key, nonce_seq)` lanes.
    fn get_transaction_by_sender_and_nonce(
        &self,
        sender: Address,
        nonce: u64,
    ) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.protocol_pool.get_transaction_by_sender_and_nonce(sender, nonce)
    }

    fn get_transactions_by_origin(
        &self,
        origin: TransactionOrigin,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        let mut txs = self.protocol_pool.get_transactions_by_origin(origin);
        let aa = self.aa_pool.read();
        for tx in aa.all_pending().chain(aa.all_queued()) {
            if tx.origin == origin {
                txs.push(tx);
            }
        }
        txs
    }

    fn get_pending_transactions_by_origin(
        &self,
        origin: TransactionOrigin,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        let mut txs = self.protocol_pool.get_pending_transactions_by_origin(origin);
        for tx in self.aa_pool.read().all_pending() {
            if tx.origin == origin {
                txs.push(tx);
            }
        }
        txs
    }

    fn unique_senders(&self) -> alloy_primitives::map::AddressSet {
        let mut senders = self.protocol_pool.unique_senders();
        let aa = self.aa_pool.read();
        for tx in aa.all_pending().chain(aa.all_queued()) {
            senders.insert(tx.sender());
        }
        senders
    }

    fn get_blob(
        &self,
        tx_hash: TxHash,
    ) -> Result<Option<Arc<alloy_eips::eip7594::BlobTransactionSidecarVariant>>, BlobStoreError>
    {
        self.protocol_pool.get_blob(tx_hash)
    }

    fn get_all_blobs(
        &self,
        tx_hashes: Vec<TxHash>,
    ) -> Result<
        Vec<(TxHash, Arc<alloy_eips::eip7594::BlobTransactionSidecarVariant>)>,
        BlobStoreError,
    > {
        self.protocol_pool.get_all_blobs(tx_hashes)
    }

    fn get_all_blobs_exact(
        &self,
        tx_hashes: Vec<TxHash>,
    ) -> Result<Vec<Arc<alloy_eips::eip7594::BlobTransactionSidecarVariant>>, BlobStoreError> {
        self.protocol_pool.get_all_blobs_exact(tx_hashes)
    }

    fn get_blobs_for_versioned_hashes_v1(
        &self,
        versioned_hashes: &[B256],
    ) -> Result<Vec<Option<alloy_eips::eip4844::BlobAndProofV1>>, BlobStoreError> {
        self.protocol_pool.get_blobs_for_versioned_hashes_v1(versioned_hashes)
    }

    fn get_blobs_for_versioned_hashes_v2(
        &self,
        versioned_hashes: &[B256],
    ) -> Result<Option<Vec<alloy_eips::eip4844::BlobAndProofV2>>, BlobStoreError> {
        self.protocol_pool.get_blobs_for_versioned_hashes_v2(versioned_hashes)
    }

    fn get_blobs_for_versioned_hashes_v3(
        &self,
        versioned_hashes: &[B256],
    ) -> Result<Vec<Option<alloy_eips::eip4844::BlobAndProofV2>>, BlobStoreError> {
        self.protocol_pool.get_blobs_for_versioned_hashes_v3(versioned_hashes)
    }
}

impl<P, Client, AaV> TransactionPoolExt for OpDualPool<P, Client, AaV>
where
    P: TransactionPoolExt + NotifyAaLifecycle<P::Transaction>,
    P::Transaction: Eip8130PoolTx + Transaction,
    Client: StateProviderFactory + Send + Sync + 'static,
    AaV: AaPoolPreValidator<P::Transaction>,
{
    type Block = P::Block;

    fn on_canonical_state_change(
        &self,
        update: reth_transaction_pool::CanonicalStateUpdate<'_, Self::Block>,
    ) {
        // Storage-diff state updates (nonce/lock/owner_config) flow
        // through `maintain_eip8130_state_future` separately because
        // `CanonicalStateUpdate` doesn't carry storage diffs. The AA-side
        // tip timestamp is owned by the wrapper validator
        // ([`OpAaTransactionValidator::on_new_head_block`]).
        self.protocol_pool.on_canonical_state_change(update);
    }

    fn set_block_info(&self, info: BlockInfo) {
        self.protocol_pool.set_block_info(info)
    }

    fn update_accounts(&self, accounts: Vec<reth_execution_types::ChangedAccount>) {
        self.protocol_pool.update_accounts(accounts)
    }

    fn delete_blob(&self, tx: TxHash) {
        self.protocol_pool.delete_blob(tx)
    }

    fn delete_blobs(&self, txs: Vec<TxHash>) {
        self.protocol_pool.delete_blobs(txs)
    }

    fn cleanup_blobs(&self) {
        self.protocol_pool.cleanup_blobs()
    }
}

// ---------------------------------------------------------------------------
// BundleState helper
// ---------------------------------------------------------------------------

/// Surface to fire `Pending` / `Discarded` events through the protocol
/// pool's existing `transaction_event_listener` / `pending_transactions_listener_for`
/// channels for AA-pool lifecycle transitions. Mirrors the pub
/// `Pool::inner().notify_on_transaction_updates(...)` reth API: a single
/// call dispatches both pending-listener fan-out and event-listener fan-out.
///
/// Why a trait: `OpDualPool<P, _>` is generic over `P: TransactionPool`,
/// but `notify_on_transaction_updates` is a method on reth's specific
/// `Pool<V, T, S>` (via `PoolInner`). The trait pins the connection
/// without leaking the concrete type up.
pub trait NotifyAaLifecycle<T: PoolTransaction> {
    /// Hands a freshly admitted AA tx through the protocol pool's standard
    /// listener fan-out (`pending_transactions_listener_for`,
    /// `new_transactions_listener_for`, per-tx event listeners, and all
    /// transaction event listeners).
    fn notify_aa_added(&self, added: AddedTransaction<T>);

    /// Hands `promoted` (AA tx now executable) and `discarded` (AA tx
    /// removed: invalidated / capacity-evicted / etc.) to the protocol
    /// pool's listener fan-out. Promoted hashes go to the pending-listener
    /// channels and fire `TransactionEvent::Pending`; discarded hashes fire
    /// `TransactionEvent::Discarded`.
    ///
    /// The `mined` set has been split out of `discarded`; use
    /// [`Self::notify_aa_mined`] to surface inclusion events.
    fn notify_aa_lifecycle(
        &self,
        promoted: Vec<Arc<ValidPoolTransaction<T>>>,
        discarded: Vec<Arc<ValidPoolTransaction<T>>>,
    );

    /// Fires `TransactionEvent::Mined(block_hash)` for every tx in
    /// `mined`. Mirrors reth's
    /// `Pool::inner().notify_on_new_state` mined fan-out
    /// (`reth-transaction-pool/src/pool/mod.rs:887-897`) so per-tx event
    /// subscribers can distinguish inclusion from eviction. Default impl
    /// is a no-op so test stubs that don't care about Mined events don't
    /// need to override.
    fn notify_aa_mined(&self, _mined: Vec<Arc<ValidPoolTransaction<T>>>, _block_hash: B256) {}
}

impl<V, O, S> NotifyAaLifecycle<V::Transaction> for Pool<V, O, S>
where
    V: TransactionValidator,
    <V as TransactionValidator>::Transaction: PoolTransaction,
    O: TransactionOrdering<Transaction = V::Transaction>,
    S: BlobStore,
{
    fn notify_aa_added(&self, added: AddedTransaction<V::Transaction>) {
        if let Some(pending) = added.as_pending() {
            self.inner().on_new_pending_transaction(pending);
        }
        self.inner().notify_event_listeners(&added);
        self.inner().on_new_transaction(added.into_new_transaction_event());
    }

    fn notify_aa_lifecycle(
        &self,
        promoted: Vec<Arc<ValidPoolTransaction<V::Transaction>>>,
        discarded: Vec<Arc<ValidPoolTransaction<V::Transaction>>>,
    ) {
        self.inner().notify_on_transaction_updates(promoted, discarded);
    }
}

impl<P, T> NotifyAaLifecycle<T> for OpPool<P>
where
    P: TransactionPool<Transaction = T> + NotifyAaLifecycle<T>,
    T: PoolTransaction + alloy_consensus::Transaction,
{
    fn notify_aa_added(&self, added: AddedTransaction<T>) {
        self.inner().notify_aa_added(added);
    }

    fn notify_aa_lifecycle(
        &self,
        promoted: Vec<Arc<ValidPoolTransaction<T>>>,
        discarded: Vec<Arc<ValidPoolTransaction<T>>>,
    ) {
        self.inner().notify_aa_lifecycle(promoted, discarded);
    }

    fn notify_aa_mined(&self, mined: Vec<Arc<ValidPoolTransaction<T>>>, block_hash: B256) {
        self.inner().notify_aa_mined(mined, block_hash);
    }
}

fn into_reth_added<T: PoolTransaction>(added: crate::Eip8130AddOutcome<T>) -> AddedTransaction<T> {
    match added {
        crate::Eip8130AddOutcome::Pending(pending) => {
            AddedTransaction::Pending(AddedPendingTransaction {
                transaction: pending.transaction,
                replaced: pending.replaced,
                promoted: pending.promoted,
                discarded: pending.discarded,
            })
        }
        crate::Eip8130AddOutcome::Queued { transaction, replaced } => AddedTransaction::Parked {
            transaction,
            replaced,
            subpool: SubPool::Queued,
            queued_reason: Some(QueuedReason::NonceGap),
        },
    }
}

/// Reassembles results from the AA and protocol-pool branches in the
/// caller's original input order. Each branch yields `(input_idx, result)`;
/// the merged output is sorted by `input_idx` so position N of the result
/// vec corresponds to position N of the input vec — the contract callers
/// of `TransactionPool::add_transactions*` rely on for hash/result pairing.
fn merge_routed_results<I, J>(non_aa: I, aa: J) -> Vec<PoolResult<AddedTransactionOutcome>>
where
    I: IntoIterator<Item = (usize, PoolResult<AddedTransactionOutcome>)>,
    J: IntoIterator<Item = (usize, PoolResult<AddedTransactionOutcome>)>,
{
    let mut combined: Vec<(usize, PoolResult<AddedTransactionOutcome>)> =
        non_aa.into_iter().chain(aa).collect();
    combined.sort_by_key(|(idx, _)| *idx);
    combined.into_iter().map(|(_, r)| r).collect()
}

/// Builds a `TransactionEvents` stream backed by the AA pool's per-tx
/// event broadcast. The returned subscription filters by `hash` and exits
/// after a final event.
fn spawn_aa_event_adapter(
    hash: TxHash,
    events_rx: tokio::sync::broadcast::Receiver<(TxHash, TransactionEvent)>,
) -> TransactionEvents {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<TransactionEvent>();
    tokio::spawn(forward_aa_events(hash, events_rx, tx));
    TransactionEvents::new(hash, rx)
}

/// Backing task for [`spawn_aa_event_adapter`]. Filters AA broadcasts by
/// `hash` and pushes the matched events into `out`. Exits after a final
/// event, when `out` is closed, or when the upstream broadcast closes.
async fn forward_aa_events(
    hash: TxHash,
    mut events_rx: tokio::sync::broadcast::Receiver<(TxHash, TransactionEvent)>,
    out: tokio::sync::mpsc::UnboundedSender<TransactionEvent>,
) {
    use tokio::sync::broadcast::error::RecvError;
    loop {
        match events_rx.recv().await {
            Ok((event_hash, event)) if event_hash == hash => {
                let is_final = event.is_final();
                if out.send(event).is_err() || is_final {
                    return;
                }
            }
            Ok(_) => continue,
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => return,
        }
    }
}

/// Drains `inner_rx` (protocol pool's pending listener) and `aa_rx`
/// (AA pool's broadcast) concurrently, forwarding each yielded hash to
/// `out`. Exits when `out` is closed (downstream Receiver dropped).
#[cfg(test)]
async fn forward_pending_listeners(
    mut inner_rx: Receiver<TxHash>,
    mut aa_rx: tokio::sync::broadcast::Receiver<TxHash>,
    out: tokio::sync::mpsc::Sender<TxHash>,
) {
    loop {
        tokio::select! {
            biased;
            inner = inner_rx.recv() => match inner {
                Some(hash) => {
                    if out.send(hash).await.is_err() {
                        return;
                    }
                }
                None => {
                    // Inner pool closed — forward AA-only from now on.
                    while let Ok(hash) = aa_rx.recv().await {
                        if out.send(hash).await.is_err() {
                            return;
                        }
                    }
                    return;
                }
            },
            aa = aa_rx.recv() => match aa {
                Ok(hash) => {
                    if out.send(hash).await.is_err() {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // Slow consumer; just skip the dropped events. AA tx
                    // hashes are best-effort gossip notifications, not
                    // a consistency-critical channel.
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    // AA channel closed — forward inner-only from now on.
                    while let Some(hash) = inner_rx.recv().await {
                        if out.send(hash).await.is_err() {
                            return;
                        }
                    }
                    return;
                }
            },
        }
    }
}

/// Extracts `(addr, slot, present_value)` diffs from a `BundleState` for
/// the two addresses [`Eip8130Pool`] watches:
/// [`STATE_DIFF_ADDRESS`] (nonce / expiring slots) and
/// [`ACCOUNT_CONFIG_ADDRESS`] (lock / owner-config invalidation slots).
/// Returns an empty `Vec` if neither address was touched in the block.
///
/// Pulled out of [`OpDualPool::on_bundle_state`] so the bundle-shape
/// extraction is testable in isolation (no need to construct an
/// `OpDualPool`). Crate-private — only `on_bundle_state` and tests call
/// this; not part of the public surface.
pub(crate) fn bundle_state_aa_diffs(
    bundle: &op_revm::revm::database::states::BundleState,
) -> Vec<(Address, U256, U256)> {
    let mut diffs = Vec::new();
    for addr in [STATE_DIFF_ADDRESS, ACCOUNT_CONFIG_ADDRESS] {
        if let Some(account) = bundle.state.get(&addr) {
            for (slot, slot_value) in account.storage.iter() {
                diffs.push((addr, *slot, slot_value.present_value));
            }
        }
    }
    diffs
}

/// Collects `(address, post_balance)` pairs from the bundle for every
/// account whose balance moved this block. Feeds
/// [`Eip8130Pool::on_balance_updates`] for state-diff-driven payer
/// re-validation (xlayer's analogue of tempo's `Check 3b` insolvent
/// fee_payer eviction at `tempo_pool.rs:267-307`).
///
/// Returns post-state balance (`info.balance`) when present; accounts
/// with `info: None` (selfdestructed / not loaded) are skipped — a
/// missing account at canon-commit time is not a balance change the
/// pool can act on. Pre-state comparison is intentionally omitted: an
/// unchanged balance still costs only an `O(1)` HashMap lookup +
/// predicate check on the pool side, and avoiding the comparison keeps
/// this helper trivially correct even when reth's bundle shape evolves.
///
/// Crate-private — only `on_bundle_state` and tests call this.
///
/// [`Eip8130Pool::on_balance_updates`]: crate::Eip8130Pool::on_balance_updates
pub(crate) fn bundle_state_aa_balances(
    bundle: &op_revm::revm::database::states::BundleState,
) -> Vec<(Address, U256)> {
    let mut balances = Vec::with_capacity(bundle.state.len());
    for (addr, account) in bundle.state.iter() {
        if let Some(info) = account.info.as_ref() {
            balances.push((*addr, info.balance));
        }
    }
    balances
}

// `AA_TX_TYPE_ID` is re-exported from op_alloy_consensus and used by the
// binary's validator setup. Surface it here so consumers don't need to
// pull from op_alloy directly.
#[allow(dead_code)]
const _AA_TX_TYPE_ID_FOR_DOC: u8 = AA_TX_TYPE_ID;
#[cfg(test)]
mod tests {
    use super::*;
    use op_revm::revm::database::{
        AccountStatus, BundleAccount,
        states::{StorageSlot, bundle_state::BundleState},
    };

    fn bundle_with_slot(addr: Address, slot: U256, prev: U256, present: U256) -> BundleState {
        let mut bundle = BundleState::default();
        let mut storage = std::collections::HashMap::default();
        storage.insert(slot, StorageSlot::new_changed(prev, present));
        let acc = BundleAccount::new(None, None, storage, AccountStatus::Changed);
        bundle.state.insert(addr, acc);
        bundle
    }

    #[test]
    fn extracts_nonce_manager_slot_diffs() {
        let slot = U256::from(0x42_u64);
        let bundle = bundle_with_slot(STATE_DIFF_ADDRESS, slot, U256::ZERO, U256::from(7u64));
        let diffs = bundle_state_aa_diffs(&bundle);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0], (STATE_DIFF_ADDRESS, slot, U256::from(7u64)));
    }

    #[test]
    fn extracts_account_config_slot_diffs() {
        let slot = U256::from(0xAB_u64);
        let bundle = bundle_with_slot(ACCOUNT_CONFIG_ADDRESS, slot, U256::ZERO, U256::from(1u64));
        let diffs = bundle_state_aa_diffs(&bundle);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0], (ACCOUNT_CONFIG_ADDRESS, slot, U256::from(1u64)));
    }

    #[test]
    fn ignores_unrelated_addresses() {
        let other = Address::repeat_byte(0x99);
        let bundle = bundle_with_slot(other, U256::ZERO, U256::ZERO, U256::from(1u64));
        assert!(bundle_state_aa_diffs(&bundle).is_empty());
    }

    #[test]
    fn empty_bundle_yields_empty_diffs() {
        let bundle = BundleState::default();
        assert!(bundle_state_aa_diffs(&bundle).is_empty());
    }

    /// Capability: the listener forwarder drains both upstream channels
    /// (protocol pool's mpsc + AA pool's broadcast) into a single
    /// downstream Receiver, preserving every message.
    #[tokio::test]
    async fn listener_forwarder_relays_both_channels() {
        let (inner_tx, inner_rx) = tokio::sync::mpsc::channel::<TxHash>(8);
        let (aa_tx, aa_rx) = tokio::sync::broadcast::channel::<TxHash>(8);
        let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<TxHash>(16);

        let forwarder = tokio::spawn(super::forward_pending_listeners(inner_rx, aa_rx, out_tx));

        // Push 2 from inner pool's channel, 2 from AA broadcast.
        let h1 = TxHash::repeat_byte(0x01);
        let h2 = TxHash::repeat_byte(0x02);
        let h3 = TxHash::repeat_byte(0x03);
        let h4 = TxHash::repeat_byte(0x04);
        inner_tx.send(h1).await.unwrap();
        let _ = aa_tx.send(h3);
        inner_tx.send(h2).await.unwrap();
        let _ = aa_tx.send(h4);
        // Close upstream channels — forwarder should drain pending then
        // exit.
        drop(inner_tx);
        drop(aa_tx);

        let mut got = Vec::new();
        while let Some(h) = out_rx.recv().await {
            got.push(h);
        }
        got.sort();
        let mut want = vec![h1, h2, h3, h4];
        want.sort();
        assert_eq!(got, want, "all four hashes must be forwarded");

        // Ensure the forwarder task exited cleanly.
        let _ = tokio::time::timeout(std::time::Duration::from_millis(100), forwarder)
            .await
            .expect("forwarder must exit when both channels close");
    }

    // ------------------------------------------------------------------
    // Routing / subscription / size-limit tests for OpDualPool itself.
    // ------------------------------------------------------------------

    use crate::{OpPool, OpPooledTransaction};
    use alloy_consensus::{Sealable, transaction::Recovered};
    use alloy_eips::eip2718::Encodable2718;
    use op_alloy_consensus::{Eip8130CallEntry, TxEip8130};
    use op_revm::precompiles_xlayer::NONCE_MANAGER_ADDRESS;
    use reth_optimism_primitives::{OpPrimitives, OpTransactionSigned};
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
    use reth_transaction_pool::{
        CoinbaseTipOrdering, FullTransactionEvent, PoolConfig,
        blobstore::InMemoryBlobStore,
        noop::{MockTransactionValidator, NoopTransactionPool},
    };
    use std::time::Duration;

    type TestInner = NoopTransactionPool<OpPooledTransaction>;
    type TestDualPool = OpDualPool<TestInner, MockEthProvider<OpPrimitives>, AlwaysValidStub>;
    type RealProtocolInner = Pool<
        MockTransactionValidator<OpPooledTransaction>,
        CoinbaseTipOrdering<OpPooledTransaction>,
        InMemoryBlobStore,
    >;
    type RealFanoutDualPool =
        OpDualPool<RealProtocolInner, MockEthProvider<OpPrimitives>, AlwaysValidStub>;

    impl NotifyAaLifecycle<OpPooledTransaction> for NoopTransactionPool<OpPooledTransaction> {
        fn notify_aa_added(&self, _added: AddedTransaction<OpPooledTransaction>) {}

        fn notify_aa_lifecycle(
            &self,
            _promoted: Vec<Arc<ValidPoolTransaction<OpPooledTransaction>>>,
            _discarded: Vec<Arc<ValidPoolTransaction<OpPooledTransaction>>>,
        ) {
        }
    }

    /// Stub validator used by routing tests in this module. Always returns
    /// `Valid` so the pool inserts the tx into the side pool. Routing
    /// tests are intentionally orthogonal to the AA spec layer; the
    /// wrapper validator is exercised end-to-end in
    /// `dual_pool_aa_path_*` integration tests below and in
    /// `crate::aa_validator::tests`.
    #[derive(Debug, Default)]
    struct AlwaysValidStub;

    impl AaPoolPreValidator<OpPooledTransaction> for AlwaysValidStub {
        async fn validate(
            &self,
            origin: TransactionOrigin,
            tx: OpPooledTransaction,
        ) -> TransactionValidationOutcome<OpPooledTransaction> {
            // Need a state_nonce + balance; use 0/MAX so the side pool
            // doesn't reject as stale.
            TransactionValidationOutcome::Valid {
                balance: U256::MAX,
                state_nonce: 0,
                bytecode_hash: None,
                transaction: ValidTransaction::Valid(tx),
                propagate: matches!(origin, TransactionOrigin::External | TransactionOrigin::Local),
                authorities: None,
            }
        }

        fn compute_l1_data_fee(&self, _tx: &OpPooledTransaction) -> U256 {
            // Routing-only tests don't drive `OpL1BlockInfo`; ZERO matches
            // the dev-mode wiring path. F5 dual-pool tests rely on
            // `gas_limit * max_fee_per_gas` alone being the predicate.
            U256::ZERO
        }
    }

    /// Builds a self-pay AA tx whose validator path passes against a
    /// `fresh_client(sender)` provider. Each `nonce_sequence` value yields
    /// a distinct envelope hash (the tx's signature hash includes the
    /// sequence) so callers can build batches without dedup collisions.
    fn make_aa_tx(sender: Address, nonce_sequence: u64) -> OpPooledTransaction {
        let tx = TxEip8130 {
            chain_id: 10,
            from: Some(sender),
            nonce_key: U256::ZERO,
            nonce_sequence,
            expiry: 0,
            max_priority_fee_per_gas: 1,
            max_fee_per_gas: 2,
            gas_limit: 100_000,
            calls: vec![vec![Eip8130CallEntry {
                to: Address::repeat_byte(0xAA),
                data: Default::default(),
            }]],
            ..Default::default()
        };
        let signed: OpTransactionSigned = tx.seal_slow().into();
        let recovered = Recovered::new_unchecked(signed, sender);
        let len = recovered.encode_2718_len();
        OpPooledTransaction::new(recovered, len)
    }

    /// Builds a nonce-free AA tx (`nonce_key == NONCE_KEY_MAX`). Different
    /// expiry values produce distinct sender-signature hashes, so multiple
    /// txs can coexist in the expiring-nonce side map.
    fn make_expiring_aa_tx(sender: Address, expiry: u64) -> OpPooledTransaction {
        let tx = TxEip8130 {
            chain_id: 10,
            from: Some(sender),
            nonce_key: op_revm::handler::NONCE_KEY_MAX,
            nonce_sequence: 0,
            expiry,
            max_priority_fee_per_gas: 1,
            max_fee_per_gas: 2,
            gas_limit: 100_000,
            calls: vec![vec![Eip8130CallEntry {
                to: Address::repeat_byte(0xAA),
                data: Default::default(),
            }]],
            ..Default::default()
        };
        let signed: OpTransactionSigned = tx.seal_slow().into();
        let recovered = Recovered::new_unchecked(signed, sender);
        let len = recovered.encode_2718_len();
        OpPooledTransaction::new(recovered, len)
    }

    /// Builds a non-AA pooled tx (legacy envelope) for routing-order tests.
    /// The protocol pool (`NoopTransactionPool`) will reject it with
    /// `NoopInsertError`; we only care that the result lands at the right
    /// position in the result vec.
    fn make_non_aa_tx(seed: u8) -> OpPooledTransaction {
        use alloy_consensus::{Signed, TxLegacy};
        use alloy_primitives::Signature;
        let tx = TxLegacy {
            chain_id: Some(10),
            nonce: 0,
            gas_price: 1,
            gas_limit: 21_000,
            to: alloy_primitives::TxKind::Call(Address::repeat_byte(seed)),
            value: U256::from(seed as u64),
            input: Default::default(),
        };
        // Dummy signature — the noop pool doesn't recover it.
        let sig = Signature::new(U256::from(1u64), U256::from(1u64), false);
        let signed: Signed<TxLegacy> = Signed::new_unchecked(tx, sig, B256::repeat_byte(seed));
        let consensus: OpTransactionSigned = signed.into();
        let recovered = Recovered::new_unchecked(consensus, Address::repeat_byte(seed));
        let len = recovered.encode_2718_len();
        OpPooledTransaction::new(recovered, len)
    }

    /// Provider seeded with the NONCE_MANAGER + the supplied sender so
    /// `validate_eip8130_transaction` sees `aa_nonce_slot(sender, 0) == 0`
    /// and a balance > tx.cost.
    fn fresh_client(sender: Address) -> MockEthProvider<OpPrimitives> {
        let client = MockEthProvider::<OpPrimitives>::new();
        client.add_account(NONCE_MANAGER_ADDRESS, ExtendedAccount::new(0, U256::ZERO));
        client.add_account(sender, ExtendedAccount::new(0, U256::from(1_000_000_000_u64)));
        client
    }

    fn make_dual_pool(client: MockEthProvider<OpPrimitives>) -> TestDualPool {
        let inner: TestInner = NoopTransactionPool::new();
        let op_pool = OpPool::new(inner, false);
        OpDualPool::with_config(
            op_pool,
            Arc::new(client),
            Arc::new(AlwaysValidStub),
            Default::default(),
        )
    }

    fn make_real_fanout_dual_pool(client: MockEthProvider<OpPrimitives>) -> RealFanoutDualPool {
        let inner: RealProtocolInner = Pool::new(
            MockTransactionValidator::default(),
            CoinbaseTipOrdering::default(),
            InMemoryBlobStore::default(),
            PoolConfig::default(),
        );
        let op_pool = OpPool::new(inner, false);
        OpDualPool::with_config(
            op_pool,
            Arc::new(client),
            Arc::new(AlwaysValidStub),
            Default::default(),
        )
    }

    /// Issue 1: `add_transaction_and_subscribe` must route AA txs to the
    /// side pool (not silently delegate to the protocol pool, where they'd
    /// bypass 2D-nonce admission) AND return a working subscription that
    /// observes the synchronously-fired Pending event.
    #[tokio::test]
    async fn add_transaction_and_subscribe_routes_aa_to_side_pool() {
        use futures_util::StreamExt;

        let sender = Address::repeat_byte(0x33);
        let pool = make_dual_pool(fresh_client(sender));
        let tx = make_aa_tx(sender, 0);
        let hash = *tx.hash();

        let mut events = pool
            .add_transaction_and_subscribe(TransactionOrigin::Local, tx)
            .await
            .expect("AA admission via subscribe path must succeed");

        // AA tx is in the side pool.
        assert!(pool.aa_pool.read().contains(&hash), "AA tx must be admitted to the side pool");

        // Subscription receives `Pending` for the synchronously-pending tx.
        // The adapter task is spawned on tokio, so give it a beat to drain
        // the broadcast.
        let next = tokio::time::timeout(Duration::from_millis(200), events.next())
            .await
            .expect("Pending event must arrive on the AA subscription");
        assert!(matches!(next, Some(TransactionEvent::Pending)));
    }

    /// Issue 2: AA per-tx subscriptions must surface the same lifecycle
    /// shape as reth's regular pool, including queued admission when a
    /// lane has a nonce gap.
    #[tokio::test]
    async fn add_transaction_and_subscribe_surfaces_queued_aa_event() {
        use futures_util::StreamExt;

        let sender = Address::repeat_byte(0x34);
        let pool = make_dual_pool(fresh_client(sender));
        let tx = make_aa_tx(sender, 1);

        let mut events = pool
            .add_transaction_and_subscribe(TransactionOrigin::Local, tx)
            .await
            .expect("queued AA admission via subscribe path must succeed");

        let next = tokio::time::timeout(Duration::from_millis(200), events.next())
            .await
            .expect("Queued event must arrive on the AA subscription");
        assert!(matches!(next, Some(TransactionEvent::Queued)));
    }

    #[tokio::test]
    async fn aa_pending_admission_fans_out_through_protocol_pool_listeners() {
        use futures_util::StreamExt;

        let sender = Address::repeat_byte(0xA1);
        let pool = make_real_fanout_dual_pool(fresh_client(sender));
        let tx = make_aa_tx(sender, 0);
        let hash = *tx.hash();

        let mut pending_rx = pool.pending_transactions_listener_for(TransactionListenerKind::All);
        let mut new_rx = pool.new_transactions_listener_for(TransactionListenerKind::All);
        let mut all_events = pool.all_transactions_event_listener();

        pool.add_transaction(TransactionOrigin::Local, tx)
            .await
            .expect("AA admission must succeed");

        let pending_hash = tokio::time::timeout(Duration::from_millis(200), pending_rx.recv())
            .await
            .expect("pending hash must be fanned out")
            .expect("pending listener must stay open");
        assert_eq!(pending_hash, hash);

        let full_tx = tokio::time::timeout(Duration::from_millis(200), new_rx.recv())
            .await
            .expect("new tx event must be fanned out")
            .expect("new transaction listener must stay open");
        assert_eq!(full_tx.subpool, SubPool::Pending);
        assert_eq!(*full_tx.transaction.hash(), hash);

        let event = tokio::time::timeout(Duration::from_millis(200), all_events.next())
            .await
            .expect("all transaction event must be fanned out");
        assert!(
            matches!(event, Some(FullTransactionEvent::Pending(event_hash)) if event_hash == hash)
        );
    }

    #[tokio::test]
    async fn aa_queued_admission_fans_out_protocol_queued_event() {
        use futures_util::StreamExt;

        let sender = Address::repeat_byte(0xA2);
        let pool = make_real_fanout_dual_pool(fresh_client(sender));
        let tx = make_aa_tx(sender, 1);
        let hash = *tx.hash();

        let mut pending_rx = pool.pending_transactions_listener_for(TransactionListenerKind::All);
        let mut new_rx = pool.new_transactions_listener_for(TransactionListenerKind::All);
        let mut all_events = pool.all_transactions_event_listener();

        pool.add_transaction(TransactionOrigin::Local, tx)
            .await
            .expect("queued AA admission must succeed");

        assert!(
            pending_rx.try_recv().is_err(),
            "queued tx must not be sent to pending hash listeners"
        );

        let full_tx = tokio::time::timeout(Duration::from_millis(200), new_rx.recv())
            .await
            .expect("queued new tx event must be fanned out")
            .expect("new transaction listener must stay open");
        assert_eq!(full_tx.subpool, SubPool::Queued);
        assert_eq!(*full_tx.transaction.hash(), hash);

        let event = tokio::time::timeout(Duration::from_millis(200), all_events.next())
            .await
            .expect("queued all transaction event must be fanned out");
        assert!(
            matches!(
                event,
                Some(FullTransactionEvent::Queued(event_hash, Some(QueuedReason::NonceGap)))
                    if event_hash == hash
            ),
            "queued AA tx must surface as a protocol Queued event"
        );
    }

    #[tokio::test]
    async fn aa_private_admission_respects_protocol_propagate_filtering() {
        let sender = Address::repeat_byte(0xA3);
        let pool = make_real_fanout_dual_pool(fresh_client(sender));
        let tx = make_aa_tx(sender, 0);
        let hash = *tx.hash();

        let mut pending_all = pool.pending_transactions_listener_for(TransactionListenerKind::All);
        let mut pending_propagate =
            pool.pending_transactions_listener_for(TransactionListenerKind::PropagateOnly);
        let mut new_all = pool.new_transactions_listener_for(TransactionListenerKind::All);
        let mut new_propagate =
            pool.new_transactions_listener_for(TransactionListenerKind::PropagateOnly);

        pool.add_transaction(TransactionOrigin::Private, tx)
            .await
            .expect("private AA admission must succeed");

        let pending_hash = tokio::time::timeout(Duration::from_millis(200), pending_all.recv())
            .await
            .expect("all pending listener must see private AA tx")
            .expect("pending listener must stay open");
        assert_eq!(pending_hash, hash);
        assert!(
            pending_propagate.try_recv().is_err(),
            "propagate-only pending listener must filter private AA tx"
        );

        let full_tx = tokio::time::timeout(Duration::from_millis(200), new_all.recv())
            .await
            .expect("all new tx listener must see private AA tx")
            .expect("new transaction listener must stay open");
        assert_eq!(*full_tx.transaction.hash(), hash);
        assert!(
            new_propagate.try_recv().is_err(),
            "propagate-only new tx listener must filter private AA tx"
        );
    }

    #[tokio::test]
    async fn aa_removed_tx_fans_out_discarded_through_protocol_all_events() {
        use futures_util::StreamExt;

        let sender = Address::repeat_byte(0xA4);
        let pool = make_real_fanout_dual_pool(fresh_client(sender));
        let tx = make_aa_tx(sender, 0);
        let hash = *tx.hash();
        let mut all_events = pool.all_transactions_event_listener();

        pool.add_transaction(TransactionOrigin::Local, tx)
            .await
            .expect("AA admission must succeed");
        let added = tokio::time::timeout(Duration::from_millis(200), all_events.next())
            .await
            .expect("pending event must be fanned out before removal");
        assert!(
            matches!(added, Some(FullTransactionEvent::Pending(event_hash)) if event_hash == hash)
        );

        let removed = pool.remove_aa_transactions(&[hash]);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].hash(), &hash);

        let discarded = tokio::time::timeout(Duration::from_millis(200), all_events.next())
            .await
            .expect("discarded event must be fanned out after removal");
        assert!(
            matches!(discarded, Some(FullTransactionEvent::Discarded(event_hash)) if event_hash == hash)
        );
    }

    /// Generic `TransactionPool::remove_transactions` (the standard
    /// reth API, distinct from the AA-specific `remove_aa_transactions`)
    /// must also fan AA discards out through the protocol pool's
    /// listener channels. Pre-fix the AA pool's own broadcast fired but
    /// `pending_transactions_listener_for` / `transaction_event_listener`
    /// consumers (the canonical reth surface) never saw AA evictions
    /// triggered through the generic remove API.
    #[tokio::test]
    async fn remove_transactions_fans_out_to_protocol_pool_listeners() {
        use futures_util::StreamExt;
        use reth_transaction_pool::TransactionPool;

        let sender = Address::repeat_byte(0xA5);
        let pool = make_real_fanout_dual_pool(fresh_client(sender));
        let tx = make_aa_tx(sender, 0);
        let hash = *tx.hash();
        let mut all_events = pool.all_transactions_event_listener();

        pool.add_transaction(TransactionOrigin::Local, tx)
            .await
            .expect("AA admission must succeed");
        let added = tokio::time::timeout(Duration::from_millis(200), all_events.next())
            .await
            .expect("pending event must be fanned out on admission");
        assert!(
            matches!(added, Some(FullTransactionEvent::Pending(event_hash)) if event_hash == hash)
        );

        // Use the generic `TransactionPool::remove_transactions` API,
        // not the AA-specific helper. Pre-fix this path bypassed the
        // `notify_aa_lifecycle` fan-out and silently ate the discard.
        let removed = TransactionPool::remove_transactions(&pool, vec![hash]);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].hash(), &hash);

        let discarded = tokio::time::timeout(Duration::from_millis(200), all_events.next())
            .await
            .expect("discarded event must be fanned out via the generic remove API");
        assert!(
            matches!(discarded, Some(FullTransactionEvent::Discarded(event_hash)) if event_hash == hash),
        );
    }

    /// Issue 3: descendant removal for AA lanes should match tempo:
    /// removing nonce N removes N and every higher nonce in the same
    /// `(sender, nonce_key)` lane, while lower nonces remain.
    #[test]
    fn remove_transactions_and_descendants_removes_aa_lane_suffix() {
        let sender = Address::repeat_byte(0x35);
        let pool = make_dual_pool(fresh_client(sender));
        let tx0 = make_aa_tx(sender, 0);
        let tx1 = make_aa_tx(sender, 1);
        let tx2 = make_aa_tx(sender, 2);
        let h0 = *tx0.hash();
        let h1 = *tx1.hash();
        let h2 = *tx2.hash();

        let rt = tokio::runtime::Builder::new_current_thread().build().expect("rt");
        rt.block_on(async {
            pool.admit_aa(TransactionOrigin::Local, tx0).await.expect("admit 0");
            pool.admit_aa(TransactionOrigin::Local, tx1).await.expect("admit 1");
            pool.admit_aa(TransactionOrigin::Local, tx2).await.expect("admit 2");
        });

        let removed = pool.remove_transactions_and_descendants(vec![h1]);
        let removed_hashes: std::collections::HashSet<_> =
            removed.iter().map(|tx| *tx.hash()).collect();

        assert_eq!(removed_hashes.len(), 2);
        assert!(removed_hashes.contains(&h1));
        assert!(removed_hashes.contains(&h2));
        assert!(pool.get(&h0).is_some(), "lower nonce must remain");
        assert!(pool.get(&h1).is_none());
        assert!(pool.get(&h2).is_none());
    }

    /// Issue 3b: nonce-free AA txs are independent expiring-nonce entries,
    /// not descendants in a shared `nonce_key == MAX` lane.
    #[test]
    fn remove_transactions_and_descendants_keeps_other_expiring_nonce_txs() {
        let sender = Address::repeat_byte(0x37);
        let pool = make_dual_pool(fresh_client(sender));
        let tx0 = make_expiring_aa_tx(sender, 1_000_000);
        let tx1 = make_expiring_aa_tx(sender, 1_000_001);
        let h0 = *tx0.hash();
        let h1 = *tx1.hash();

        let rt = tokio::runtime::Builder::new_current_thread().build().expect("rt");
        rt.block_on(async {
            pool.admit_aa(TransactionOrigin::Local, tx0).await.expect("admit expiring 0");
            pool.admit_aa(TransactionOrigin::Local, tx1).await.expect("admit expiring 1");
        });

        let removed = pool.remove_transactions_and_descendants(vec![h0]);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].hash(), &h0);
        assert!(pool.get(&h0).is_none());
        assert!(pool.get(&h1).is_some(), "other expiring-nonce tx must remain");
    }

    /// Issue 4: P2P pooled views must not leak transactions whose
    /// validation outcome requested `propagate = false`.
    #[test]
    fn pooled_views_filter_non_propagating_aa_transactions() {
        let sender = Address::repeat_byte(0x36);
        let pool = make_dual_pool(fresh_client(sender));
        let tx = make_aa_tx(sender, 0);
        let hash = *tx.hash();

        let rt = tokio::runtime::Builder::new_current_thread().build().expect("rt");
        rt.block_on(async {
            pool.admit_aa(TransactionOrigin::Private, tx).await.expect("private AA admit");
        });

        assert!(
            pool.all_transaction_hashes().contains(&hash),
            "private AA tx is still pooled internally"
        );
        assert!(
            !pool.pooled_transaction_hashes().contains(&hash),
            "P2P hash view must filter propagate=false"
        );
        assert!(
            pool.get_pooled_transaction_element(hash).is_none(),
            "P2P body view must filter propagate=false"
        );
    }

    /// Issue 2: `get_pooled_transaction_elements` must respect the size
    /// limit cumulatively across both pools. Pre-fix the protocol pool
    /// returned its full quota and AA was appended unconditionally; with
    /// the fix the AA branch stops once `accumulated_size + candidate >
    /// limit`.
    #[test]
    fn get_pooled_transaction_elements_respects_size_limit() {
        let sender = Address::repeat_byte(0x55);
        let pool = make_dual_pool(fresh_client(sender));

        // Admit two AA txs so we have multiple candidates for the size cap
        // to reject.
        let tx0 = make_aa_tx(sender, 0);
        let tx1 = make_aa_tx(sender, 1);
        let h0 = *tx0.hash();
        let h1 = *tx1.hash();
        let len0 = tx0.encoded_length();
        let len1 = tx1.encoded_length();
        // `admit_aa` is async (the wrapper validator runs in async); these
        // two calls are part of a `#[test]` test and we only need a sync
        // executor to drive them. `tokio::runtime::Runtime` keeps the
        // existing fixture's plumbing minimal.
        let rt = tokio::runtime::Builder::new_current_thread().build().expect("rt");
        rt.block_on(async {
            pool.admit_aa(TransactionOrigin::Local, tx0).await.expect("admit 0");
            pool.admit_aa(TransactionOrigin::Local, tx1).await.expect("admit 1");
        });

        // Limit just under one tx's encoded size — nothing fits.
        let tight = GetPooledTransactionLimit::ResponseSizeSoftLimit(len0.saturating_sub(1));
        let got = pool.get_pooled_transaction_elements(vec![h0, h1], tight);
        assert!(got.is_empty(), "limit smaller than the smallest tx must yield no results");

        // Limit fits exactly the first tx but not both. We expect one tx.
        let single = GetPooledTransactionLimit::ResponseSizeSoftLimit(len0);
        let got = pool.get_pooled_transaction_elements(vec![h0, h1], single);
        assert_eq!(got.len(), 1, "single-tx-budget limit must stop after the first AA tx");

        // Limit fits both — should include both.
        let both = GetPooledTransactionLimit::ResponseSizeSoftLimit(len0 + len1);
        let got = pool.get_pooled_transaction_elements(vec![h0, h1], both);
        assert_eq!(got.len(), 2, "limit fitting both must include both");

        // No-limit baseline: both included regardless of order.
        let unlimited = GetPooledTransactionLimit::None;
        let got = pool.get_pooled_transaction_elements(vec![h0, h1], unlimited);
        assert_eq!(got.len(), 2);
    }

    /// Issue 3: `add_transactions` results must align with input positions.
    /// We submit `[non_aa, aa, non_aa, aa]`; AA admissions succeed, non-AA
    /// admissions fail (NoopTransactionPool returns errors). The merged
    /// result vec must alternate `[Err, Ok, Err, Ok]`, with each Ok hash
    /// matching the AA tx at the same input index.
    #[tokio::test]
    async fn add_transactions_preserves_input_order() {
        let sender = Address::repeat_byte(0x77);
        let pool = make_dual_pool(fresh_client(sender));

        let aa0 = make_aa_tx(sender, 0);
        let aa1 = make_aa_tx(sender, 1);
        let non0 = make_non_aa_tx(0xA1);
        let non1 = make_non_aa_tx(0xA2);
        let aa0_hash = *aa0.hash();
        let aa1_hash = *aa1.hash();

        let batch = vec![non0, aa0, non1, aa1];
        let results = pool.add_transactions(TransactionOrigin::External, batch).await;

        assert_eq!(results.len(), 4, "one result per input");
        assert!(results[0].is_err(), "non-AA at index 0 must error from noop pool");
        assert!(results[1].is_ok(), "AA at index 1 must succeed");
        assert_eq!(
            results[1].as_ref().unwrap().hash,
            aa0_hash,
            "result hash at index 1 must match aa0",
        );
        assert!(results[2].is_err(), "non-AA at index 2 must error from noop pool");
        assert!(results[3].is_ok(), "AA at index 3 must succeed");
        assert_eq!(
            results[3].as_ref().unwrap().hash,
            aa1_hash,
            "result hash at index 3 must match aa1",
        );
    }

    /// Batch AA admission must validate concurrently. A stub validator
    /// that sleeps per `validate` call surfaces the parallelism: wall-clock
    /// time for an N-batch should be ~1× single-validate, not N×. We assert
    /// `elapsed < 0.5 × N × delay` — generous slack on the upper bound,
    /// well below the serial worst case.
    #[tokio::test]
    async fn add_transactions_validates_aa_concurrently() {
        use std::time::Duration;
        // 25ms per validate. N=8 ⇒ serial ≥ 200ms, parallel ≈ 25ms.
        const PER_VALIDATE_MS: u64 = 25;
        const BATCH: usize = 8;

        #[derive(Debug, Default)]
        struct SlowStub;
        impl AaPoolPreValidator<OpPooledTransaction> for SlowStub {
            async fn validate(
                &self,
                origin: TransactionOrigin,
                tx: OpPooledTransaction,
            ) -> TransactionValidationOutcome<OpPooledTransaction> {
                tokio::time::sleep(Duration::from_millis(PER_VALIDATE_MS)).await;
                TransactionValidationOutcome::Valid {
                    balance: U256::MAX,
                    state_nonce: 0,
                    bytecode_hash: None,
                    transaction: ValidTransaction::Valid(tx),
                    propagate: matches!(
                        origin,
                        TransactionOrigin::External | TransactionOrigin::Local,
                    ),
                    authorities: None,
                }
            }
            fn compute_l1_data_fee(&self, _tx: &OpPooledTransaction) -> U256 {
                U256::ZERO
            }
        }

        let sender = Address::repeat_byte(0x88);
        let client = fresh_client(sender);
        let inner: TestInner = NoopTransactionPool::new();
        let op_pool = OpPool::new(inner, false);
        let pool = OpDualPool::with_config(
            op_pool,
            Arc::new(client),
            Arc::new(SlowStub),
            Eip8130PoolConfig::default(),
        );

        // Distinct nonce_sequence per tx so hashes don't collide.
        let batch: Vec<_> = (0..BATCH as u64).map(|i| make_aa_tx(sender, i)).collect();

        let start = std::time::Instant::now();
        let results = pool.add_transactions(TransactionOrigin::External, batch).await;
        let elapsed = start.elapsed();

        assert_eq!(results.len(), BATCH);
        for (i, r) in results.iter().enumerate() {
            assert!(r.is_ok(), "tx {i} must admit, got {:?}", r.as_ref().err());
        }

        // Concurrency upper bound: half the serial cost. Serial would be
        // N × PER_VALIDATE_MS; concurrent should be near 1× (plus
        // side-pool insert serial cost, which is microseconds).
        let serial_bound = Duration::from_millis(PER_VALIDATE_MS * BATCH as u64);
        assert!(
            elapsed < serial_bound / 2,
            "expected parallel admission < {:?}, took {:?}",
            serial_bound / 2,
            elapsed,
        );
    }

    // ------------------------------------------------------------------
    // End-to-end checks that the unified wrapper validator is wired into
    // the AA admission path: AA txs go through reth's standard mempool
    // gates (size, gas limit, intrinsic gas, ...) AND the EIP-8130 spec
    // layer.
    // ------------------------------------------------------------------

    use crate::{OpAaTransactionValidator, OpTransactionValidator};
    use reth_optimism_chainspec::{OpChainSpec, OpChainSpecBuilder};
    use reth_optimism_evm::OpEvmConfig;
    use reth_transaction_pool::validate::EthTransactionValidatorBuilder;

    /// Chain id of the test chain spec (base mainnet activated through
    /// XLayerV1).
    const E2E_CHAIN_ID: u64 = 8453;

    /// Builds an AA tx with arbitrary `gas_limit`, `expiry`, and matching
    /// chain id for the e2e fixtures below.
    fn make_aa_tx_e2e(
        sender: Address,
        nonce_sequence: u64,
        gas_limit: u64,
        expiry: u64,
    ) -> OpPooledTransaction {
        let tx = TxEip8130 {
            chain_id: E2E_CHAIN_ID,
            from: Some(sender),
            nonce_key: U256::ZERO,
            nonce_sequence,
            expiry,
            max_priority_fee_per_gas: 1,
            max_fee_per_gas: 2,
            gas_limit,
            calls: vec![vec![Eip8130CallEntry {
                to: Address::repeat_byte(0xAA),
                data: Default::default(),
            }]],
            ..Default::default()
        };
        let signed: OpTransactionSigned = tx.seal_slow().into();
        let recovered = Recovered::new_unchecked(signed, sender);
        let len = recovered.encode_2718_len();
        OpPooledTransaction::new(recovered, len)
    }

    type E2eProvider = MockEthProvider<OpPrimitives, Arc<OpChainSpec>>;
    type E2eWrapper = OpAaTransactionValidator<
        OpTransactionValidator<E2eProvider, OpPooledTransaction, OpEvmConfig>,
        E2eProvider,
    >;
    type E2eDualPool =
        OpDualPool<NoopTransactionPool<OpPooledTransaction>, E2eProvider, E2eWrapper>;

    fn build_e2e_pool(sender: Address) -> E2eDualPool {
        let chain_spec = Arc::new(
            OpChainSpecBuilder::base_mainnet().ecotone_activated().xlayer_v1_activated().build(),
        );
        let client = MockEthProvider::<OpPrimitives>::new()
            .with_chain_spec(chain_spec.clone())
            .with_genesis_block();
        client.add_account(NONCE_MANAGER_ADDRESS, ExtendedAccount::new(0, U256::ZERO));
        client.add_account(sender, ExtendedAccount::new(0, U256::from(1_000_000_000_u64)));

        let evm_config = OpEvmConfig::optimism(chain_spec.clone());
        let inner = EthTransactionValidatorBuilder::new(client.clone(), evm_config)
            .no_eip4844()
            .with_custom_tx_type(op_alloy_consensus::AA_TX_TYPE_ID)
            .build(InMemoryBlobStore::default());
        let inner = OpTransactionValidator::new(inner).require_l1_data_gas_fee(false);
        let wrapper = OpAaTransactionValidator::new(inner, Arc::new(client.clone()));
        let validator = Arc::new(wrapper);

        let inner_pool: NoopTransactionPool<OpPooledTransaction> = NoopTransactionPool::new();
        let op_pool = OpPool::new(inner_pool, false);
        OpDualPool::with_config(op_pool, Arc::new(client), validator, Default::default())
    }

    /// Pins that the dual pool's AA admission runs reth's standard mempool
    /// gates: build an AA tx whose `gas_limit` exceeds the genesis block
    /// gas limit, then assert the dual pool rejects it before it reaches
    /// the side pool. Pre-refactor `admit_aa` skipped these checks.
    #[tokio::test]
    async fn dual_pool_aa_path_runs_common_validation() {
        let sender = Address::repeat_byte(0xC1);
        let pool = build_e2e_pool(sender);

        let oversized =
            make_aa_tx_e2e(sender, 0, /* gas_limit= */ u64::MAX / 4, /* expiry= */ 0);
        let hash = *oversized.hash();
        let err = pool
            .add_transaction(TransactionOrigin::External, oversized)
            .await
            .expect_err("AA tx with oversized gas_limit must be rejected by inner");

        // Side pool stays empty.
        assert!(
            !pool.aa_pool.read().contains(&hash),
            "rejected AA tx must not land in the side pool"
        );

        // Error must come from reth's std validator (gas-limit / size /
        // intrinsic-gas family), not the AA spec layer.
        let msg = err.to_string();
        assert!(
            !msg.contains("EIP-8130"),
            "rejection should originate from reth std validator, got: {msg}"
        );
    }

    /// Pins that the dual pool's AA admission still runs the EIP-8130
    /// spec layer on top of the inner validator: build an AA tx that
    /// passes inner gates but is expired per the spec; assert rejection
    /// with the spec error.
    #[tokio::test]
    async fn dual_pool_aa_path_still_runs_spec_check() {
        let sender = Address::repeat_byte(0xC2);
        let pool = build_e2e_pool(sender);

        // Drive the wrapper validator's tip timestamp forward so the AA
        // tx's `expiry=50` is in the past.
        pool.aa_validator.tip_timestamp_for_test_set(100);

        let expired =
            make_aa_tx_e2e(sender, 0, /* gas_limit= */ 100_000, /* expiry= */ 50);
        let hash = *expired.hash();
        let err = pool
            .add_transaction(TransactionOrigin::External, expired)
            .await
            .expect_err("expired AA tx must be rejected by the spec layer");

        assert!(
            !pool.aa_pool.read().contains(&hash),
            "rejected AA tx must not land in the side pool"
        );

        let msg = err.to_string();
        assert!(msg.contains("expired"), "expected EIP-8130 expired error, got: {msg}");
    }

    /// Capability: applying tempo's `EVICTION_BUFFER_SECS` (3) to the
    /// sweep horizon evicts a tx that would expire on the next
    /// block-builder slot. With `tip_timestamp = 100` and `tx.expiry = 102`,
    /// the unbuffered sweep `sweep_aa_expired(100)` would keep the tx
    /// (`expiry > now`) but the loop's buffered `sweep_aa_expired(100 + 3)`
    /// drops it before peers reject it.
    #[test]
    fn sweep_uses_eviction_buffer() {
        use crate::maintain::AA_EVICTION_BUFFER_SECS;
        assert_eq!(AA_EVICTION_BUFFER_SECS, 3, "tempo parity");

        let sender = Address::repeat_byte(0xE0);
        let pool = make_dual_pool(fresh_client(sender));
        // tx.expiry = tip_timestamp + 2 — within the 3-second buffer.
        let tx = make_expiring_aa_tx(sender, 102);
        let hash = *tx.hash();

        let rt = tokio::runtime::Builder::new_current_thread().build().expect("rt");
        rt.block_on(async {
            pool.admit_aa(TransactionOrigin::Local, tx).await.expect("admit expiring");
        });
        assert!(pool.aa_pool.read().contains(&hash));

        let tip_timestamp = 100u64;
        // Without the buffer the tx survives (`expiry > now`).
        let unbuffered = pool.sweep_aa_expired(tip_timestamp);
        assert!(unbuffered.is_empty(), "sweep at exactly tip must not drop tx with expiry > tip");
        assert!(pool.aa_pool.read().contains(&hash));

        // With the buffer (matches the maintain loop's behavior) the tx
        // is dropped before it would otherwise be re-broadcast.
        let buffered = pool.sweep_aa_expired(tip_timestamp.saturating_add(AA_EVICTION_BUFFER_SECS));
        assert_eq!(buffered.len(), 1, "buffered sweep must evict near-expiry tx");
        assert_eq!(buffered[0].hash(), &hash);
        assert!(!pool.aa_pool.read().contains(&hash));
    }

    /// `aa_pending_tx_hashes` snapshot powers the staleness tracker; it
    /// must surface every pending AA tx exactly once.
    #[test]
    fn aa_pending_tx_hashes_returns_pending_set() {
        let sender = Address::repeat_byte(0xE1);
        let pool = make_dual_pool(fresh_client(sender));
        let tx0 = make_aa_tx(sender, 0);
        let tx1 = make_aa_tx(sender, 1);
        let h0 = *tx0.hash();
        let h1 = *tx1.hash();

        let rt = tokio::runtime::Builder::new_current_thread().build().expect("rt");
        rt.block_on(async {
            pool.admit_aa(TransactionOrigin::Local, tx0).await.expect("admit 0");
            pool.admit_aa(TransactionOrigin::Local, tx1).await.expect("admit 1");
        });

        let snapshot = pool.aa_pending_tx_hashes();
        assert_eq!(snapshot.len(), 2);
        assert!(snapshot.contains(&h0));
        assert!(snapshot.contains(&h1));
    }

    /// `remove_aa_transactions` drops the listed hashes from the side
    /// pool and fires `Discarded` on the AA event broadcaster — the
    /// staleness path relies on this for downstream gossip / RPC parity.
    #[test]
    fn remove_aa_transactions_fires_discarded() {
        let sender = Address::repeat_byte(0xE2);
        let pool = make_dual_pool(fresh_client(sender));
        let tx = make_aa_tx(sender, 0);
        let hash = *tx.hash();
        let rt = tokio::runtime::Builder::new_current_thread().build().expect("rt");
        let mut discarded_rx = pool.aa_pool.read().subscribe_discarded();
        rt.block_on(async {
            pool.admit_aa(TransactionOrigin::Local, tx).await.expect("admit");
        });
        assert!(pool.aa_pool.read().contains(&hash));

        let removed = pool.remove_aa_transactions(&[hash]);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].hash(), &hash);
        assert!(!pool.aa_pool.read().contains(&hash));
        // Discarded broadcast must have fired exactly once for the hash.
        assert_eq!(discarded_rx.try_recv(), Ok(hash));
    }

    /// Capability: a `BundleState` carrying a payer-balance drop drives
    /// the side pool's insolvent-eviction path through `on_bundle_state`,
    /// and the resulting eviction fires a `Discarded` lifecycle event on
    /// the AA pool's broadcast channel. Mirrors tempo's `Check 3b`
    /// canon-commit eviction (`tempo_pool.rs:267-307`), routed via reth's
    /// `BundleState` shape.
    #[test]
    fn on_bundle_state_evicts_insolvent_payer() {
        use op_revm::revm::{
            database::states::bundle_state::BundleState as RevmBundleState, state::AccountInfo,
        };

        let sender = Address::repeat_byte(0xE3);
        let pool = make_dual_pool(fresh_client(sender));
        let tx = make_aa_tx(sender, 0);
        let hash = *tx.hash();
        let mut discarded_rx = pool.aa_pool.read().subscribe_discarded();

        let rt = tokio::runtime::Builder::new_current_thread().build().expect("rt");
        rt.block_on(async {
            pool.admit_aa(TransactionOrigin::Local, tx).await.expect("admit");
        });
        assert!(pool.aa_pool.read().contains(&hash));

        // tx requires gas_limit (100_000) * max_fee_per_gas (2) = 200_000.
        // Build a bundle whose payer (== sender) post-balance is below
        // the required cost.
        let mut bundle = RevmBundleState::default();
        let starved_info = AccountInfo {
            balance: U256::from(100_000_u64),
            nonce: 0,
            code_hash: Default::default(),
            account_id: None,
            code: None,
        };
        let acc = op_revm::revm::database::BundleAccount::new(
            None,
            Some(starved_info),
            Default::default(),
            op_revm::revm::database::AccountStatus::Changed,
        );
        bundle.state.insert(sender, acc);

        let outcome = pool.on_bundle_state(&bundle, 0, B256::ZERO);
        assert_eq!(
            outcome.invalidated.len(),
            1,
            "insolvent payer must surface as `invalidated` for lifecycle fan-out"
        );
        assert_eq!(outcome.invalidated[0].hash(), &hash);
        assert!(!pool.aa_pool.read().contains(&hash), "evicted tx must be gone from the side pool");
        // Lifecycle hook fired Discarded for the evicted tx.
        assert_eq!(discarded_rx.try_recv(), Ok(hash));
    }

    /// Negative: `on_bundle_state` with a payer-balance update that
    /// still covers the fee leaves the pool intact. Pins that an
    /// every-block balance update isn't a uniform eviction storm —
    /// only insolvent payers trip eviction.
    #[test]
    fn on_bundle_state_keeps_solvent_payer() {
        use op_revm::revm::{
            database::states::bundle_state::BundleState as RevmBundleState, state::AccountInfo,
        };

        let sender = Address::repeat_byte(0xE4);
        let pool = make_dual_pool(fresh_client(sender));
        let tx = make_aa_tx(sender, 0);
        let hash = *tx.hash();

        let rt = tokio::runtime::Builder::new_current_thread().build().expect("rt");
        rt.block_on(async {
            pool.admit_aa(TransactionOrigin::Local, tx).await.expect("admit");
        });
        assert!(pool.aa_pool.read().contains(&hash));

        let mut bundle = RevmBundleState::default();
        let healthy_info = AccountInfo {
            balance: U256::from(10_000_000_u64),
            nonce: 0,
            code_hash: Default::default(),
            account_id: None,
            code: None,
        };
        let acc = op_revm::revm::database::BundleAccount::new(
            None,
            Some(healthy_info),
            Default::default(),
            op_revm::revm::database::AccountStatus::Changed,
        );
        bundle.state.insert(sender, acc);

        let outcome = pool.on_bundle_state(&bundle, 0, B256::ZERO);
        assert!(outcome.invalidated.is_empty());
        assert!(pool.aa_pool.read().contains(&hash));
    }
}
