//! Priority-merging iterator that yields transactions from two
//! [`BestTransactions`] sources in non-increasing priority order.
//!
//! Used by [`crate::OpDualPool`] to interleave the protocol pool's iterator
//! (regular EIP-1559 / Legacy / 7702 / etc.) with the AA side pool's
//! lane-aware iterator [`BestAaTransactions`], so block builders see one
//! ordered stream rather than having to fan in themselves.
//!
//! Mirrors tempo's `transaction-pool/src/best.rs::MergeBestTransactions`,
//! trimmed to a single uniform priority extractor (`effective_gas_price(base_fee)`)
//! since both sources implement [`alloy_consensus::Transaction`] with
//! consistent semantics for the AA and standard tx types.

use std::{
    cmp::Ordering as CmpOrdering,
    collections::{BTreeMap, BinaryHeap, HashMap, HashSet},
    sync::Arc,
};

#[cfg_attr(not(test), allow(unused_imports))]
use alloy_consensus::Transaction;
use alloy_primitives::TxHash;
use reth_transaction_pool::{
    BestTransactions, PoolTransaction, ValidPoolTransaction, error::InvalidPoolTransactionError,
};

use crate::eip8130_pool::{Eip8130PoolTx, Eip8130SeqId, Eip8130TxId};

/// Which side last yielded a tx; used to route `mark_invalid` /
/// `mark_invalid_with_error` back to the correct underlying iterator.
#[derive(Debug, Clone, Copy)]
enum Source {
    Left,
    Right,
}

/// Merges two [`BestTransactions`] streams by priority. Each pull from
/// `next()` peeks both sides, yields the higher-priority head, and
/// refills from the side it consumed from.
///
/// `base_fee` is fixed at construction; both sides' priorities are
/// computed against the same value, so the comparison is well-defined
/// even when sides came from pools with different ordering schemes.
///
/// # Tie-breaking
/// Equal priority → left wins. This is arbitrary but deterministic;
/// matches tempo's behavior. Block builders that care about strict FIFO
/// at equal priority should drive the side pool's listener channel to
/// inspect submission order separately.
pub struct MergeBestTransactions<T: PoolTransaction> {
    left: Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<T>>>>,
    right: Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<T>>>>,
    base_fee: u64,
    /// Lookahead buffer for the left iterator. `None` after refill miss.
    next_left: Option<Arc<ValidPoolTransaction<T>>>,
    /// Lookahead buffer for the right iterator. `None` after refill miss.
    next_right: Option<Arc<ValidPoolTransaction<T>>>,
    last_source: Option<Source>,
}

impl<T: PoolTransaction> std::fmt::Debug for MergeBestTransactions<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MergeBestTransactions")
            .field("base_fee", &self.base_fee)
            .field("next_left_hash", &self.next_left.as_ref().map(|t| *t.hash()))
            .field("next_right_hash", &self.next_right.as_ref().map(|t| *t.hash()))
            .field("last_source", &self.last_source)
            .finish_non_exhaustive()
    }
}

impl<T: PoolTransaction> MergeBestTransactions<T> {
    /// Builds a new merger.
    ///
    /// Staleness asymmetry: `left` (reth's protocol pool) is a
    /// live-broadcast iterator, `right` (the AA side via
    /// [`BestAaTransactions`]) is a fresh snapshot per call.
    ///
    /// Acceptable under ~200ms flashblock cadence; revisit if the AA pool grows past
    /// ~10k pending — at that point per-call snapshot construction
    /// (~O(P log P)) becomes a measurable cost on the build path.
    pub fn new(
        left: Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<T>>>>,
        right: Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<T>>>>,
        base_fee: u64,
    ) -> Self {
        Self { left, right, base_fee, next_left: None, next_right: None, last_source: None }
    }

    fn refill(&mut self) {
        if self.next_left.is_none() {
            self.next_left = self.left.next();
        }
        if self.next_right.is_none() {
            self.next_right = self.right.next();
        }
    }

    fn priority(&self, tx: &Arc<ValidPoolTransaction<T>>) -> u128 {
        tx.transaction.effective_gas_price(Some(self.base_fee))
    }
}

impl<T: PoolTransaction> Iterator for MergeBestTransactions<T> {
    type Item = Arc<ValidPoolTransaction<T>>;

    fn next(&mut self) -> Option<Self::Item> {
        self.refill();
        match (&self.next_left, &self.next_right) {
            (Some(l), Some(r)) => {
                let lp = self.priority(l);
                let rp = self.priority(r);
                if lp >= rp {
                    self.last_source = Some(Source::Left);
                    self.next_left.take()
                } else {
                    self.last_source = Some(Source::Right);
                    self.next_right.take()
                }
            }
            (Some(_), None) => {
                self.last_source = Some(Source::Left);
                self.next_left.take()
            }
            (None, Some(_)) => {
                self.last_source = Some(Source::Right);
                self.next_right.take()
            }
            (None, None) => None,
        }
    }
}

impl<T: PoolTransaction> BestTransactions for MergeBestTransactions<T> {
    fn mark_invalid(&mut self, tx: &Self::Item, kind: &InvalidPoolTransactionError) {
        // Route to the side that just yielded; if no side has yielded
        // (which is the path the caller takes only after `next()` returned),
        // forward to both as a defensive fallback. In practice builders
        // only call `mark_invalid` after `next()`.
        match self.last_source {
            Some(Source::Left) => self.left.mark_invalid(tx, kind),
            Some(Source::Right) => self.right.mark_invalid(tx, kind),
            None => {
                self.left.mark_invalid(tx, kind);
                self.right.mark_invalid(tx, kind);
            }
        }
    }

    fn no_updates(&mut self) {
        self.left.no_updates();
        self.right.no_updates();
    }

    fn set_skip_blobs(&mut self, skip_blobs: bool) {
        self.left.set_skip_blobs(skip_blobs);
        self.right.set_skip_blobs(skip_blobs);
    }
}

// ---------------------------------------------------------------------------
// AA-side cursor iterator
// ---------------------------------------------------------------------------

/// Snapshot row for a single AA tx the iterator may yield. Holds enough
/// state to (re-)insert into the priority heap and to reach the next-nonce
/// successor inside `by_id`.
///
/// Crate-private fields — the pool constructs these directly under its read
/// lock and hands them to [`BestAaTransactions::from_parts`].
#[derive(Debug, Clone)]
pub struct AaSnapshotEntry<T: PoolTransaction> {
    /// Priority key (effective_gas_price(base_fee) at snapshot time).
    pub(crate) priority: u128,
    /// Per-pool monotonic id used as a tie-break against `priority`. Older
    /// txs win equal-priority races — this matches the pool's own eviction
    /// ordering so block builders see the same first-in-first-out shape.
    pub(crate) submission_id: u64,
    pub(crate) tx: Arc<ValidPoolTransaction<T>>,
}

/// Heap entry keyed on `(priority, !submission_id, hash)` so `BinaryHeap`'s
/// max-pop yields the highest priority and, on ties, the lowest submission
/// id (oldest tx). Hash is the final disambiguator — `BinaryHeap` requires
/// total ordering and `priority`/`submission_id` collisions, while pathological,
/// must not poison the iterator's structure.
#[derive(Debug, Clone)]
pub(crate) struct HeapKey {
    pub(crate) priority: u128,
    pub(crate) submission_id: u64,
    pub(crate) hash: TxHash,
}

impl PartialEq for HeapKey {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == CmpOrdering::Equal
    }
}
impl Eq for HeapKey {}
impl PartialOrd for HeapKey {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}
impl Ord for HeapKey {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        // Higher priority is "greater" so BinaryHeap pops it first.
        // For equal priority: smaller submission_id is greater (oldest tx
        // wins). Hash breaks the final tie deterministically.
        self.priority
            .cmp(&other.priority)
            .then_with(|| other.submission_id.cmp(&self.submission_id))
            .then_with(|| other.hash.cmp(&self.hash))
    }
}

/// What kind of slot a heap entry refers to. Sequenced lanes drive
/// next-nonce advancement after yielding; expiring-mode txs are
/// independent (no successor lookup needed).
#[derive(Debug, Clone, Copy)]
pub(crate) enum AaSlot {
    /// Sequenced AA tx: `(seq, nonce_seq)`. After yielding, the iterator
    /// looks up `(seq, nonce_seq + 1)` in `by_lane` to advance the lane.
    Sequenced(Eip8130TxId),
    /// Expiring-mode tx: yielded as-is, no follow-up.
    Expiring(TxHash),
}

/// Heap node combining the priority key with the slot reference. `Ord` is
/// delegated to `HeapKey` only — `AaSlot` is just a payload, never compared.
#[derive(Debug, Clone)]
pub(crate) struct HeapNode {
    pub(crate) key: HeapKey,
    pub(crate) slot: AaSlot,
}

impl PartialEq for HeapNode {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}
impl Eq for HeapNode {}
impl PartialOrd for HeapNode {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}
impl Ord for HeapNode {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        self.key.cmp(&other.key)
    }
}

/// Lane-advancing AA-side iterator. Snapshots the side pool's state at
/// construction; each `next()` pop drains the heap head and, for a
/// sequenced lane, pushes the next-nonce entry from the same lane back in.
///
/// Yields items in non-increasing `(priority, -submission_id)` order
/// across all lanes plus expiring-mode txs. Crucially, after a lane head
/// is yielded, the *next* pending nonce in the same lane becomes
/// available for ordering against other lanes' heads — so a high-priority
/// lane's nonce_seq=1 can preempt a low-priority lane's nonce_seq=0
/// according to the global priority ordering.
///
/// Mirrors tempo's `BestAA2dTransactions` but drops the live-broadcast
/// channel — the side pool acquires the read lock once at snapshot time,
/// so newly-admitted txs after construction are simply ignored. Block
/// building lasts on the order of milliseconds; missing those is fine,
/// they'll be picked up on the next round.
pub struct BestAaTransactions<T: Eip8130PoolTx> {
    /// Heap of currently-eligible entries. At construction it holds one
    /// entry per lane (the lowest-nonce *pending* tx) plus every expiring
    /// tx. After a lane head is yielded, its successor is pushed in.
    heap: BinaryHeap<HeapNode>,

    /// Snapshot of all *pending* sequenced AA txs by `(seq, nonce)`. Used
    /// by `next()` to find the lane successor of a just-yielded head.
    /// Removed entries (yielded or marked invalid) are pruned to keep
    /// memory bounded.
    by_lane: BTreeMap<Eip8130TxId, AaSnapshotEntry<T>>,

    /// Snapshot of expiring-mode txs by hash. Pruned on yield / invalidate.
    expiring: HashMap<TxHash, AaSnapshotEntry<T>>,

    /// Lanes whose head was marked invalid: skip every subsequent lane
    /// successor for these. Mirrors tempo's `invalid: HashSet<AASequenceId>`.
    invalid_lanes: HashSet<Eip8130SeqId>,
}

impl<T: Eip8130PoolTx> std::fmt::Debug for BestAaTransactions<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BestAaTransactions")
            .field("heap_len", &self.heap.len())
            .field("by_lane_len", &self.by_lane.len())
            .field("expiring_len", &self.expiring.len())
            .field("invalid_lanes", &self.invalid_lanes.len())
            .finish()
    }
}

impl<T: Eip8130PoolTx> BestAaTransactions<T> {
    /// Builds a new iterator from raw snapshot containers. Callers
    /// (typically [`crate::Eip8130Pool::best_aa_transactions`]) assemble the
    /// inputs under the pool's read lock so the snapshot is internally
    /// consistent.
    ///
    /// `heap` already contains one node per lane head plus every expiring
    /// tx, keyed on `(priority, !submission_id, hash)`. `by_lane` is the
    /// full set of pending sequenced txs (heads + their contiguous
    /// successors). `expiring` is every expiring-mode tx by hash.
    pub(crate) fn from_parts(
        heap: BinaryHeap<HeapNode>,
        by_lane: BTreeMap<Eip8130TxId, AaSnapshotEntry<T>>,
        expiring: HashMap<TxHash, AaSnapshotEntry<T>>,
    ) -> Self {
        Self { heap, by_lane, expiring, invalid_lanes: HashSet::new() }
    }
}

impl<T: Eip8130PoolTx> Iterator for BestAaTransactions<T> {
    type Item = Arc<ValidPoolTransaction<T>>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let node = self.heap.pop()?;
            match node.slot {
                AaSlot::Sequenced(id) => {
                    if self.invalid_lanes.contains(&id.seq) {
                        // Lane was poisoned by a prior `mark_invalid`;
                        // drop the entry without yielding and continue
                        // popping. The corresponding `by_lane` entry is
                        // also dropped to free memory.
                        self.by_lane.remove(&id);
                        continue;
                    }
                    let Some(entry) = self.by_lane.remove(&id) else {
                        // Concurrent removal (mark_invalid path may have
                        // pruned a lane) — skip.
                        continue;
                    };

                    // Push the next nonce in the same lane, if it was
                    // pending in the snapshot. We only re-push when the
                    // successor is contiguous (`prev_nonce + 1`); if the
                    // snapshot had a gap (queued tail), the lane stops
                    // here.
                    let next_id = Eip8130TxId::new(id.seq, id.nonce.saturating_add(1));
                    if let Some(next_entry) = self.by_lane.get(&next_id) {
                        self.heap.push(HeapNode {
                            key: HeapKey {
                                priority: next_entry.priority,
                                submission_id: next_entry.submission_id,
                                hash: *next_entry.tx.hash(),
                            },
                            slot: AaSlot::Sequenced(next_id),
                        });
                    }
                    return Some(entry.tx);
                }
                AaSlot::Expiring(hash) => {
                    let Some(entry) = self.expiring.remove(&hash) else {
                        continue;
                    };
                    return Some(entry.tx);
                }
            }
        }
    }
}

impl<T: Eip8130PoolTx> BestTransactions for BestAaTransactions<T> {
    fn mark_invalid(&mut self, tx: &Self::Item, _kind: &InvalidPoolTransactionError) {
        // Sequenced: poison the entire lane so descendants in the snapshot
        // don't get yielded. Expiring: just record the hash; expiring txs
        // have no successors so a lane-level mark would be meaningless.
        if let Some(seq) = tx.transaction.aa_seq_id() {
            self.invalid_lanes.insert(seq);
            return;
        }
        // Expiring or non-AA (the latter shouldn't reach this iterator,
        // but defend against drift).
        let hash = *tx.hash();
        self.expiring.remove(&hash);
    }

    fn no_updates(&mut self) {
        // Snapshot iterator: `no_updates` is a no-op because we never
        // listen to live updates in the first place.
    }

    fn set_skip_blobs(&mut self, _skip_blobs: bool) {
        // AA txs carry no blobs; this is a no-op.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Sealable, transaction::Recovered};
    use alloy_eips::eip2718::Encodable2718;
    use alloy_primitives::{Address, U256, bytes};
    use op_alloy_consensus::{Eip8130CallEntry, TxEip8130};
    use op_revm::handler::NONCE_KEY_MAX;
    use reth_optimism_primitives::OpTransactionSigned;
    use reth_transaction_pool::{
        TransactionOrigin,
        identifier::{SenderId, TransactionId},
    };
    use std::time::Instant;

    use crate::{Eip8130Pool, OpPooledTransaction};

    // ---- helpers shared by merge-iterator and AA-iterator tests ----

    /// Builds an AA tx with the supplied priority fee for fee-based
    /// ordering tests.
    fn aa_tx_pri(
        sender: Address,
        nonce_key: U256,
        nonce_sequence: u64,
        priority: u128,
    ) -> Arc<ValidPoolTransaction<OpPooledTransaction>> {
        let tx = TxEip8130 {
            chain_id: 10,
            from: Some(sender),
            nonce_key,
            nonce_sequence,
            expiry: 0,
            max_priority_fee_per_gas: priority,
            max_fee_per_gas: priority + 1_000,
            gas_limit: 100_000,
            calls: vec![vec![Eip8130CallEntry { to: Address::repeat_byte(0xBB), data: bytes!() }]],
            ..Default::default()
        };
        let signed: OpTransactionSigned = tx.seal_slow().into();
        let recovered = Recovered::new_unchecked(signed, sender);
        let len = recovered.encode_2718_len();
        let pooled = OpPooledTransaction::new(recovered, len);
        Arc::new(ValidPoolTransaction {
            transaction: pooled,
            transaction_id: TransactionId::new(SenderId::from(0), nonce_sequence),
            propagate: false,
            timestamp: Instant::now(),
            origin: TransactionOrigin::External,
            authority_ids: None,
        })
    }

    /// Single-source [`BestTransactions`] adapter that yields a fixed vec
    /// in submission order. Local to this test module — production code
    /// uses [`BestAaTransactions`] for the AA side.
    struct VecBest<T: PoolTransaction> {
        txs: std::vec::IntoIter<Arc<ValidPoolTransaction<T>>>,
        invalidated: std::collections::HashSet<TxHash>,
    }

    impl<T: PoolTransaction> VecBest<T> {
        fn new(txs: Vec<Arc<ValidPoolTransaction<T>>>) -> Self {
            Self { txs: txs.into_iter(), invalidated: std::collections::HashSet::new() }
        }
    }

    impl<T: PoolTransaction> Iterator for VecBest<T> {
        type Item = Arc<ValidPoolTransaction<T>>;
        fn next(&mut self) -> Option<Self::Item> {
            loop {
                let tx = self.txs.next()?;
                if !self.invalidated.contains(tx.hash()) {
                    return Some(tx);
                }
            }
        }
    }

    impl<T: PoolTransaction> BestTransactions for VecBest<T> {
        fn mark_invalid(&mut self, tx: &Self::Item, _: &InvalidPoolTransactionError) {
            self.invalidated.insert(*tx.hash());
        }
        fn no_updates(&mut self) {}
        fn set_skip_blobs(&mut self, _: bool) {}
    }

    // -------- merge-iterator tests --------

    /// Capability: the merge iterator yields the higher-priority tx
    /// first, alternating between left/right as priorities dictate.
    #[test]
    fn merger_yields_highest_priority_first() {
        let s = Address::repeat_byte(0xAA);
        let left = VecBest::new(vec![
            aa_tx_pri(s, U256::from(0u64), 0, 10_000),
            aa_tx_pri(s, U256::from(1u64), 0, 5_000),
        ]);
        let right = VecBest::new(vec![
            aa_tx_pri(s, U256::from(2u64), 0, 8_000),
            aa_tx_pri(s, U256::from(3u64), 0, 3_000),
        ]);
        let mut merger: MergeBestTransactions<OpPooledTransaction> =
            MergeBestTransactions::new(Box::new(left), Box::new(right), 0);

        let order: Vec<u128> = std::iter::from_fn(|| merger.next())
            .map(|tx| tx.transaction.effective_gas_price(Some(0)))
            .collect();
        assert_eq!(order, vec![10_000, 8_000, 5_000, 3_000]);
    }

    /// Capability: when one side empties first, the other drains the
    /// remainder in original order.
    #[test]
    fn merger_drains_remaining_side() {
        let s = Address::repeat_byte(0xAA);
        let left = VecBest::new(vec![aa_tx_pri(s, U256::from(0u64), 0, 20)]);
        let right = VecBest::new(vec![
            aa_tx_pri(s, U256::from(1u64), 0, 50),
            aa_tx_pri(s, U256::from(2u64), 0, 30),
            aa_tx_pri(s, U256::from(3u64), 0, 10),
        ]);
        let mut merger: MergeBestTransactions<OpPooledTransaction> =
            MergeBestTransactions::new(Box::new(left), Box::new(right), 0);

        let order: Vec<u128> = std::iter::from_fn(|| merger.next())
            .map(|tx| tx.transaction.effective_gas_price(Some(0)))
            .collect();
        assert_eq!(order, vec![50, 30, 20, 10]);
    }

    /// Capability: empty iterators produce empty merge.
    #[test]
    fn merger_handles_both_empty() {
        let left: VecBest<OpPooledTransaction> = VecBest::new(vec![]);
        let right: VecBest<OpPooledTransaction> = VecBest::new(vec![]);
        let mut merger: MergeBestTransactions<OpPooledTransaction> =
            MergeBestTransactions::new(Box::new(left), Box::new(right), 0);
        assert!(merger.next().is_none());
    }

    // -------- AA-side cursor iterator tests --------

    /// Capability: lane head and its in-lane successor both yield, in
    /// nonce order. This is the core regression — pre-fix, only the head
    /// (nonce_seq=0) was emitted because the side pool exposed only
    /// `independent_pending` and the merge iterator never advanced.
    #[test]
    fn aa_iter_advances_within_lane() {
        let sender = Address::repeat_byte(0x11);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let key = U256::from(7u64);

        let h0 = aa_tx_pri(sender, key, 0, 1_000);
        let h0_hash = *h0.hash();
        let h1 = aa_tx_pri(sender, key, 1, 1_000);
        let h1_hash = *h1.hash();
        pool.add_transaction(h0, 0, 0).unwrap();
        pool.add_transaction(h1, 0, 0).unwrap();

        let mut iter = pool.best_aa_transactions(0);
        let first = iter.next().expect("head must yield").hash().to_owned();
        let second = iter.next().expect("next-nonce must yield").hash().to_owned();
        assert!(iter.next().is_none(), "only two txs in this lane");
        assert_eq!(first, h0_hash, "nonce_seq=0 yields first");
        assert_eq!(second, h1_hash, "nonce_seq=1 advances after head");
    }

    /// Capability: every 8130 tx (incl. `nonce_key == 0`) routes through
    /// the AA iterator and lane advancement still works there. Pins the
    /// xlayer-specific "nonce_key=0 is on the AA side, not the protocol
    /// pool" invariant.
    #[test]
    fn aa_iter_advances_within_lane_with_zero_nonce_key() {
        let sender = Address::repeat_byte(0x12);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());

        // nonce_key = 0 — in tempo this would route to the protocol pool;
        // in xlayer it stays in the AA side pool.
        let h0 = aa_tx_pri(sender, U256::ZERO, 0, 1_000);
        let h1 = aa_tx_pri(sender, U256::ZERO, 1, 1_000);
        pool.add_transaction(h0, 0, 0).unwrap();
        pool.add_transaction(h1, 0, 0).unwrap();

        let count = pool.best_aa_transactions(0).count();
        assert_eq!(count, 2, "both nonce_key=0 txs must yield from AA iter");
    }

    /// Capability: marking the head invalid prevents the next-nonce in
    /// the same lane from being yielded — descendants are blocked behind
    /// the now-invalid ancestor.
    #[test]
    fn aa_iter_skips_descendants_of_marked_invalid() {
        let sender = Address::repeat_byte(0x21);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let key = U256::from(3u64);

        let h0 = aa_tx_pri(sender, key, 0, 1_000);
        let h0_hash = *h0.hash();
        let h1 = aa_tx_pri(sender, key, 1, 1_000);
        pool.add_transaction(h0, 0, 0).unwrap();
        pool.add_transaction(h1, 0, 0).unwrap();

        let mut iter = pool.best_aa_transactions(0);
        let first = iter.next().expect("head yields");
        assert_eq!(first.hash(), &h0_hash);
        let kind = InvalidPoolTransactionError::Underpriced;
        iter.mark_invalid(&first, &kind);
        assert!(iter.next().is_none(), "descendant must NOT yield after head invalidation");
    }

    /// Capability: across-lane priority ordering with in-lane advancement.
    /// Two lanes from the same sender:
    ///   * lane A (priority 1000): nonce_seq 0, 1
    ///   * lane B (priority 100):  nonce_seq 0
    /// Order must be: A.0, A.1, B.0 — the high-priority lane fully drains
    /// before B is touched, even though B.0 is independently pending.
    #[test]
    fn aa_iter_priority_across_lanes() {
        let sender = Address::repeat_byte(0x31);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());

        let key_hi = U256::from(1u64);
        let key_lo = U256::from(2u64);

        let hi0 = aa_tx_pri(sender, key_hi, 0, 1_000);
        let hi0_hash = *hi0.hash();
        let hi1 = aa_tx_pri(sender, key_hi, 1, 1_000);
        let hi1_hash = *hi1.hash();
        let lo0 = aa_tx_pri(sender, key_lo, 0, 100);
        let lo0_hash = *lo0.hash();
        pool.add_transaction(hi0, 0, 0).unwrap();
        pool.add_transaction(hi1, 0, 0).unwrap();
        pool.add_transaction(lo0, 0, 0).unwrap();

        let order: Vec<TxHash> = pool.best_aa_transactions(0).map(|t| *t.hash()).collect();
        assert_eq!(
            order,
            vec![hi0_hash, hi1_hash, lo0_hash],
            "high-priority lane drains before low-priority head"
        );
    }

    /// Capability: a queued (nonce-gap) tx is not yielded. Insert
    /// nonce_seq=2 only with on-chain head 0; the lane has a gap (no 0,1)
    /// so nothing is pending. Iter must produce zero items.
    #[test]
    fn aa_iter_skips_queued_gap() {
        let sender = Address::repeat_byte(0x41);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let key = U256::from(9u64);

        let parked = aa_tx_pri(sender, key, 2, 1_000);
        pool.add_transaction(parked, 0, 0).unwrap();

        assert_eq!(
            pool.best_aa_transactions(0).count(),
            0,
            "queued (gapped) lane must yield nothing"
        );
    }

    /// Capability: when the AA-side iterator is wired into
    /// [`MergeBestTransactions`] alongside an empty protocol-pool side, a
    /// lane's full chain (head + descendants) flows through the merge.
    /// Pins the `OpDualPool::best_transactions` integration: pre-fix,
    /// `VecBestTransactions(independent_pending)` yielded a single tx per
    /// lane and the merge dropped descendants on the floor.
    #[test]
    fn dual_pool_best_yields_aa_lane_descendants() {
        let sender = Address::repeat_byte(0x91);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let key = U256::from(5u64);

        let h0 = aa_tx_pri(sender, key, 0, 1_000);
        let h0_hash = *h0.hash();
        let h1 = aa_tx_pri(sender, key, 1, 1_000);
        let h1_hash = *h1.hash();
        let h2 = aa_tx_pri(sender, key, 2, 1_000);
        let h2_hash = *h2.hash();
        pool.add_transaction(h0, 0, 0).unwrap();
        pool.add_transaction(h1, 0, 0).unwrap();
        pool.add_transaction(h2, 0, 0).unwrap();

        let aa = pool.best_aa_transactions(0);
        let left: VecBest<OpPooledTransaction> = VecBest::new(vec![]);
        let mut merger: MergeBestTransactions<OpPooledTransaction> =
            MergeBestTransactions::new(Box::new(left), Box::new(aa), 0);

        let order: Vec<TxHash> = std::iter::from_fn(|| merger.next()).map(|t| *t.hash()).collect();
        assert_eq!(
            order,
            vec![h0_hash, h1_hash, h2_hash],
            "merge iterator must yield all three lane txs in nonce order"
        );
    }

    /// Capability: expiring-mode txs are independent — they all yield,
    /// in priority order, regardless of nonce_key.
    #[test]
    fn aa_iter_yields_expiring_in_priority_order() {
        let sender = Address::repeat_byte(0x51);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());

        // Two expiring-mode txs with different priorities.
        let mk = |priority: u128, expiry: u64| -> Arc<ValidPoolTransaction<OpPooledTransaction>> {
            let mut tx = TxEip8130 {
                chain_id: 10,
                from: Some(sender),
                nonce_key: NONCE_KEY_MAX,
                nonce_sequence: 0,
                expiry,
                max_priority_fee_per_gas: priority,
                max_fee_per_gas: priority + 1_000,
                gas_limit: 100_000,
                calls: vec![vec![Eip8130CallEntry {
                    to: Address::repeat_byte(0xBB),
                    data: bytes!(),
                }]],
                ..Default::default()
            };
            // Vary expiry so the sender_signature_hash differs per call,
            // avoiding the AlreadyImported dedup on identical expiring txs.
            tx.expiry = expiry;
            let signed: OpTransactionSigned = tx.seal_slow().into();
            let recovered = Recovered::new_unchecked(signed, sender);
            let len = recovered.encode_2718_len();
            let pooled = OpPooledTransaction::new(recovered, len);
            Arc::new(ValidPoolTransaction {
                transaction: pooled,
                transaction_id: TransactionId::new(SenderId::from(0), 0),
                propagate: false,
                timestamp: Instant::now(),
                origin: TransactionOrigin::External,
                authority_ids: None,
            })
        };
        let lo = mk(100, 1_000_001);
        let hi = mk(10_000, 1_000_002);
        let lo_hash = *lo.hash();
        let hi_hash = *hi.hash();
        pool.add_transaction(lo, 0, 0).unwrap();
        pool.add_transaction(hi, 0, 0).unwrap();

        let order: Vec<TxHash> = pool.best_aa_transactions(0).map(|t| *t.hash()).collect();
        assert_eq!(order, vec![hi_hash, lo_hash], "higher-priority expiring tx yields first");
    }
}
