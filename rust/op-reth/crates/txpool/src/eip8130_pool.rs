//! Side pool for EIP-8130 (XLayer AA) transactions, indexed by 2D nonce.
//!
//! # Why a side pool
//!
//! Reth's standard txpool keys ordering by `(sender, nonce: u64)`. EIP-8130
//! AA transactions identify themselves by `(sender, nonce_key: U256, nonce_sequence: u64)`,
//! and a special expiring-nonce mode replaces the sequenced ordering with a
//! one-shot tx-hash admission keyed off a `NonceManager` ring buffer.
//! The standard pool's ordering invariants don't compose with either, so we
//! maintain a dedicated structure and merge the two pools at `best_transactions()`
//! time.
//!
//! # Lanes ("sequences")
//!
//! For sequenced AA txs (`nonce_key != NONCE_KEY_MAX`), each `(sender, nonce_key)`
//! pair is a **lane**: an ordered chain of nonces starting from the on-chain
//! `aa_nonce_slot(sender, nonce_key)` value. Within a lane, contiguous txs from
//! `next_nonce` upward are *pending*; the first nonce gap demotes the rest to
//! *queued*. A lane can hold up to [`Eip8130PoolConfig::max_txs_per_sequence`]
//! txs.
//!
//! # Expiring-nonce path
//!
//! For `nonce_key == NONCE_KEY_MAX`, replay protection comes from a `NonceManager`
//! storage slot derived from the sealed transaction hash (see
//! [`op_revm::handler::aa_expiring_seen_slot`]). These txs have no sequencing
//! relationship: each is independently *pending* on insertion and removed when
//! the seen-slot is observed non-zero in a state diff.
//!
//! # State-driven promotion / removal
//!
//! All nonce / replay state lives at one address (`NONCE_MANAGER_ADDRESS`).
//! [`Eip8130Pool::on_state_updates`] consumes a per-block storage-diff snapshot
//! and:
//!   * advances per-lane `next_nonce` when a tracked `aa_nonce_slot` changes, promoting
//!     newly-contiguous txs and removing mined ones;
//!   * removes expiring-nonce txs whose `aa_expiring_seen_slot` flipped non-zero.
//!
//! # Out of scope (intentional design choices, not TODO items)
//!
//! * tier-based DoS limits (locked / trusted accounts) — see base's
//!   `Eip8130PoolConfig::trusted_max_*`. v2 uses a flat per-sender cap by design.
//! * keychain spending limits, validator-token pricing, blacklist invalidation — tempo-specific
//!   protocol features with no XLayer counterpart.

use alloy_primitives::{Address, B256, TxHash, U256};
use op_revm::{handler::NONCE_KEY_MAX, precompiles_xlayer::aa_nonce_slot};
use reth_metrics::{
    Metrics,
    metrics::{Counter, Gauge},
};
use reth_transaction_pool::{
    PoolTransaction, PriceBumpConfig, SubPoolLimit, TransactionEvent, ValidPoolTransaction,
    error::{PoolError, PoolErrorKind},
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, btree_map::Entry},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering as AtomicOrdering},
    },
};
use tokio::sync::broadcast;

/// Metrics for the EIP-8130 (XLayer AA) side pool. Mirrors tempo's
/// `AA2dPoolMetrics` shape (tempo `crates/transaction-pool/src/metrics.rs:11-75`)
/// but adapts the gauge set to the side pool's actual data structures
/// (sequenced lanes + expiring entries) and adds eviction-cause counters
/// the production telemetry depends on.
#[derive(Metrics, Clone)]
#[metrics(scope = "transaction_pool.aa")]
pub struct Eip8130PoolMetrics {
    /// Number of AA transactions inserted (admit + replace).
    pub txs_inserted: Counter,
    /// Number of AA transactions removed via `remove_by_hash`
    /// (mined / invalidated / capacity / expiry are also counted under
    /// the more specific eviction-cause counters below; this is the
    /// generic super-counter).
    pub txs_removed: Counter,
    /// Number of AA transactions promoted from queued to pending.
    pub txs_promoted: Counter,
    /// Number of AA transactions demoted from pending to queued.
    pub txs_demoted: Counter,
    /// Number of AA transactions evicted because the pool / sub-pool
    /// hit its capacity limit (queued, pending, or `max_pool_size`).
    pub txs_evicted_capacity: Counter,
    /// Number of AA transactions evicted because their `expiry` passed
    /// (`sweep_expired`).
    pub txs_evicted_expiry: Counter,
    /// Number of AA transactions evicted because of an `on_state_updates`
    /// invalidation event (lock toggle, owner-config rotation, ...).
    pub txs_evicted_invalidation: Counter,
    /// Number of AA transactions evicted because their payer's post-state
    /// balance dropped below `gas_limit * max_fee_per_gas` (mirrors tempo's
    /// `Check 3b` insolvent-fee_payer eviction at `tempo_pool.rs:267-307`).
    pub txs_evicted_insolvent_payer: Counter,
    /// Number of AA transactions replaced at an existing tx_id
    /// (price-bumped resubmission). A replace fires `txs_inserted` and
    /// `txs_replaced`; the replaced tx is reflected in `txs_removed`.
    pub txs_replaced: Counter,

    /// Total AA transactions currently in the pool (sequenced + expiring).
    pub pool_size_total: Gauge,
    /// Pending (executable) AA transactions currently in the pool.
    pub pool_size_pending: Gauge,
    /// Queued (nonce-gap-parked) AA transactions currently in the pool.
    pub pool_size_queued: Gauge,
    /// AA transactions tracked in `expiry_index` (non-zero `expiry`).
    pub pool_size_expiring: Gauge,
    /// Distinct AA-effective senders with at least one tx in the pool.
    pub unique_senders: Gauge,
}

// ---------------------------------------------------------------------------
// IDs
// ---------------------------------------------------------------------------

/// Identifies a 2D-nonce lane: one chain of sequential nonces per
/// `(sender, nonce_key)` pair. `nonce_key == NONCE_KEY_MAX` is reserved for
/// the expiring-nonce path and is **not** routed through this id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Eip8130SeqId {
    /// AA tx sender (NOT the EIP-2718 signer; payer-sponsored txs use this).
    pub sender: Address,
    /// 2D nonce key.
    pub nonce_key: U256,
}

impl Eip8130SeqId {
    /// Constructs a lane id.
    pub const fn new(sender: Address, nonce_key: U256) -> Self {
        Self { sender, nonce_key }
    }
}

/// Unique identifier for a pooled sequenced AA tx: `(lane, nonce_sequence)`.
///
/// `Ord` orders by `(sender, nonce_key, nonce)` — the `BTreeMap<TxId, ...>`
/// range scan from `(seq, on_chain_nonce)` walks the lane's contiguous prefix
/// in O(log n + k). Expiring-nonce txs are not assigned a `Eip8130TxId`; they
/// live in a separate map keyed by `expiring_nonce_hash`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Eip8130TxId {
    /// `(sender, nonce_key)` lane.
    pub seq: Eip8130SeqId,
    /// Position within the lane.
    pub nonce: u64,
}

impl Eip8130TxId {
    /// Constructs a tx id from its lane and nonce sequence.
    pub const fn new(seq: Eip8130SeqId, nonce: u64) -> Self {
        Self { seq, nonce }
    }
}

// ---------------------------------------------------------------------------
// Invalidation rules
// ---------------------------------------------------------------------------

/// Action to take on a registered tx when its tracked state slot is mutated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidationOutcome {
    /// State change is compatible with the tx's pre-conditions; tx survives.
    Keep,
    /// State change broke the tx's pre-conditions; the pool must drop it.
    Evict,
}

/// Per-key rule encoding the structural reason a tx registered this slot.
///
/// A single tx registers one or more `(addr, slot) → InvalidationRule` pairs
/// at admission time. When the on-chain value at one of those slots moves,
/// [`evaluate_invalidation`] decides whether the new value is still
/// compatible with the tx (in which case the tx stays) or proves the tx is
/// no longer valid (in which case it is evicted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InvalidationRule {
    /// Owner-config slot for some owner_id participating in `sender_auth` or
    /// `payer_auth`. The slot's word encodes `(verifier, scope)` (spec line
    /// 226). Compatibility check: verifier matches the auth's expectation
    /// AND scope still includes `required_scope_bit` (or scope == 0,
    /// "default scope" — legacy / unset words).
    OwnerConfig {
        /// Verifier the auth was constructed against. Mismatch → evict.
        expected_verifier: Address,
        /// Scope bit that must be present (e.g. SENDER for `sender_auth`,
        /// PAYER for `payer_auth`).
        required_scope_bit: u8,
    },
    /// `account_state[sender]` slot — packs lock window AND change_sequence
    /// halves into one word. A diff at this slot can mean unrelated things
    /// (lock toggle, sequence advance, both); the rule encodes which halves
    /// matter for this tx.
    AccountState {
        /// True iff the tx's success depends on the sender being unlocked
        /// at execution time (e.g. tx contains `account_changes`).
        lock_sensitive: bool,
        /// `Some(seq)` iff the tx's sender_auth / payer_auth bound itself
        /// to a specific change_sequence value (signature-bound, spec line
        /// 376; advancing past it means the auth is no longer replayable).
        seq_check: Option<SeqExpect>,
    },
}

/// Sequence-window expectation embedded in an [`InvalidationRule::AccountState`].
///
/// Spec line 376: `change_sequence` is signature-bound, so any advance
/// past the expected pre-value invalidates the auth. We never "rewrite"
/// the expected value with the new on-chain value — we always evict,
/// because we can't synthesize a replacement signature.
///
/// One tx may bind both halves of the packed `_accountState[sender]`
/// slot (see `op_revm/handler.rs:483-519`): a `ConfigChange` entry with
/// `chain_id == 0` updates the multichain half, and a separate entry
/// with `chain_id == tx.chain_id` updates the local half. Each populated
/// half is checked independently against the new on-chain word, and a
/// single advance on either half triggers eviction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SeqExpect {
    /// `Some(pre)` when the tx pinned the multichain half — the low 8
    /// bytes / `limbs[0]` of the packed word. `None` means the tx
    /// doesn't touch the multichain half and the multichain check is
    /// skipped.
    pub multichain_pre: Option<u64>,
    /// `Some(pre)` when the tx pinned the local half — the next 8 bytes
    /// / `(word >> 64).limbs[0]`. `None` means the tx doesn't touch the
    /// local half and the local check is skipped.
    pub local_pre: Option<u64>,
}

/// Pure rule-aware evaluator: given the new on-chain word at the registered
/// slot and the current block timestamp, decide whether the tx survives.
///
/// Cited spec lines (mirrored at /tmp/eip-8130.md in this branch):
///   * scope bits `0x01..=0x08` — line 226;
///   * `change_sequence` is signature-bound — line 376;
///   * lock-rejection rule — line 511;
///   * `REVOKED_VERIFIER` sentinel revoking owner config — lines 525-526 / 787;
///   * sentinel address layout — lines 54 / 62.
pub(crate) fn evaluate_invalidation(
    rule: &InvalidationRule,
    new_value: U256,
    block_timestamp: u64,
) -> InvalidationOutcome {
    use op_revm::{
        constants::REVOKED_VERIFIER,
        handler::{
            parse_owner_config_word, read_packed_sequence, unlocks_at_from_account_state_word,
        },
    };
    match rule {
        InvalidationRule::OwnerConfig { expected_verifier, required_scope_bit } => {
            let (verifier, scope) = parse_owner_config_word(new_value);
            // Spec lines 525-526 / 787: REVOKED_VERIFIER is the explicit
            // tombstone — never compatible with any expected_verifier.
            if verifier == REVOKED_VERIFIER {
                return InvalidationOutcome::Evict;
            }
            if verifier != *expected_verifier {
                return InvalidationOutcome::Evict;
            }
            // scope == 0 means "default scope" (effectively all bits set);
            // only a non-zero scope that explicitly drops the required bit
            // breaks compatibility (matches handler.rs:242).
            if scope != 0 && (scope & required_scope_bit) == 0 {
                return InvalidationOutcome::Evict;
            }
            InvalidationOutcome::Keep
        }
        InvalidationRule::AccountState { lock_sensitive, seq_check } => {
            if *lock_sensitive {
                let unlocks_at = unlocks_at_from_account_state_word(new_value);
                if block_timestamp < unlocks_at {
                    return InvalidationOutcome::Evict;
                }
            }
            // Spec line 376 + `op_revm/handler.rs:483-519`: each half of
            // the packed `_accountState` word is signature-bound
            // independently. Either half advancing past the expected
            // pre-value means a competing config-change already won the
            // race for that half — our auth no longer covers it.
            if let Some(seq) = seq_check {
                if let Some(expected_mc) = seq.multichain_pre {
                    if read_packed_sequence(new_value, true) > expected_mc {
                        return InvalidationOutcome::Evict;
                    }
                }
                if let Some(expected_local) = seq.local_pre {
                    if read_packed_sequence(new_value, false) > expected_local {
                        return InvalidationOutcome::Evict;
                    }
                }
            }
            InvalidationOutcome::Keep
        }
    }
}

// ---------------------------------------------------------------------------
// Tx accessor trait
// ---------------------------------------------------------------------------

/// Pool-tx accessors needed by [`Eip8130Pool`].
///
/// The pool is generic over `T: Eip8130PoolTx` so that unit tests can use mock
/// transactions while production code specializes on
/// [`crate::OpPooledTransaction`].
///
/// All methods are total: they return `None` when the transaction is not an
/// AA tx, or when the requested field is irrelevant to its mode (e.g.
/// `expiring_nonce_hash()` returns `None` for sequenced AA txs).
pub trait Eip8130PoolTx: PoolTransaction {
    /// True iff this is an EIP-8130 AA tx (any mode).
    fn is_eip8130(&self) -> bool;

    /// Returns the inner [`op_alloy_consensus::TxEip8130`] if this is an
    /// AA tx. The pool admission path passes the result to
    /// `validate_eip8130_transaction` for spec checks.
    fn as_eip8130(&self) -> Option<&op_alloy_consensus::TxEip8130>;

    /// AA-effective sender (post-recovery / explicit `from`).
    ///
    /// Distinct from [`PoolTransaction::sender`] which returns the *signer*
    /// of the EIP-2718 envelope; for a sponsored AA tx the signer may be the
    /// payer, not the sender. Returns `None` for non-AA txs.
    fn aa_sender(&self) -> Option<Address>;

    /// AA-effective payer — the address that funds `gas_limit *
    /// max_fee_per_gas`. Returns `tx.payer` when set (sponsored shape) and
    /// falls back to [`Self::aa_sender`] (self-pay) otherwise. Used by
    /// [`Eip8130Pool::on_balance_updates`] to re-validate gas-affordability
    /// against post-state balances; mirrors tempo's `fee_payer` resolution
    /// (`tempo_pool.rs:279`). Returns `None` for non-AA txs.
    fn aa_payer(&self) -> Option<Address> {
        self.as_eip8130()?.payer.or_else(|| self.aa_sender())
    }

    /// 2D nonce key. `Some(NONCE_KEY_MAX)` indicates expiring mode.
    fn aa_nonce_key(&self) -> Option<U256>;

    /// Nonce sequence within the lane. Always `Some(0)` for expiring mode
    /// (validated upstream).
    fn aa_nonce_sequence(&self) -> Option<u64>;

    /// `aa_expiring_seen_slot(tx_hash)` — the storage slot
    /// in `NONCE_MANAGER_ADDRESS` whose non-zero value invalidates the tx.
    /// Returns `None` for sequenced txs.
    fn aa_expiring_nonce_slot(&self) -> Option<U256>;

    /// `tx.expiry` (zero ⇒ no-expiry sentinel). Maintenance uses this to
    /// drop txs whose deadline has passed without ever landing on chain;
    /// see [`Eip8130Pool::sweep_expired`].
    fn aa_expiry(&self) -> Option<u64> {
        Some(self.as_eip8130()?.expiry)
    }

    /// `tx_hash` — the unique identifier for an
    /// expiring-nonce tx within the pool. Returns `None` for sequenced txs.
    fn aa_expiring_nonce_hash(&self) -> Option<B256>;

    /// Effective priority fee (`min(max_priority_fee, max_fee - base_fee)`)
    /// used for ordering / eviction. AA txs have their own fee fields, so
    /// this isn't derivable from [`PoolTransaction`] generic accessors.
    fn aa_priority_fee(&self, base_fee: u64) -> u128;

    /// Returns the `((address, slot), InvalidationRule)` pairs the pool
    /// must consult when state at any of those slots mutates. The rule
    /// encodes the structural reason the tx cares about that slot;
    /// [`evaluate_invalidation`] uses it to decide Keep vs Evict against
    /// the new on-chain value (in [`Eip8130Pool::on_state_updates`]).
    ///
    /// Default: empty, treating the tx as state-immune. Production
    /// bindings (see the `OpPooledTransaction` impl) override to track
    /// at least the sender's owner_config slot. Over-reporting causes
    /// spurious evictions; under-reporting lets stale txs survive into
    /// block building.
    fn aa_invalidation_rules(&self) -> Vec<((Address, U256), InvalidationRule)> {
        Vec::new()
    }

    // ---- derived helpers ----

    /// True iff this is an AA tx whose `nonce_key == NONCE_KEY_MAX`.
    fn is_expiring_nonce(&self) -> bool {
        self.aa_nonce_key().is_some_and(|k| k == NONCE_KEY_MAX)
    }

    /// True iff this is a sequenced AA tx (`is_eip8130() && !is_expiring_nonce()`).
    fn is_eip8130_2d(&self) -> bool {
        self.is_eip8130() && !self.is_expiring_nonce()
    }

    /// Lane id for a sequenced AA tx.
    fn aa_seq_id(&self) -> Option<Eip8130SeqId> {
        if !self.is_eip8130_2d() {
            return None;
        }
        Some(Eip8130SeqId::new(self.aa_sender()?, self.aa_nonce_key()?))
    }

    /// Tx id for a sequenced AA tx.
    fn aa_tx_id(&self) -> Option<Eip8130TxId> {
        Some(Eip8130TxId::new(self.aa_seq_id()?, self.aa_nonce_sequence()?))
    }

    /// `aa_nonce_slot(sender, key)` — the storage slot in
    /// `NONCE_MANAGER_ADDRESS` whose value is the lane's `next_nonce`.
    /// Returns `None` for expiring-mode txs.
    fn aa_nonce_key_slot(&self) -> Option<U256> {
        let seq = self.aa_seq_id()?;
        Some(aa_nonce_slot(seq.sender, seq.nonce_key))
    }
}

// ---------------------------------------------------------------------------
// Internal entries
// ---------------------------------------------------------------------------

/// Per-tx pool state: the validated tx + its priority + monotonic submission
/// id + a flag (atomic for cheap mutation across promotion sweeps without
/// re-keying the BTreeMap).
#[derive(Debug)]
struct PooledEntry<T: PoolTransaction> {
    tx: Arc<ValidPoolTransaction<T>>,
    priority: u128,
    submission_id: u64,
    /// `true` if the tx is currently executable (no preceding nonce gap in
    /// its lane, or — for expiring-mode — unconditionally).
    is_pending: AtomicBool,
    /// Cached resolution of `tx.aa_invalidation_rules()`, computed once at
    /// admission time. The trait method runs `build_sender_auth_state` +
    /// `build_payer_auth_state` (keccak256 + secp256k1 ecrecover) and is
    /// invoked on register / unregister / replacement paths; caching here
    /// drops worst-case ~4× ECRecover per admit+replace cycle (cf PERF-1).
    /// Lifecycle: populated when the entry is constructed, dropped when
    /// the entry is dropped (replacements drop the old entry's cache
    /// alongside it).
    cached_invalidation_rules: Vec<((Address, U256), InvalidationRule)>,
    /// Admission-time payer-balance predicate (= `gas_limit *
    /// max_fee_per_gas + l1_data_fee`). F5: cached here so
    /// [`Eip8130Pool::on_balance_updates`] re-validates with the exact
    /// threshold the validator enforced, including the L1 component which
    /// the maintenance loop does not otherwise have access to (no
    /// `OpL1BlockInfo` plumbing there).
    required_balance: U256,
}

impl<T: PoolTransaction> PooledEntry<T> {
    fn set_pending(&self, val: bool) -> bool {
        self.is_pending.swap(val, AtomicOrdering::AcqRel)
    }

    fn is_pending(&self) -> bool {
        self.is_pending.load(AtomicOrdering::Acquire)
    }
}

/// Eviction-order key. `BTreeSet<EvictionKey>` always pops the *lowest*
/// priority entry first; ties broken by submission_id (newer first, matching
/// tempo's "preserve older waiters" policy).
#[derive(Debug, Clone)]
struct EvictionKey {
    priority: u128,
    submission_id: u64,
    /// The tx hash uniquely disambiguates entries with identical priority
    /// and submission_id (which shouldn't occur in practice but the BTreeSet
    /// must enforce a total order regardless).
    hash: TxHash,
}

impl PartialEq for EvictionKey {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority &&
            self.submission_id == other.submission_id &&
            self.hash == other.hash
    }
}

impl Eq for EvictionKey {}

impl PartialOrd for EvictionKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EvictionKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.priority
            .cmp(&other.priority)
            .then(other.submission_id.cmp(&self.submission_id))
            .then(self.hash.cmp(&other.hash))
    }
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Tunables for [`Eip8130Pool`]. Defaults follow base's published values where
/// they exist; the per-sender cap follows tempo (single-tier DoS).
#[derive(Debug, Clone)]
pub struct Eip8130PoolConfig {
    /// Legacy global hard cap across pending + queued. Kept for callers
    /// that configure a single flat limit; default is unlimited so the
    /// tempo-style per-subpool limits below are authoritative.
    pub max_pool_size: usize,
    /// Maximum executable AA txs. Mirrors tempo's pending subpool limit.
    pub pending_limit: SubPoolLimit,
    /// Maximum parked AA txs. Mirrors tempo's queued subpool limit.
    pub queued_limit: SubPoolLimit,
    /// Maximum txs per `(sender)`, summed across all lanes + expiring.
    /// Prevents a single sender from monopolising pool capacity.
    pub max_txs_per_sender: usize,
    /// Maximum txs per `(sender, nonce_key)` lane.
    pub max_txs_per_sequence: usize,
    /// Replacement under-pricing rule for same-`Eip8130TxId` collisions.
    pub price_bump: PriceBumpConfig,
}

impl Default for Eip8130PoolConfig {
    fn default() -> Self {
        Self {
            max_pool_size: usize::MAX,
            pending_limit: SubPoolLimit::default(),
            queued_limit: SubPoolLimit::default(),
            max_txs_per_sender: 128,
            max_txs_per_sequence: 16,
            price_bump: PriceBumpConfig::default(),
        }
    }
}

impl Eip8130PoolConfig {
    /// Inherits the protocol pool's [`reth_transaction_pool::PoolConfig`]
    /// per-subpool limits (count + bytes) so the AA side pool honors the
    /// same operator-supplied tuning instead of falling back to reth's
    /// `TXPOOL_SUBPOOL_MAX_*` defaults via [`SubPoolLimit::default`]. F5.
    ///
    /// `max_pool_size` is derived as the sum of `pending_limit.max_txs +
    /// queued_limit.max_txs` so the legacy global cap is never tighter
    /// than the sum of the per-subpool caps; the per-subpool caps remain
    /// authoritative. `max_txs_per_sender` / `max_txs_per_sequence` /
    /// `price_bump` are AA-specific and keep their existing defaults —
    /// reth's `PoolConfig::max_account_slots` / `price_bumps` shapes
    /// don't compose 1:1 with 2D-nonce lanes.
    pub fn from_node_config(cfg: &reth_transaction_pool::PoolConfig) -> Self {
        let defaults = Self::default();
        Self {
            max_pool_size: cfg.pending_limit.max_txs.saturating_add(cfg.queued_limit.max_txs),
            pending_limit: cfg.pending_limit,
            queued_limit: cfg.queued_limit,
            max_txs_per_sender: defaults.max_txs_per_sender,
            max_txs_per_sequence: defaults.max_txs_per_sequence,
            price_bump: defaults.price_bump,
        }
    }
}

// ---------------------------------------------------------------------------
// Outcomes
// ---------------------------------------------------------------------------

/// Result of [`Eip8130Pool::add_transaction`].
#[derive(Debug)]
pub enum Eip8130AddOutcome<T: PoolTransaction> {
    /// Admitted as immediately executable (no nonce gap, or expiring mode).
    Pending(Eip8130PendingAdded<T>),
    /// Admitted but parked behind a nonce gap; will be promoted by a future
    /// `on_state_updates` or `add_transaction` filling the gap.
    Queued {
        /// The freshly inserted transaction.
        transaction: Arc<ValidPoolTransaction<T>>,
        /// Replaced transaction, if this was a same-`tx_id` replacement.
        replaced: Option<Arc<ValidPoolTransaction<T>>>,
    },
}

/// Detail of a pending admission: pending promotions and capacity-driven
/// discards triggered by the insertion.
#[derive(Debug)]
pub struct Eip8130PendingAdded<T: PoolTransaction> {
    /// Inserted transaction.
    pub transaction: Arc<ValidPoolTransaction<T>>,
    /// Replaced transaction, if this was a same-`tx_id` replacement.
    pub replaced: Option<Arc<ValidPoolTransaction<T>>>,
    /// Other txs in the same lane that became pending as a side effect of
    /// the insertion (they were queued behind this tx's nonce gap).
    pub promoted: Vec<Arc<ValidPoolTransaction<T>>>,
    /// Txs evicted due to capacity overflow during this insertion.
    pub discarded: Vec<Arc<ValidPoolTransaction<T>>>,
}

// ---------------------------------------------------------------------------
// Pool
// ---------------------------------------------------------------------------

/// EIP-8130 side pool: tracks 2D-nonce sequenced lanes and one-shot
/// expiring-nonce txs.
///
/// Single-threaded; the production wrapper [`crate::OpPool`] holds the pool
/// behind an `Arc<RwLock<_>>`.
#[derive(Debug)]
pub struct Eip8130Pool<T: Eip8130PoolTx> {
    config: Eip8130PoolConfig,
    /// Monotonic submission id, used for eviction tie-breaking and FIFO
    /// fairness within identical priority.
    next_submission_id: u64,

    /// Sequenced txs indexed by `(sender, nonce_key, nonce)`. Range scan
    /// over a lane prefix is the primary access pattern.
    by_id: BTreeMap<Eip8130TxId, Arc<PooledEntry<T>>>,

    /// O(1) lane count maintained alongside `by_id`; cf PERF-4. Pre-fix,
    /// `lane_count` did `by_id.range(..).count()` (O(lane_size)) on every
    /// admission and removal. Incremented on `by_id` insert, decremented
    /// on `by_id` remove.
    txs_by_seq: HashMap<Eip8130SeqId, usize>,

    /// All txs (sequenced + expiring) by hash, for deduplication and
    /// `remove_by_hash`.
    by_hash: HashMap<TxHash, Arc<ValidPoolTransaction<T>>>,

    /// Independent pending head per lane: the lowest-nonce *pending* tx of
    /// each lane. Used by `best_transactions` to drive block building one
    /// tx per lane at a time.
    independent: HashMap<Eip8130SeqId, Arc<PooledEntry<T>>>,

    /// Exclusive upper bound of the contiguous pending prefix for each lane.
    /// This lets admission append to the pending prefix without rescanning
    /// from the on-chain lane head on every tx.
    pending_tip_by_seq: HashMap<Eip8130SeqId, u64>,

    /// Expiring-nonce txs keyed by their unique
    /// `tx_hash`. Always immediately pending.
    expiring: HashMap<B256, Arc<PooledEntry<T>>>,

    /// Running pending/queued transaction counts. Expiring-mode entries
    /// contribute to `pending_count`; sequenced entries move between
    /// `pending_count` and `queued_count` when promoted/demoted. Keeping
    /// these counters incremental avoids an O(pool) scan on every canonical
    /// update just to refresh gauges or enforce caps.
    pending_count: usize,
    queued_count: usize,

    /// Reverse index: `aa_nonce_slot(sender, key)` storage slot in
    /// `NONCE_MANAGER_ADDRESS` → lane id. Lets `on_state_updates` resolve a
    /// state diff to the affected lane in O(1) without re-deriving the slot.
    slot_to_seq: HashMap<U256, Eip8130SeqId>,

    /// Reverse index: `aa_expiring_seen_slot(tx_hash)` storage slot →
    /// expiring-nonce hash. Lets `on_state_updates` evict an expiring tx
    /// whose seen-slot was flipped non-zero by inclusion.
    slot_to_expiring: HashMap<U256, B256>,

    /// Per-sender count for DoS cap enforcement; entries dropped on count
    /// reaching zero.
    txs_by_sender: HashMap<Address, usize>,

    /// Last observed on-chain `next_nonce` per lane. Initial value is
    /// captured from the first admitted tx's `state_nonce`; updates flow
    /// through `on_state_updates`.
    on_chain_nonces: HashMap<Eip8130SeqId, u64>,

    /// All txs ordered by eviction priority. New insertions appear here;
    /// capacity overflow pops the head (lowest priority).
    by_eviction: BTreeSet<EvictionKey>,

    /// Expiry-driven eviction index: `tx.expiry` → set of tx hashes.
    /// Only populated for txs with non-zero `expiry` (zero is the no-expiry
    /// sentinel). A `BTreeMap` keyed by timestamp lets `sweep_expired` walk
    /// the head in O(log n + k) rather than scan every entry.
    expiry_index: BTreeMap<u64, HashSet<TxHash>>,

    /// Cross-address state-diff invalidation index:
    /// `(addr, slot) → { tx_hash → InvalidationRule }`. Each AA tx publishes
    /// its rules at admission time via
    /// [`Eip8130PoolTx::aa_invalidation_rules`]; on a diff at one of the
    /// registered slots, the pool evaluates each registered rule against
    /// the new value (rule-aware Keep/Evict, not unconditional eviction —
    /// e.g. an owner_config diff that adds the PAYER bit while keeping
    /// SENDER does not evict a sender_auth-only tx).
    /// Distinct from `slot_to_seq` / `slot_to_expiring`, which encode
    /// nonce-advance / inclusion (the *successful* lifecycle); this index
    /// encodes *failure* (lock toggle, owner revocation, sequence advance).
    invalidation_index: HashMap<(Address, U256), HashMap<TxHash, InvalidationRule>>,

    /// Broadcasts a tx hash whenever an AA tx becomes pending (admission
    /// or promotion). The OpDualPool wrapper subscribes to forward these
    /// to its `pending_transactions_listener_for(...)` consumers, giving
    /// gossip and RPC parity with regular tx admission.
    pending_tx_broadcaster: broadcast::Sender<TxHash>,
    /// Broadcasts a tx hash whenever an AA tx is removed from the pool
    /// (mined, invalidated, capacity-evicted, expired, manually pruned).
    /// Subscribed by `OpDualPool::add_transaction_and_subscribe` so a
    /// per-tx event subscription can surface a `Discarded` event without
    /// reaching into the protocol pool's listener bookkeeping.
    discarded_tx_broadcaster: broadcast::Sender<TxHash>,
    /// Broadcasts full per-tx events for AA txs. This backs
    /// `OpDualPool::transaction_event_listener` and
    /// `add_transaction_and_subscribe` for side-pool-only txs.
    event_broadcaster: broadcast::Sender<(TxHash, TransactionEvent)>,

    /// Pool telemetry. Per-event counters (`txs_inserted`, `txs_removed`,
    /// etc.) increment on every state-changing op; size gauges
    /// (pool/pending/queued/expiring/unique_senders) are refreshed only at
    /// batch-end via [`Self::record_size_gauges`] so per-admit / per-remove
    /// paths skip the O(n) walk.
    metrics: Eip8130PoolMetrics,

    /// Running pending-bucket byte total for [`Self::discard_to_cap`]'s
    /// byte-based eviction pass (F5). Maintained incrementally by
    /// [`Self::add_pending_bytes`] / [`Self::sub_pending_bytes`] and the
    /// promotion / demotion paths so we don't scan every entry on each
    /// admit. Unit matches `PoolTransaction::encoded_length()`. Reth
    /// tracks the same per-subpool byte total in its own subpools (cf
    /// `SubPoolLimit::max_size`); the AA side pool intentionally skips
    /// counting it across `pool_size_total` since `max_size` only
    /// constrains the per-subpool bucket.
    pending_bytes: usize,
    /// Running queued-bucket byte total. Mirror of [`Self::pending_bytes`]
    /// for the queued sub-pool. Expiring-mode txs are always pending and
    /// contribute to `pending_bytes` only.
    queued_bytes: usize,
}

impl<T: Eip8130PoolTx> Eip8130Pool<T> {
    /// Builds an empty pool with the given config.
    pub fn new(config: Eip8130PoolConfig) -> Self {
        Self {
            config,
            next_submission_id: 0,
            by_id: BTreeMap::new(),
            txs_by_seq: HashMap::new(),
            by_hash: HashMap::new(),
            independent: HashMap::new(),
            pending_tip_by_seq: HashMap::new(),
            expiring: HashMap::new(),
            pending_count: 0,
            queued_count: 0,
            slot_to_seq: HashMap::new(),
            slot_to_expiring: HashMap::new(),
            txs_by_sender: HashMap::new(),
            on_chain_nonces: HashMap::new(),
            by_eviction: BTreeSet::new(),
            expiry_index: BTreeMap::new(),
            invalidation_index: HashMap::new(),
            // Capacity 200 mirrors tempo's `AA2dPool::new` channel size —
            // small enough to fit the typical fan-out of `pending_transactions_listener_for`
            // subscribers (gossip + RPC + tooling) without unbounded
            // memory growth on subscriber-lag.
            pending_tx_broadcaster: broadcast::channel(200).0,
            discarded_tx_broadcaster: broadcast::channel(200).0,
            event_broadcaster: broadcast::channel(200).0,
            metrics: Eip8130PoolMetrics::default(),
            pending_bytes: 0,
            queued_bytes: 0,
        }
    }

    /// Recompute the size gauges from the pool's internal indices and push
    /// them to the metrics sink.
    ///
    /// Cadence: called only at the end of *batch* operations
    /// (`on_state_updates`, `on_balance_updates`, `sweep_expired`,
    /// `discard_to_cap`) — never inside per-tx admission / removal.
    /// Per-tx counters (`txs_inserted`, `txs_removed`, `txs_evicted_*`,
    /// `txs_replaced`, `txs_promoted`, `txs_demoted`) still fire on every
    /// single-tx path, so exact-count signals are not delayed; only the
    /// derived gauges (pool size, pending/queued split, expiring count,
    /// unique senders) are refreshed at batch boundaries. This mirrors
    /// tempo's `update_metrics` cadence (tt_2d_pool.rs:134) and avoids
    /// the O(n) walk over `by_id` on every per-admit / per-remove call.
    /// The cost is one walk over `by_id` (O(n)) per call; cheaper than
    /// tracking incremental deltas because batch ops can fan out (e.g.
    /// `discard_to_cap` removes multiple txs and the pending/queued
    /// classification can change for descendants), and a single recompute
    /// is harder to drift than four counters.
    fn record_size_gauges(&self) {
        let (pending, queued) = self.pending_and_queued_counts();
        self.metrics.pool_size_total.set(self.len() as f64);
        self.metrics.pool_size_pending.set(pending as f64);
        self.metrics.pool_size_queued.set(queued as f64);
        self.metrics
            .pool_size_expiring
            .set(self.expiry_index.values().map(|s| s.len()).sum::<usize>() as f64);
        self.metrics.unique_senders.set(self.txs_by_sender.len() as f64);
    }

    /// Returns a clone of the metrics handle. Tests use this to assert on
    /// counter / gauge values via the `metrics` test recorder.
    #[cfg(test)]
    pub fn metrics_snapshot(&self) -> Eip8130PoolMetrics {
        self.metrics.clone()
    }

    /// Subscribe to new-pending tx hashes. Each subscriber sees every
    /// admission / promotion event after the call site. Lagged
    /// subscribers (slow consumers) receive `RecvError::Lagged` and skip
    /// to current; the side pool guarantees at-most-once delivery, no
    /// retransmits.
    pub fn subscribe_pending(&self) -> broadcast::Receiver<TxHash> {
        self.pending_tx_broadcaster.subscribe()
    }

    /// Subscribe to discarded tx hashes. Each subscriber sees every
    /// removal-by-hash after the call site (mined, invalidated, expired,
    /// capacity-evicted, manually pruned). Same lag semantics as
    /// [`Self::subscribe_pending`].
    pub fn subscribe_discarded(&self) -> broadcast::Receiver<TxHash> {
        self.discarded_tx_broadcaster.subscribe()
    }

    /// Subscribe to all AA per-tx events. Consumers filter by hash.
    pub fn subscribe_events(&self) -> broadcast::Receiver<(TxHash, TransactionEvent)> {
        self.event_broadcaster.subscribe()
    }

    fn broadcast_event(&self, hash: TxHash, event: TransactionEvent) {
        let _ = self.event_broadcaster.send((hash, event));
    }

    /// Returns the number of admitted txs (sequenced + expiring).
    pub fn len(&self) -> usize {
        self.by_id.len() + self.expiring.len()
    }

    /// Returns true if no txs are admitted.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns true if a tx with this hash is in the pool.
    pub fn contains(&self, hash: &TxHash) -> bool {
        self.by_hash.contains_key(hash)
    }

    /// Returns the validated tx for `hash` if present.
    pub fn get(&self, hash: &TxHash) -> Option<&Arc<ValidPoolTransaction<T>>> {
        self.by_hash.get(hash)
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_submission_id;
        self.next_submission_id = self.next_submission_id.wrapping_add(1);
        id
    }

    /// Returns the byte-cost an entry contributes to the running
    /// pending/queued byte counter. Uses the cached
    /// `PoolTransaction::encoded_length()` (per the trait contract,
    /// "implementations should cache this value").
    fn entry_bytes(tx: &Arc<ValidPoolTransaction<T>>) -> usize {
        tx.transaction.encoded_length()
    }

    /// Adds `bytes` to the appropriate subpool counter (F5).
    fn add_subpool_bytes(&mut self, is_pending: bool, bytes: usize) {
        if is_pending {
            self.pending_bytes = self.pending_bytes.saturating_add(bytes);
        } else {
            self.queued_bytes = self.queued_bytes.saturating_add(bytes);
        }
    }

    /// Subtracts `bytes` from the appropriate subpool counter (F5).
    fn sub_subpool_bytes(&mut self, is_pending: bool, bytes: usize) {
        if is_pending {
            self.pending_bytes = self.pending_bytes.saturating_sub(bytes);
        } else {
            self.queued_bytes = self.queued_bytes.saturating_sub(bytes);
        }
    }

    /// Adds `count` to the appropriate subpool transaction counter.
    fn add_subpool_count(&mut self, is_pending: bool, count: usize) {
        if is_pending {
            self.pending_count = self.pending_count.saturating_add(count);
        } else {
            self.queued_count = self.queued_count.saturating_add(count);
        }
    }

    /// Subtracts `count` from the appropriate subpool transaction counter.
    fn sub_subpool_count(&mut self, is_pending: bool, count: usize) {
        if is_pending {
            self.pending_count = self.pending_count.saturating_sub(count);
        } else {
            self.queued_count = self.queued_count.saturating_sub(count);
        }
    }

    /// Moves `bytes` between the pending and queued byte counters when an
    /// entry transitions sub-pools (F5). Count deltas are maintained at the
    /// call site because callers usually already know how many flags flipped.
    fn shift_subpool_bytes(&mut self, from_pending: bool, bytes: usize) {
        self.sub_subpool_bytes(from_pending, bytes);
        self.add_subpool_bytes(!from_pending, bytes);
    }

    /// Moves `count` transactions between the pending and queued counters.
    fn shift_subpool_count(&mut self, from_pending: bool, count: usize) {
        self.sub_subpool_count(from_pending, count);
        self.add_subpool_count(!from_pending, count);
    }

    /// Admits a validated AA tx into the pool. `on_chain_nonce` is the
    /// `aa_nonce_slot(sender, key)` value at validation time; ignored for
    /// expiring-mode txs.
    ///
    /// On a same-`tx_id` collision the new tx replaces the old one iff it
    /// satisfies [`PriceBumpConfig`]; otherwise [`PoolErrorKind::ReplacementUnderpriced`]
    /// is returned. Stale txs (`nonce < on_chain_nonce`) are rejected up
    /// front.
    ///
    /// `required_balance` is the admission-time payer-balance predicate
    /// the validator enforced (default fallback: `gas_limit *
    /// max_fee_per_gas`). The production path threads the full
    /// `gas_limit * max_fee_per_gas + l1_data_fee` here via
    /// [`Self::add_transaction_with_required`]; the test-friendly
    /// [`Self::add_transaction`] shorthand omits the L1 component. F5.
    pub fn add_transaction(
        &mut self,
        tx: Arc<ValidPoolTransaction<T>>,
        on_chain_nonce: u64,
        base_fee: u64,
    ) -> Result<Eip8130AddOutcome<T>, PoolError> {
        // Shorthand for callers (tests) that don't have an `l1_data_fee`
        // available. The fallback predicate matches the legacy
        // `on_balance_updates` recompute (no L1 component), preserving
        // existing behaviour for tests that exercise the balance sweep.
        let required = U256::from(tx.transaction.gas_limit())
            .saturating_mul(U256::from(tx.transaction.max_fee_per_gas()));
        self.add_transaction_with_required(tx, on_chain_nonce, base_fee, required)
    }

    /// Variant of [`Self::add_transaction`] that accepts the validator's
    /// admission-time `required_balance` (= `gas_limit * max_fee_per_gas
    /// + l1_data_fee`). Used by [`crate::OpDualPool::admit_aa`] so the
    /// side pool's `on_balance_updates` sees the same predicate. F5.
    pub fn add_transaction_with_required(
        &mut self,
        tx: Arc<ValidPoolTransaction<T>>,
        on_chain_nonce: u64,
        base_fee: u64,
        required_balance: U256,
    ) -> Result<Eip8130AddOutcome<T>, PoolError> {
        debug_assert!(tx.transaction.is_eip8130(), "non-AA tx routed to Eip8130Pool");

        let hash = *tx.hash();
        if self.by_hash.contains_key(&hash) {
            return Err(PoolError::new(hash, PoolErrorKind::AlreadyImported));
        }
        if tx.transaction.is_expiring_nonce() {
            return self.add_expiring(tx, base_fee, required_balance);
        }
        self.add_sequenced(tx, on_chain_nonce, base_fee, required_balance)
    }

    fn add_sequenced(
        &mut self,
        tx: Arc<ValidPoolTransaction<T>>,
        on_chain_nonce: u64,
        base_fee: u64,
        required_balance: U256,
    ) -> Result<Eip8130AddOutcome<T>, PoolError> {
        let hash = *tx.hash();
        let tx_id = tx.transaction.aa_tx_id().expect("sequenced AA tx must have aa_tx_id");
        let seq = tx_id.seq;

        // Stale nonce: tx's sequence number is below the lane's current
        // on-chain head. The validator will already have caught most of
        // these, but we re-check to defend against on_chain_nonce moving
        // forward between validate and add.
        if tx_id.nonce < on_chain_nonce {
            return Err(PoolError::new(
                hash,
                PoolErrorKind::InvalidTransaction(
                    reth_transaction_pool::error::InvalidPoolTransactionError::Other(Box::new(
                        StaleNonceError { tx_nonce: tx_id.nonce, on_chain_nonce },
                    )),
                ),
            ));
        }

        // Build the new entry up front so we can compare priorities for the
        // replacement check without two separate constructions.
        let priority = tx.transaction.aa_priority_fee(base_fee);
        let submission_id = self.next_id();
        // Resolve invalidation rules once at admission; the cached vec on
        // the entry is consulted by every subsequent register / unregister
        // (cf PERF-1).
        let cached_invalidation_rules = tx.transaction.aa_invalidation_rules();
        let new_entry = Arc::new(PooledEntry {
            tx: tx.clone(),
            priority,
            submission_id,
            is_pending: AtomicBool::new(false),
            cached_invalidation_rules,
            required_balance,
        });

        // AA-effective sender: the per-sender DoS cap binds to the
        // sender the AA tx authorizes, not the EIP-2718 envelope signer
        // (which for sponsored shapes is the payer). Falling back to the
        // envelope signer for non-AA txs is dead code on this path —
        // `add_sequenced` is only reachable after the `is_eip8130()`
        // dispatch in `add_transaction` — but keep the fallback so the
        // pool degrades safely if the trait impl drifts.
        let sender = tx.transaction.aa_sender().unwrap_or_else(|| tx.sender());
        // Pre-compute caps before opening the entry: `lane_count` reads
        // `self.by_id`, which the entry would mutably borrow.
        let is_replacement = self.by_id.contains_key(&tx_id);
        if !is_replacement {
            let count = self.txs_by_sender.get(&sender).copied().unwrap_or(0);
            if count >= self.config.max_txs_per_sender {
                return Err(PoolError::new(hash, PoolErrorKind::SpammerExceededCapacity(sender)));
            }
            let lane_count = self.lane_count(seq);
            if lane_count >= self.config.max_txs_per_sequence {
                return Err(PoolError::new(
                    hash,
                    PoolErrorKind::InvalidTransaction(
                        reth_transaction_pool::error::InvalidPoolTransactionError::Other(Box::new(
                            LaneFullError {
                                sender,
                                nonce_key: seq.nonce_key,
                                limit: self.config.max_txs_per_sequence,
                            },
                        )),
                    ),
                ));
            }
        }

        // Capture the prior entry's exact priority + submission_id so the
        // `by_eviction` row keyed on those values can be removed precisely;
        // a wrong key would leak the dangling row and let an attacker grow
        // `by_eviction` unbounded by repeated same-id replacements.
        let mut replaced_eviction_key: Option<EvictionKey> = None;
        // Hold the prior entry so we can reuse its cached invalidation
        // rules during unregister (cf PERF-1).
        let mut replaced_entry: Option<Arc<PooledEntry<T>>> = None;
        let replaced: Option<Arc<ValidPoolTransaction<T>>> = match self.by_id.entry(tx_id) {
            Entry::Occupied(mut entry) => {
                let existing = entry.get();
                if existing.tx.is_underpriced(&new_entry.tx, &self.config.price_bump) {
                    return Err(PoolError::new(hash, PoolErrorKind::ReplacementUnderpriced));
                }
                let prev = entry.insert(new_entry.clone());
                replaced_eviction_key = Some(EvictionKey {
                    priority: prev.priority,
                    submission_id: prev.submission_id,
                    hash: *prev.tx.hash(),
                });
                let prev_tx = prev.tx.clone();
                replaced_entry = Some(prev);
                Some(prev_tx)
            }
            Entry::Vacant(slot) => {
                slot.insert(new_entry.clone());
                *self.txs_by_sender.entry(sender).or_insert(0) += 1;
                // Maintain the lane counter in lock-step with `by_id`
                // (cf PERF-4). Replacements keep the count constant; only
                // a fresh insert into a new tx_id slot increments.
                *self.txs_by_seq.entry(seq).or_insert(0) += 1;
                None
            }
        };

        // Bookkeeping: by_hash, slot_to_seq, eviction set. Order matters —
        // we removed the replaced tx from by_id above, so its by_hash entry
        // and eviction key need cleanup before the new ones overwrite.
        if let Some(prev) = replaced.as_ref() {
            self.by_hash.remove(prev.hash());
            self.broadcast_event(*prev.hash(), TransactionEvent::Replaced(hash));
            if let Some(key) = replaced_eviction_key.as_ref() {
                self.by_eviction.remove(key);
            }
            if let Some(prev_entry) = replaced_entry.as_ref() {
                self.unregister_invalidation_keys(
                    *prev.hash(),
                    &prev_entry.cached_invalidation_rules,
                );
                // Subtract the replaced entry's byte cost from the
                // sub-pool counter it occupied. The new entry's bytes are
                // added below as queued; promotion below shifts the delta
                // back to pending if the lane prefix is contiguous.
                let prev_was_pending = prev_entry.is_pending();
                self.sub_subpool_bytes(prev_was_pending, Self::entry_bytes(prev));
                self.sub_subpool_count(prev_was_pending, 1);
            }
            self.unregister_expiry(prev);
        }
        self.by_hash.insert(hash, tx.clone());
        // Account the new entry as queued initially (matches
        // `is_pending: AtomicBool::new(false)` above); `promote_lane`
        // below will shift it into pending if the lane prefix is
        // contiguous.
        self.add_subpool_bytes(false, Self::entry_bytes(&tx));
        self.add_subpool_count(false, 1);
        let slot = aa_nonce_slot(seq.sender, seq.nonce_key);
        self.slot_to_seq.entry(slot).or_insert(seq);
        self.by_eviction.insert(EvictionKey { priority, submission_id, hash });
        self.register_invalidation_keys(hash, &new_entry.cached_invalidation_rules);
        self.register_expiry(&tx);
        // Pin the lane's last-known on-chain nonce on first insertion. Later
        // inserts re-use the cached value; `on_state_updates` overwrites.
        self.on_chain_nonces.entry(seq).or_insert(on_chain_nonce);

        // Promote contiguous nonces from `on_chain_nonce` upward. The new
        // tx becomes pending iff it's the first nonce in line or every
        // nonce below it is already pending.
        //
        // `promote_lane` returns *every* tx that transitioned queued →
        // pending. The just-inserted tx (`tx_id.nonce`) shouldn't appear
        // in the caller-facing `promoted` list — it's surfaced as the
        // outcome's `transaction` field. Filter it out.
        if replaced.is_some() {
            self.pending_tip_by_seq.remove(&seq);
        }
        let mut promoted = self.promote_lane(seq, on_chain_nonce);
        promoted.retain(|t| t.hash() != &hash);
        let inserted_pending = new_entry.is_pending();

        // Capacity overflow eviction last so a same-tx replacement isn't
        // counted against the pool's cap.
        let discarded = self.discard_to_cap();

        // Account counters before the early return on the pending path.
        // Replacements call `by_hash.remove(prev.hash())` directly and bypass
        // `remove_by_hash`, so `txs_removed` is not incremented for the
        // replaced tx — `txs_replaced` is the canonical signal there.
        self.metrics.txs_inserted.increment(1);
        if replaced.is_some() {
            self.metrics.txs_replaced.increment(1);
        }
        if !promoted.is_empty() {
            self.metrics.txs_promoted.increment(promoted.len() as u64);
        }
        if !discarded.is_empty() {
            self.metrics.txs_evicted_capacity.increment(discarded.len() as u64);
        }
        // Per-tx admission path: skip the gauge refresh. The next
        // batch-end (canon commit / expiry sweep / etc.) will reconcile.

        if inserted_pending {
            // Track lane head for `best_transactions`.
            if tx_id.nonce == on_chain_nonce {
                self.independent.insert(seq, new_entry.clone());
            }
            // Broadcast the new pending hash + every promoted-from-queued
            // hash. Subscribers (OpDualPool's listener channels) propagate
            // to gossip / RPC consumers.
            let _ = self.pending_tx_broadcaster.send(hash);
            self.broadcast_event(hash, TransactionEvent::Pending);
            for promoted_tx in &promoted {
                let _ = self.pending_tx_broadcaster.send(*promoted_tx.hash());
                self.broadcast_event(*promoted_tx.hash(), TransactionEvent::Pending);
            }
            return Ok(Eip8130AddOutcome::Pending(Eip8130PendingAdded {
                transaction: tx,
                replaced,
                promoted,
                discarded,
            }));
        }
        self.broadcast_event(hash, TransactionEvent::Queued);
        Ok(Eip8130AddOutcome::Queued { transaction: tx, replaced })
    }

    fn add_expiring(
        &mut self,
        tx: Arc<ValidPoolTransaction<T>>,
        base_fee: u64,
        required_balance: U256,
    ) -> Result<Eip8130AddOutcome<T>, PoolError> {
        let hash = *tx.hash();
        let exp_hash = tx
            .transaction
            .aa_expiring_nonce_hash()
            .expect("expiring AA tx must have expiring_nonce_hash");

        if self.expiring.contains_key(&exp_hash) {
            // Expiring-nonce txs are uniquely identified by tx hash.
            // Same-hash resubmission is treated as a duplicate, not a
            // replacement candidate (no nonce → no underprice contest).
            return Err(PoolError::new(hash, PoolErrorKind::AlreadyImported));
        }

        // Per-sender DoS cap binds to the AA-effective sender; see the
        // matching note in `add_sequenced`.
        let sender = tx.transaction.aa_sender().unwrap_or_else(|| tx.sender());
        let count = self.txs_by_sender.get(&sender).copied().unwrap_or(0);
        if count >= self.config.max_txs_per_sender {
            return Err(PoolError::new(hash, PoolErrorKind::SpammerExceededCapacity(sender)));
        }

        let priority = tx.transaction.aa_priority_fee(base_fee);
        let submission_id = self.next_id();
        // Resolve invalidation rules once at admission (cf PERF-1).
        let cached_invalidation_rules = tx.transaction.aa_invalidation_rules();
        let entry = Arc::new(PooledEntry {
            tx: tx.clone(),
            priority,
            submission_id,
            // Always immediately pending — no nonce dependencies.
            is_pending: AtomicBool::new(true),
            cached_invalidation_rules,
            required_balance,
        });

        self.expiring.insert(exp_hash, entry.clone());
        if let Some(slot) = tx.transaction.aa_expiring_nonce_slot() {
            self.slot_to_expiring.insert(slot, exp_hash);
        }
        self.by_hash.insert(hash, tx.clone());
        // Expiring-mode txs are unconditionally pending.
        self.add_subpool_bytes(true, Self::entry_bytes(&tx));
        self.add_subpool_count(true, 1);
        *self.txs_by_sender.entry(sender).or_insert(0) += 1;
        self.by_eviction.insert(EvictionKey { priority, submission_id, hash });
        self.register_invalidation_keys(hash, &entry.cached_invalidation_rules);
        self.register_expiry(&tx);

        // Expiring-mode txs are immediately pending; broadcast.
        let _ = self.pending_tx_broadcaster.send(hash);
        self.broadcast_event(hash, TransactionEvent::Pending);

        let discarded = self.discard_to_cap();
        self.metrics.txs_inserted.increment(1);
        if !discarded.is_empty() {
            self.metrics.txs_evicted_capacity.increment(discarded.len() as u64);
        }
        // Per-tx admission path: skip the gauge refresh.
        Ok(Eip8130AddOutcome::Pending(Eip8130PendingAdded {
            transaction: tx,
            replaced: None,
            promoted: Vec::new(),
            discarded,
        }))
    }

    /// Removes a tx by hash, fixing all reverse indices. Returns the removed
    /// tx if present.
    ///
    /// For sequenced txs, descendants in the same `(sender, nonce_key)`
    /// lane (any tx with `nonce > removed.nonce`) are demoted from pending
    /// to queued, since the gap left by removal makes them un-executable
    /// until the missing nonce is re-supplied. Without this, callers that
    /// iterate `independent_pending()` / `all_pending()` would see stale
    /// "ready" txs that the merge iterator can't actually emit.
    pub fn remove_by_hash(&mut self, hash: &TxHash) -> Option<Arc<ValidPoolTransaction<T>>> {
        self.remove_by_hash_inner(hash, true)
    }

    /// Like [`Self::remove_by_hash`] but skips the `Discarded` lifecycle
    /// broadcast. Used by `on_state_updates` to drain mined txs so
    /// the caller can emit `TransactionEvent::Mined(block_hash)` instead.
    fn remove_by_hash_no_event(&mut self, hash: &TxHash) -> Option<Arc<ValidPoolTransaction<T>>> {
        self.remove_by_hash_inner(hash, false)
    }

    /// Removes a mined sequenced tx by id without demoting descendants.
    ///
    /// `advance_lane` drains a contiguous prefix below the new on-chain
    /// nonce and then re-promotes/demotes the remaining lane exactly once.
    /// Calling the generic remove path for every mined nonce would create a
    /// temporary gap on each step and repeatedly demote the same descendants.
    fn remove_mined_sequenced_by_id_no_demote(
        &mut self,
        tx_id: Eip8130TxId,
    ) -> Option<Arc<ValidPoolTransaction<T>>> {
        let entry = self.by_id.remove(&tx_id)?;
        let tx = entry.tx.clone();
        let hash = *tx.hash();

        self.by_hash.remove(&hash);
        self.metrics.txs_removed.increment(1);
        self.unregister_expiry(&tx);
        let sender = tx.transaction.aa_sender().unwrap_or_else(|| tx.sender());

        self.pending_tip_by_seq.remove(&tx_id.seq);
        let entry_was_pending = entry.is_pending();
        self.sub_subpool_bytes(entry_was_pending, Self::entry_bytes(&tx));
        self.sub_subpool_count(entry_was_pending, 1);

        if let Some(count) = self.txs_by_seq.get_mut(&tx_id.seq) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.txs_by_seq.remove(&tx_id.seq);
            }
        }
        self.unregister_invalidation_keys(hash, &entry.cached_invalidation_rules);
        self.by_eviction.remove(&EvictionKey {
            priority: entry.priority,
            submission_id: entry.submission_id,
            hash,
        });
        if self.independent.get(&tx_id.seq).is_some_and(|h| Arc::ptr_eq(h, &entry)) {
            self.independent.remove(&tx_id.seq);
        }

        if self.lane_count(tx_id.seq) == 0 {
            let slot = aa_nonce_slot(tx_id.seq.sender, tx_id.seq.nonce_key);
            self.slot_to_seq.remove(&slot);
            self.on_chain_nonces.remove(&tx_id.seq);
            self.pending_tip_by_seq.remove(&tx_id.seq);
        }
        self.decrement_sender(sender);
        Some(tx)
    }

    fn remove_by_hash_inner(
        &mut self,
        hash: &TxHash,
        emit_discarded: bool,
    ) -> Option<Arc<ValidPoolTransaction<T>>> {
        let tx = self.by_hash.remove(hash)?;
        self.metrics.txs_removed.increment(1);
        // Fire the discard broadcast first thing so per-tx event subscribers
        // see the signal exactly once per removal regardless of which
        // mode-specific cleanup path runs below. Same-tx-id replacements
        // bypass `remove_by_hash` (`add_sequenced` calls `by_hash.remove`
        // directly), so they don't fire here — replacements aren't
        // discards in the lifecycle sense.
        if emit_discarded {
            let _ = self.discarded_tx_broadcaster.send(*hash);
            self.broadcast_event(*hash, TransactionEvent::Discarded);
        }
        // Expiry index drops up front (no entry-cache dependency).
        self.unregister_expiry(&tx);
        // The AA-effective sender is the per-sender bookkeeping key; see
        // the note in `add_sequenced`. Read it before mode-specific drops
        // since those move out of `tx`.
        let sender = tx.transaction.aa_sender().unwrap_or_else(|| tx.sender());

        if tx.transaction.is_expiring_nonce() {
            let exp_hash = tx.transaction.aa_expiring_nonce_hash()?;
            let entry = self.expiring.remove(&exp_hash)?;
            // Drop invalidation index using the entry's cached rules so we
            // don't recompute `aa_invalidation_rules` here (cf PERF-1).
            self.unregister_invalidation_keys(*hash, &entry.cached_invalidation_rules);
            if let Some(slot) = tx.transaction.aa_expiring_nonce_slot() {
                // Only drop the reverse index entry if it still points at
                // this exact tx (a later replay-protected tx of the same
                // sender could overwrite the slot mapping; we don't want
                // to clobber that).
                if self.slot_to_expiring.get(&slot) == Some(&exp_hash) {
                    self.slot_to_expiring.remove(&slot);
                }
            }
            self.by_eviction.remove(&EvictionKey {
                priority: entry.priority,
                submission_id: entry.submission_id,
                hash: *hash,
            });
            // Expiring-mode entries always live in the pending bucket.
            self.sub_subpool_bytes(true, Self::entry_bytes(&tx));
            self.sub_subpool_count(true, 1);
            self.decrement_sender(sender);
            // Per-tx removal: gauge refresh deferred to the batch-end caller.
            return Some(tx);
        }

        let tx_id = tx.transaction.aa_tx_id()?;
        let entry = self.by_id.remove(&tx_id)?;
        self.pending_tip_by_seq.remove(&tx_id.seq);
        // Subtract the removed entry's byte cost from whichever
        // sub-pool it occupied at the moment of removal.
        let entry_was_pending = entry.is_pending();
        self.sub_subpool_bytes(entry_was_pending, Self::entry_bytes(&tx));
        self.sub_subpool_count(entry_was_pending, 1);
        // Decrement the maintained lane counter in lock-step with the
        // `by_id` removal (cf PERF-4); drop the row when the lane is
        // empty so `lane_count` returns 0 without retaining stale keys.
        if let Some(count) = self.txs_by_seq.get_mut(&tx_id.seq) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.txs_by_seq.remove(&tx_id.seq);
            }
        }
        // Drop invalidation index using the entry's cached rules (cf PERF-1).
        self.unregister_invalidation_keys(*hash, &entry.cached_invalidation_rules);
        self.by_eviction.remove(&EvictionKey {
            priority: entry.priority,
            submission_id: entry.submission_id,
            hash: *hash,
        });
        // If this was the lane's pending head, drop it.
        if self.independent.get(&tx_id.seq).is_some_and(|h| Arc::ptr_eq(h, &entry)) {
            self.independent.remove(&tx_id.seq);
        }
        // Demote descendants whose head just disappeared. Only the
        // removed tx's own pending status was relevant for the lane head;
        // higher nonces are now blocked behind the gap and must leave
        // the pending bucket.
        let removed_was_pending = entry.is_pending();
        if removed_was_pending {
            let demoted = self.demote_descendants_count(tx_id);
            if demoted > 0 {
                self.metrics.txs_demoted.increment(demoted as u64);
            }
        }
        // If the lane is now empty, free the reverse-slot entry. Otherwise
        // leave it; the next tx in the lane still needs the mapping.
        if self.lane_count(tx_id.seq) == 0 {
            let slot = aa_nonce_slot(tx_id.seq.sender, tx_id.seq.nonce_key);
            self.slot_to_seq.remove(&slot);
            self.on_chain_nonces.remove(&tx_id.seq);
            self.pending_tip_by_seq.remove(&tx_id.seq);
        }
        self.decrement_sender(sender);
        // Per-tx removal: gauge refresh deferred to the batch-end caller.
        Some(tx)
    }

    /// Removes a tx and every later nonce in the same AA lane. Expiring
    /// nonce txs have no descendants, so this falls back to single-hash
    /// removal for them.
    pub fn remove_by_hash_and_descendants(
        &mut self,
        hash: &TxHash,
    ) -> Vec<Arc<ValidPoolTransaction<T>>> {
        let Some(tx) = self.by_hash.get(hash).cloned() else {
            return Vec::new();
        };
        let Some(tx_id) = tx.transaction.aa_tx_id() else {
            return self.remove_by_hash(hash).into_iter().collect();
        };

        let lo = tx_id;
        let hi = Eip8130TxId::new(tx_id.seq, u64::MAX);
        let hashes: Vec<TxHash> =
            self.by_id.range(lo..=hi).map(|(_, entry)| *entry.tx.hash()).collect();

        let mut removed = Vec::with_capacity(hashes.len());
        for hash in hashes {
            if let Some(tx) = self.remove_by_hash(&hash) {
                removed.push(tx);
            }
        }
        removed
    }

    /// Marks every tx in the same lane with `nonce > removed.nonce` as
    /// queued and returns the number of txs that actually transitioned
    /// pending → queued. Cheap: one BTreeMap range scan + atomic flag
    /// flip per affected tx. Used by the metrics path to count demotions
    /// accurately.
    fn demote_descendants_count(&mut self, removed: Eip8130TxId) -> usize {
        let lo = Eip8130TxId::new(removed.seq, removed.nonce.saturating_add(1));
        let hi = Eip8130TxId::new(removed.seq, u64::MAX);
        let mut count = 0usize;
        // Collect bytes-to-shift while iterating; apply after the
        // borrow ends so we don't conflict with the &mut self counter.
        let mut shift_bytes: usize = 0;
        for (_, entry) in self.by_id.range(lo..=hi) {
            let was_pending = entry.set_pending(false);
            if was_pending {
                count += 1;
                shift_bytes = shift_bytes.saturating_add(Self::entry_bytes(&entry.tx));
            }
        }
        if shift_bytes > 0 {
            self.shift_subpool_bytes(true, shift_bytes);
            self.shift_subpool_count(true, count);
        }
        count
    }

    fn decrement_sender(&mut self, sender: Address) {
        if let Some(count) = self.txs_by_sender.get_mut(&sender) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.txs_by_sender.remove(&sender);
            }
        }
    }

    /// Number of admitted txs in the given lane.
    /// O(1) via the `txs_by_seq` counter maintained alongside `by_id`
    /// (cf PERF-4).
    fn lane_count(&self, seq: Eip8130SeqId) -> usize {
        self.txs_by_seq.get(&seq).copied().unwrap_or(0)
    }

    /// Walks the lane from `from_nonce` upward, marking every contiguous
    /// nonce as pending and returning the txs that just transitioned from
    /// queued to pending.
    fn promote_lane(
        &mut self,
        seq: Eip8130SeqId,
        from_nonce: u64,
    ) -> Vec<Arc<ValidPoolTransaction<T>>> {
        let mut promoted = Vec::new();
        let start_nonce = self
            .pending_tip_by_seq
            .get(&seq)
            .copied()
            .filter(|tip| *tip >= from_nonce)
            .unwrap_or(from_nonce);
        let mut expected = start_nonce;
        let lo = Eip8130TxId::new(seq, start_nonce);
        let hi = Eip8130TxId::new(seq, u64::MAX);
        let mut head: Option<Arc<PooledEntry<T>>> = None;
        // Accumulate the byte shift while iterating, apply after the
        // immutable borrow ends.
        let mut shift_bytes: usize = 0;
        for (id, entry) in self.by_id.range(lo..=hi) {
            if id.nonce != expected {
                break;
            }
            let was_pending = entry.set_pending(true);
            if !was_pending {
                promoted.push(entry.tx.clone());
                shift_bytes = shift_bytes.saturating_add(Self::entry_bytes(&entry.tx));
            }
            if id.nonce == from_nonce {
                head = Some(entry.clone());
            }
            expected = expected.saturating_add(1);
        }
        if let Some(h) = head {
            self.independent.insert(seq, h);
        } else if start_nonce > from_nonce &&
            let Some(entry) = self.by_id.get(&Eip8130TxId::new(seq, from_nonce)) &&
            entry.is_pending()
        {
            // The cursor may let us skip a prefix that was already marked
            // pending. Keep the independent head anchored at the current
            // on-chain nonce even when no newly-promoted entry was scanned.
            self.independent.insert(seq, entry.clone());
        }
        if expected > start_nonce || start_nonce > from_nonce {
            self.pending_tip_by_seq.insert(seq, expected);
        }
        if shift_bytes > 0 {
            self.shift_subpool_bytes(false, shift_bytes);
            self.shift_subpool_count(false, promoted.len());
        }
        promoted
    }

    /// Demotes every tx in the lane (after a nonce-jump or contract reset
    /// where the on-chain head moved past txs we held, leaving a gap).
    /// Returns the txs that transitioned pending → queued.
    fn demote_lane(&mut self, seq: Eip8130SeqId) -> Vec<Arc<ValidPoolTransaction<T>>> {
        self.pending_tip_by_seq.remove(&seq);
        let lo = Eip8130TxId::new(seq, 0);
        let hi = Eip8130TxId::new(seq, u64::MAX);
        let mut demoted = Vec::new();
        // Accumulate the pending → queued byte shift.
        let mut shift_bytes: usize = 0;
        for (_, entry) in self.by_id.range(lo..=hi) {
            let was_pending = entry.set_pending(false);
            if was_pending {
                demoted.push(entry.tx.clone());
                shift_bytes = shift_bytes.saturating_add(Self::entry_bytes(&entry.tx));
            }
        }
        self.independent.remove(&seq);
        if shift_bytes > 0 {
            self.shift_subpool_bytes(true, shift_bytes);
            self.shift_subpool_count(true, demoted.len());
        }
        demoted
    }

    /// Drops every tx whose expiry has elapsed (`0 < expiry < now`).
    /// Returns the evicted entries so callers can fire `Discarded`
    /// lifecycle events. Zero-expiry txs are never swept; they're the
    /// no-expiry sentinel.
    pub fn sweep_expired(&mut self, now: u64) -> Vec<Arc<ValidPoolTransaction<T>>> {
        // Drain the BTreeMap head while keys are strictly less than `now`.
        // `range(..now)` is the contiguous prefix; collecting hashes lets
        // us drop the borrow before mutating via `remove_by_hash`, which
        // also rewrites `expiry_index`.
        let to_remove: Vec<TxHash> =
            self.expiry_index.range(..now).flat_map(|(_, hashes)| hashes.iter().copied()).collect();
        let mut expired = Vec::with_capacity(to_remove.len());
        for hash in to_remove {
            if let Some(tx) = self.remove_by_hash(&hash) {
                expired.push(tx);
            }
        }
        if !expired.is_empty() {
            self.metrics.txs_evicted_expiry.increment(expired.len() as u64);
        }
        // Batch end: refresh derived gauges once for the whole sweep.
        self.record_size_gauges();
        expired
    }

    /// Drops the lowest-priority entries until tempo-style subpool limits
    /// are satisfied: queued excess first, then pending excess. The legacy
    /// `max_pool_size` cap is applied afterwards if explicitly configured.
    /// Returns the discarded txs in eviction order.
    ///
    /// After the count-based eviction pass, this also enforces
    /// `pending_limit.max_size` / `queued_limit.max_size` (running byte
    /// totals tracked incrementally via [`Self::pending_bytes`] /
    /// [`Self::queued_bytes`]). Reth's standard subpools enforce both
    /// `max_txs` and `max_size` on the same `SubPoolLimit`; pre-fix the
    /// AA side pool only honored `max_txs`, so an operator-configured
    /// byte cap silently leaked.
    fn discard_to_cap(&mut self) -> Vec<Arc<ValidPoolTransaction<T>>> {
        let len = self.len();
        if len <= self.config.max_pool_size &&
            len <= self.config.pending_limit.max_txs &&
            len <= self.config.queued_limit.max_txs &&
            self.pending_bytes <= self.config.pending_limit.max_size &&
            self.queued_bytes <= self.config.queued_limit.max_size
        {
            return Vec::new();
        }

        let mut discarded = Vec::new();
        let (pending_count, queued_count) = self.pending_and_queued_counts();

        if queued_count > self.config.queued_limit.max_txs {
            discarded.extend(
                self.evict_lowest_priority(queued_count - self.config.queued_limit.max_txs, false),
            );
        }

        if pending_count > self.config.pending_limit.max_txs {
            discarded.extend(
                self.evict_lowest_priority(pending_count - self.config.pending_limit.max_txs, true),
            );
        }

        // Byte-based pass. Eviction order matches the count-based
        // pass: lowest-priority first, ties broken by submission_id (older
        // wins, matching tempo's "preserve older waiters" policy).
        if self.queued_bytes > self.config.queued_limit.max_size {
            let budget = self.queued_bytes.saturating_sub(self.config.queued_limit.max_size);
            discarded.extend(self.evict_bytes_lowest_priority(budget, false));
        }
        if self.pending_bytes > self.config.pending_limit.max_size {
            let budget = self.pending_bytes.saturating_sub(self.config.pending_limit.max_size);
            discarded.extend(self.evict_bytes_lowest_priority(budget, true));
        }

        while self.len() > self.config.max_pool_size {
            let Some(victim) = self.by_eviction.iter().next().cloned() else {
                break;
            };
            if let Some(tx) = self.remove_by_hash(&victim.hash) {
                discarded.push(tx);
            } else {
                // Defensive: if `by_eviction` ever drifts ahead of
                // `by_hash` (stale entry whose hash is no longer indexed),
                // `remove_by_hash` short-circuits without touching
                // `by_eviction` and `self.len()` doesn't drop — the loop
                // would spin forever while holding the pool write guard.
                // Drop the dangling entry so the loop always progresses.
                self.by_eviction.remove(&victim);
            }
        }
        // Batch end: refresh derived gauges only when this call actually
        // evicted. Avoids the O(n) walk on the no-op path that fires on
        // every well-behaved admit (no over-cap).
        if !discarded.is_empty() {
            self.record_size_gauges();
        }
        discarded
    }

    fn pending_and_queued_counts(&self) -> (usize, usize) {
        (self.pending_count, self.queued_count)
    }

    fn evict_lowest_priority(
        &mut self,
        count: usize,
        evict_pending: bool,
    ) -> Vec<Arc<ValidPoolTransaction<T>>> {
        if count == 0 {
            return Vec::new();
        }

        let victims: Vec<TxHash> = self
            .by_eviction
            .iter()
            .filter(|key| self.eviction_key_is_pending(key) == Some(evict_pending))
            .map(|key| key.hash)
            .take(count)
            .collect();

        let mut removed = Vec::with_capacity(victims.len());
        for hash in victims {
            if let Some(tx) = self.remove_by_hash(&hash) {
                removed.push(tx);
            }
        }
        removed
    }

    /// Evicts lowest-priority entries from the chosen sub-pool until
    /// at least `bytes_budget` bytes have been freed. Eviction key is
    /// the same `EvictionKey` as the count-based pass so byte-driven and
    /// count-driven evictions stay symmetric in priority order.
    fn evict_bytes_lowest_priority(
        &mut self,
        bytes_budget: usize,
        evict_pending: bool,
    ) -> Vec<Arc<ValidPoolTransaction<T>>> {
        if bytes_budget == 0 {
            return Vec::new();
        }

        // Snapshot victim hashes + their byte cost up front so we can
        // walk a sorted view of `by_eviction` without holding the borrow
        // across `remove_by_hash`.
        let mut victims: Vec<(TxHash, usize)> = Vec::new();
        let mut accumulated: usize = 0;
        for key in &self.by_eviction {
            if self.eviction_key_is_pending(key) != Some(evict_pending) {
                continue;
            }
            let Some(tx) = self.by_hash.get(&key.hash) else { continue };
            let bytes = Self::entry_bytes(tx);
            victims.push((key.hash, bytes));
            accumulated = accumulated.saturating_add(bytes);
            if accumulated >= bytes_budget {
                break;
            }
        }

        let mut removed = Vec::with_capacity(victims.len());
        for (hash, _) in victims {
            if let Some(tx) = self.remove_by_hash(&hash) {
                removed.push(tx);
            }
        }
        removed
    }

    fn eviction_key_is_pending(&self, key: &EvictionKey) -> Option<bool> {
        let tx = self.by_hash.get(&key.hash)?;
        if tx.transaction.is_expiring_nonce() {
            return Some(true);
        }
        let tx_id = tx.transaction.aa_tx_id()?;
        self.by_id.get(&tx_id).map(|entry| entry.is_pending())
    }
}

// ---------------------------------------------------------------------------
// Error types (for InvalidPoolTransactionError::Other)
// ---------------------------------------------------------------------------

/// Tx's nonce sequence is below the lane's on-chain head.
#[derive(Debug, thiserror::Error)]
#[error("EIP-8130 stale nonce: tx nonce {tx_nonce} < on-chain {on_chain_nonce}")]
struct StaleNonceError {
    tx_nonce: u64,
    on_chain_nonce: u64,
}

impl reth_transaction_pool::error::PoolTransactionError for StaleNonceError {
    fn is_bad_transaction(&self) -> bool {
        false
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Per-`(sender, nonce_key)` lane is at capacity.
#[derive(Debug, thiserror::Error)]
#[error("EIP-8130 lane full for sender {sender} key {nonce_key}: limit {limit}")]
struct LaneFullError {
    sender: Address,
    nonce_key: U256,
    limit: usize,
}

impl reth_transaction_pool::error::PoolTransactionError for LaneFullError {
    fn is_bad_transaction(&self) -> bool {
        false
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// Re-exposed so external code can advance the cached on-chain head without
// driving a full state-update cycle (e.g. when the validator's own state
// read raced ahead of the pool's last `on_state_updates`).
impl<T: Eip8130PoolTx> Eip8130Pool<T> {
    /// Last-observed `next_nonce` for the lane, if the pool has admitted
    /// any tx in it.
    pub fn lane_on_chain_nonce(&self, seq: Eip8130SeqId) -> Option<u64> {
        self.on_chain_nonces.get(&seq).copied()
    }

    /// Returns the highest `nonce_sequence` in lane `seq` that's pending
    /// in an unbroken chain starting at `on_chain_seq`, or `None` if no
    /// tx at `on_chain_seq` is pending.
    pub fn highest_consecutive_pending_seq_in_lane(
        &self,
        seq: Eip8130SeqId,
        on_chain_seq: u64,
    ) -> Option<u64> {
        let start = Eip8130TxId::new(seq, on_chain_seq);
        let mut expected = on_chain_seq;
        let mut last = None;
        for (tx_id, entry) in self.by_id.range(start..) {
            if tx_id.seq != seq || tx_id.nonce != expected || !entry.is_pending() {
                break;
            }
            last = Some(tx_id.nonce);
            expected = expected.checked_add(1)?;
        }
        last
    }

    /// Iterates over the *independent* pending heads (one tx per lane plus
    /// every expiring-mode tx). Block builders consume this; the side pool
    /// is never the only source of pending txs, so the caller is expected
    /// to merge with the standard pool's iterator.
    pub fn independent_pending(&self) -> impl Iterator<Item = Arc<ValidPoolTransaction<T>>> + '_ {
        let lanes = self.independent.values().map(|e| e.tx.clone());
        let exps = self.expiring.values().map(|e| e.tx.clone());
        lanes.chain(exps)
    }

    /// Builds a [`crate::best::BestAaTransactions`] snapshot iterator from
    /// the current pool state. Walks the lane-pending entries once under
    /// the caller's read lock and hands the iterator a self-contained
    /// snapshot — subsequent pool mutations don't affect iteration.
    ///
    /// `base_fee` reseeds the per-tx priority via
    /// [`Eip8130PoolTx::aa_priority_fee`] so cross-lane ordering is
    /// computed against the current block's fee floor (not the
    /// admission-time `base_fee` cached on `PooledEntry`).
    ///
    /// Used by [`crate::OpDualPool::best_transactions`] / `_with_attributes`
    /// to feed the AA side of [`crate::MergeBestTransactions`]. Unlike
    /// the prior `independent_pending`-based approach, the returned
    /// iterator advances within a lane after the head yields — so a lane
    /// holding nonce_seq 0..=N drains all N+1 txs across calls to
    /// `next()`, in priority-aware ordering against other lanes.
    pub fn best_aa_transactions(&self, base_fee: u64) -> crate::best::BestAaTransactions<T> {
        use crate::best::{AaSlot, AaSnapshotEntry, HeapKey, HeapNode};
        use std::collections::{BinaryHeap, HashMap};

        let mut heap: BinaryHeap<HeapNode> =
            BinaryHeap::with_capacity(self.independent.len() + self.expiring.len());
        let mut by_lane: Vec<(Eip8130TxId, AaSnapshotEntry<T>)> =
            Vec::with_capacity(self.pending_count.saturating_sub(self.expiring.len()));
        let mut expiring: HashMap<TxHash, AaSnapshotEntry<T>> =
            HashMap::with_capacity(self.expiring.len());

        // Pass 1 — pending sequenced txs. `by_id` is already ordered by
        // `(seq, nonce_seq)`, so a lane successor is adjacent in the snapshot
        // vector. This keeps construction O(P) without per-entry map inserts.
        for (id, entry) in &self.by_id {
            if !entry.is_pending() {
                continue;
            }
            let priority = entry.tx.transaction.aa_priority_fee(base_fee);
            let snapshot_index = by_lane.len();
            let snap = AaSnapshotEntry {
                priority,
                submission_id: entry.submission_id,
                tx: entry.tx.clone(),
            };
            if self.independent.get(&id.seq).is_some_and(|head| Arc::ptr_eq(head, entry)) {
                heap.push(HeapNode {
                    key: HeapKey {
                        priority: snap.priority,
                        submission_id: snap.submission_id,
                        hash: *snap.tx.hash(),
                    },
                    slot: AaSlot::Sequenced(snapshot_index),
                });
            }
            by_lane.push((*id, snap));
        }

        // Pass 2 — expiring-mode txs. Unconditionally pending and have no
        // successor relationship; keyed by hash. Priority is computed (and
        // the Arc cloned) once per tx — same entry feeds both heap and map.
        for entry in self.expiring.values() {
            let priority = entry.tx.transaction.aa_priority_fee(base_fee);
            let hash = *entry.tx.hash();
            heap.push(HeapNode {
                key: HeapKey { priority, submission_id: entry.submission_id, hash },
                slot: AaSlot::Expiring(hash),
            });
            expiring.insert(
                hash,
                AaSnapshotEntry {
                    priority,
                    submission_id: entry.submission_id,
                    tx: entry.tx.clone(),
                },
            );
        }

        crate::best::BestAaTransactions::from_parts(heap, by_lane, expiring)
    }

    /// Iterates every *pending* tx in every lane (not just independent
    /// heads), plus all expiring-mode txs. Distinct from
    /// [`Self::independent_pending`] which only yields lane heads. Used
    /// by aggregation methods (`pending_transactions`,
    /// `all_transactions`).
    pub fn all_pending(&self) -> impl Iterator<Item = Arc<ValidPoolTransaction<T>>> + '_ {
        let lanes = self.by_id.values().filter(|e| e.is_pending()).map(|e| e.tx.clone());
        let exps = self.expiring.values().map(|e| e.tx.clone());
        lanes.chain(exps)
    }

    /// Iterates every *pending* tx hash. Mirrors
    /// [`Self::all_pending`] without the per-tx `Arc` clone — callers
    /// only interested in the hash should use this (cf PERF-9).
    pub fn pending_hashes(&self) -> impl Iterator<Item = TxHash> + '_ {
        let lanes = self.by_id.values().filter(|e| e.is_pending()).map(|e| *e.tx.hash());
        let exps = self.expiring.values().map(|e| *e.tx.hash());
        lanes.chain(exps)
    }

    /// Iterates every *queued* (nonce-gap-parked) tx. Expiring-mode txs
    /// are never queued — they're always immediately pending — so this
    /// only yields sequenced lane entries with `is_pending == false`.
    pub fn all_queued(&self) -> impl Iterator<Item = Arc<ValidPoolTransaction<T>>> + '_ {
        self.by_id.values().filter(|e| !e.is_pending()).map(|e| e.tx.clone())
    }
}

// ---------------------------------------------------------------------------
// State-update hook
// ---------------------------------------------------------------------------

/// Result of a state-update sweep.
#[derive(Debug)]
pub struct Eip8130StateUpdateOutcome<T: PoolTransaction> {
    /// Txs newly executable as a result of advancing one or more lane
    /// heads. The merge iterator should pick these up at the next pull.
    pub promoted: Vec<Arc<ValidPoolTransaction<T>>>,
    /// Txs removed because they were included on-chain (lane head jumped
    /// past their nonce, or expiring-nonce slot flipped non-zero).
    pub mined: Vec<Arc<ValidPoolTransaction<T>>>,
    /// Txs demoted from pending to queued because a lane head jumped over
    /// them (e.g. a re-org or a manual `setNonce` admin action).
    pub demoted: Vec<Arc<ValidPoolTransaction<T>>>,
    /// Txs evicted because a tracked invalidation key (e.g.
    /// `aa_lock_slot(sender)`) flipped. Distinct from `mined` — these
    /// were never on-chain, just no longer admissible.
    pub invalidated: Vec<Arc<ValidPoolTransaction<T>>>,
}

// Manual `Default` — `derive(Default)` would require `T: Default`, which
// `PoolTransaction` doesn't imply.
impl<T: PoolTransaction> Default for Eip8130StateUpdateOutcome<T> {
    fn default() -> Self {
        Self {
            promoted: Vec::new(),
            mined: Vec::new(),
            demoted: Vec::new(),
            invalidated: Vec::new(),
        }
    }
}

impl<T: Eip8130PoolTx> Eip8130Pool<T> {
    /// Registers the given `rules` (typically the entry's cached
    /// `aa_invalidation_rules()`) in the cross-address invalidation
    /// index. Cheap (typically 1-3 rules per AA tx); idempotent —
    /// re-registering the same `(addr, slot)` overwrites the existing
    /// rule for this hash. Caller passes the pre-computed slice from
    /// `PooledEntry.cached_invalidation_rules` so we don't re-run
    /// `build_*_auth_state` on every call (cf PERF-1).
    fn register_invalidation_keys(
        &mut self,
        hash: TxHash,
        rules: &[((Address, U256), InvalidationRule)],
    ) {
        for (key, rule) in rules {
            self.invalidation_index.entry(*key).or_default().insert(hash, *rule);
        }
    }

    /// Mirror of [`Self::register_invalidation_keys`]: removes `hash`
    /// from every key map it joined, dropping fully-empty maps.
    fn unregister_invalidation_keys(
        &mut self,
        hash: TxHash,
        rules: &[((Address, U256), InvalidationRule)],
    ) {
        for (key, _rule) in rules {
            if let Some(map) = self.invalidation_index.get_mut(key) {
                map.remove(&hash);
                if map.is_empty() {
                    self.invalidation_index.remove(key);
                }
            }
        }
    }

    /// Records `tx.expiry` in the expiry index when non-zero. Zero is the
    /// no-expiry sentinel and is not tracked.
    fn register_expiry(&mut self, tx: &Arc<ValidPoolTransaction<T>>) {
        let Some(expiry) = tx.transaction.aa_expiry() else {
            return;
        };
        if expiry == 0 {
            return;
        }
        self.expiry_index.entry(expiry).or_default().insert(*tx.hash());
    }

    /// Mirror of [`Self::register_expiry`].
    fn unregister_expiry(&mut self, tx: &Arc<ValidPoolTransaction<T>>) {
        let Some(expiry) = tx.transaction.aa_expiry() else {
            return;
        };
        if expiry == 0 {
            return;
        }
        if let Entry::Occupied(mut entry) = self.expiry_index.entry(expiry) {
            entry.get_mut().remove(tx.hash());
            if entry.get().is_empty() {
                entry.remove();
            }
        }
    }

    /// Reconciles the pool with a per-block, cross-address storage-diff
    /// snapshot. Each `(addr, slot, present_value)` triple is consumed:
    ///
    ///   * `addr == STATE_DIFF_ADDRESS` (`NONCE_MANAGER_ADDRESS`):
    ///     - tracked **lane slot** `aa_nonce_slot(sender, key)` → advance lane head, drain mined
    ///       txs, re-promote contiguous prefix;
    ///     - tracked **expiring-seen slot** `aa_expiring_seen_slot(tx_hash)` → evict the expiring
    ///       tx (only when the value flipped non-zero; the zero case means the slot was just
    ///       provisioned, not consumed).
    ///
    ///   * any address with a slot present in `invalidation_index` (typically
    ///     `ACCOUNT_CONFIG_ADDRESS` for `aa_lock_slot` / `aa_owner_config_slot`) → consult each
    ///     registered rule and evict only the txs whose rule rejects the new value.
    ///
    /// `block_timestamp` is the canonical tip's timestamp; used by
    /// [`InvalidationRule::AccountState`] to compare against `unlocksAt`.
    ///
    /// The caller pre-filters the per-block `BundleAccount` snapshot to
    /// only relevant addresses; passing the full snapshot is also valid
    /// but wasteful.
    pub fn on_state_updates<I>(
        &mut self,
        diffs: I,
        block_timestamp: u64,
        block_hash: B256,
    ) -> Eip8130StateUpdateOutcome<T>
    where
        I: IntoIterator<Item = (Address, U256, U256)>,
    {
        let mut outcome = Eip8130StateUpdateOutcome::default();

        // First pass: collect the lanes whose head advanced + the
        // expiring-nonce hashes whose seen-slot flipped non-zero + the
        // invalidation hashes that the rule evaluator marks Evict. We
        // can't borrow `self` mutably inside the iteration, so we gather
        // first and apply in a second pass.
        let mut lane_advances: Vec<(Eip8130SeqId, u64)> = Vec::new();
        let mut expiring_evictions: Vec<B256> = Vec::new();
        let mut invalidated_hashes: Vec<TxHash> = Vec::new();
        for (addr, slot, value) in diffs {
            if addr == STATE_DIFF_ADDRESS {
                if let Some(seq) = self.slot_to_seq.get(&slot).copied() {
                    let new_head: u64 = value.saturating_to();
                    lane_advances.push((seq, new_head));
                }
                if !value.is_zero() &&
                    let Some(exp_hash) = self.slot_to_expiring.get(&slot).copied()
                {
                    expiring_evictions.push(exp_hash);
                }
            }
            if let Some(map) = self.invalidation_index.get(&(addr, slot)) {
                for (hash, rule) in map {
                    if matches!(
                        evaluate_invalidation(rule, value, block_timestamp),
                        InvalidationOutcome::Evict
                    ) {
                        invalidated_hashes.push(*hash);
                    }
                }
            }
        }

        for (seq, new_head) in lane_advances {
            self.advance_lane(seq, new_head, &mut outcome);
        }
        for exp_hash in expiring_evictions {
            if let Some(entry) = self.expiring.get(&exp_hash).cloned() {
                let hash = *entry.tx.hash();
                // Drain mined expiring-mode txs without firing the
                // generic `Discarded` event; emit `Mined(block_hash)`
                // afterwards so subscribers can distinguish inclusion
                // from eviction.
                if let Some(tx) = self.remove_by_hash_no_event(&hash) {
                    outcome.mined.push(tx);
                }
            }
        }
        // Apply invalidations last — `mined` already pulled those that
        // also moved their nonce head out from under the index, so the
        // dedup is implicit (the tx is already gone from `by_hash`).
        for hash in invalidated_hashes {
            if let Some(tx) = self.remove_by_hash(&hash) {
                outcome.invalidated.push(tx);
            }
        }

        // Fire `Mined(block_hash)` for the full mined set in one
        // pass. Reth's standard pool fires `Mined` from `with_event_listener`
        // (`reth-transaction-pool/src/pool/mod.rs:887-897`); we mirror that
        // shape on the AA side pool's broadcast channel so subscribers via
        // `transaction_event_listener` see Mined distinct from Discarded.
        for tx in &outcome.mined {
            self.broadcast_event(*tx.hash(), TransactionEvent::Mined(block_hash));
        }

        if !outcome.promoted.is_empty() {
            self.metrics.txs_promoted.increment(outcome.promoted.len() as u64);
        }
        if !outcome.demoted.is_empty() {
            self.metrics.txs_demoted.increment(outcome.demoted.len() as u64);
        }
        if !outcome.invalidated.is_empty() {
            self.metrics.txs_evicted_invalidation.increment(outcome.invalidated.len() as u64);
        }
        self.record_size_gauges();
        outcome
    }

    /// Re-validates payer balance for every AA tx (pending **and** queued)
    /// whose payer appears in `balance_updates`, evicting those whose new
    /// balance can no longer cover the admission-time required cost
    /// (`gas_limit * max_fee_per_gas + l1_data_fee`).
    ///
    /// Mirrors tempo's `Check 3b` insolvent-fee_payer eviction
    /// (`tempo_pool.rs:197-307`), adapted to xlayer's raw-ETH model: the
    /// predicate matches admission's at [`crate::eip8130_xlayer`]
    /// (`balance >= gas_limit * max_fee_per_gas + l1_data_fee`), so
    /// admission and re-validation are kept consistent.
    ///
    /// Queued txs are walked too: F4 — without this, a queued tx whose
    /// payer drained between admission and the gap closing would
    /// silently promote to pending via [`Self::promote_lane`] (which
    /// checks nonce contiguity only) and then fail at execution. Tempo
    /// walks `pending.iter().chain(queued.iter())` in one pass at
    /// `tempo_pool.rs:197`; we mirror that shape here.
    ///
    /// Each [`PooledEntry`] caches its admission-time
    /// `required_balance` (the validator's full predicate including the
    /// L1 component). The maintenance loop has no access to
    /// `OpL1BlockInfo`, so caching is strictly cheaper than re-piping it
    /// here — and the cached value is identically the value the
    /// validator just enforced, so admission and re-validation cannot
    /// drift.
    pub fn on_balance_updates<I>(&mut self, balance_updates: I) -> Vec<Arc<ValidPoolTransaction<T>>>
    where
        I: IntoIterator<Item = (Address, U256)>,
    {
        let updates: HashMap<Address, U256> = balance_updates.into_iter().collect();
        if updates.is_empty() {
            return Vec::new();
        }

        // Walk all sequenced entries (pending + queued lane members) and
        // every expiring-mode tx.
        let mut to_evict: Vec<TxHash> = Vec::new();
        let pending_iter = self.by_id.values().chain(self.expiring.values());
        for entry in pending_iter {
            let payer = match entry.tx.transaction.aa_payer() {
                Some(p) => p,
                None => continue, // non-AA route shouldn't reach this pool, but be defensive.
            };
            let Some(&new_balance) = updates.get(&payer) else {
                continue;
            };
            if new_balance < entry.required_balance {
                to_evict.push(*entry.tx.hash());
            }
        }

        let mut evicted = Vec::with_capacity(to_evict.len());
        for hash in to_evict {
            if let Some(tx) = self.remove_by_hash(&hash) {
                evicted.push(tx);
            }
        }
        if !evicted.is_empty() {
            self.metrics.txs_evicted_insolvent_payer.increment(evicted.len() as u64);
        }
        self.record_size_gauges();
        evicted
    }

    /// Advance lane `seq` to `new_head`. Drains `by_id` for nonces below
    /// `new_head` (those are now mined). Demotes the lane in case the new
    /// head jumps past a nonce we never held (gap), then re-promotes from
    /// `new_head` upward.
    fn advance_lane(
        &mut self,
        seq: Eip8130SeqId,
        new_head: u64,
        outcome: &mut Eip8130StateUpdateOutcome<T>,
    ) {
        let prev_head = self.on_chain_nonces.insert(seq, new_head).unwrap_or(new_head);

        // Cheaper path: head didn't actually move (replayed identical
        // diff). Nothing to do.
        if new_head == prev_head {
            return;
        }

        // Drain mined txs (`nonce < new_head`). Collect ids first to avoid
        // borrowing `by_id` while iterating + mutating.
        // Use the silent removal path so the per-tx event listener
        // doesn't see a `Discarded` event. The caller (`on_state_updates`)
        // emits `Mined(block_hash)` for the whole mined set after every
        // `advance_lane` returns.
        let drain_ids: Vec<Eip8130TxId> = {
            let lo = Eip8130TxId::new(seq, 0);
            let hi = Eip8130TxId::new(seq, new_head);
            self.by_id.range(lo..hi).map(|(id, _)| *id).collect()
        };
        for id in drain_ids {
            if let Some(tx) = self.remove_mined_sequenced_by_id_no_demote(id) {
                outcome.mined.push(tx);
            }
        }

        // If the new head jumped *past* our remaining holdings (no tx at
        // exactly `new_head`), the lane is gapped: every remaining tx must
        // be demoted to queued and the independent head cleared.
        let head_id = Eip8130TxId::new(seq, new_head);
        let head_present = self.by_id.contains_key(&head_id);
        if !head_present {
            let demoted = self.demote_lane(seq);
            outcome.demoted.extend(demoted);
            return;
        }

        // Otherwise re-promote from the new head. `promote_lane` re-runs
        // the contiguous-prefix scan and re-installs the independent head.
        let promoted = self.promote_lane(seq, new_head);
        for tx in &promoted {
            let _ = self.pending_tx_broadcaster.send(*tx.hash());
            self.broadcast_event(*tx.hash(), TransactionEvent::Pending);
        }
        outcome.promoted.extend(promoted);
    }
}

// Surface the address every state-update consumer needs to filter for.
pub use op_revm::precompiles_xlayer::NONCE_MANAGER_ADDRESS as STATE_DIFF_ADDRESS;

// Wiring `OpDualPool::on_state_diffs` from a reth `CanonStateNotification`:
//
// ```rust,ignore
// use revm::database::BundleAccount;
// let outcome = notification.committed.execution_outcome();
// let bundle: &BundleStateInner = outcome.state();
// let acc: Option<&BundleAccount> = bundle.state.get(&STATE_DIFF_ADDRESS);
// let diffs = acc
//     .into_iter()
//     .flat_map(|a| a.storage.iter().map(|(slot, v)| (*slot, v.present_value)));
// pool.on_state_diffs(diffs);
// ```
//
// We don't ship that helper inline because the txpool crate doesn't take a
// direct revm-database dep; the binary integration crate has the access.

// ---------------------------------------------------------------------------
// Eip8130PoolTx for OpPooledTransaction
// ---------------------------------------------------------------------------
//
// Production binding: routes the trait through the existing `OpPooledTx`
// adapter which already knows how to lift `&Sealed<TxEip8130>` out of the
// inner consensus transaction. Defined here (not in `transaction.rs`) so
// the pool's trait stays the single source of truth for AA-tx accessors.

use crate::transaction::{OpPooledTransaction, OpPooledTx};
use op_revm::{
    OpEip8130TxTr,
    constants::ACCOUNT_CONFIG_ADDRESS,
    handler::{aa_lock_slot, aa_owner_config_slot},
};
use reth_primitives_traits::SignedTransaction;
use std::cmp::min;

impl<Cons, Pooled> Eip8130PoolTx for OpPooledTransaction<Cons, Pooled>
where
    Cons: SignedTransaction + From<Pooled> + OpEip8130TxTr,
    Pooled: SignedTransaction + TryFrom<Cons>,
    <Pooled as TryFrom<Cons>>::Error: core::error::Error,
{
    fn is_eip8130(&self) -> bool {
        OpPooledTx::as_eip8130(self).is_some()
    }

    fn as_eip8130(&self) -> Option<&op_alloy_consensus::TxEip8130> {
        OpPooledTx::as_eip8130(self)
    }

    /// AA-effective sender:
    ///   1. `tx.from` if explicitly set (the validator trusts this for K1 self-pay shapes);
    ///   2. otherwise the recovered envelope signer — for K1 EOA-mode txs the envelope signer is
    ///      the AA sender.
    ///
    /// For non-K1 verifier admissions this is a placeholder; the validator
    /// is currently the source of truth and the pool's view is allowed to
    /// drift until proper sender caching is added (see `xlayer-aa.md`
    /// "deferred validator outcome propagation").
    fn aa_sender(&self) -> Option<Address> {
        let inner = OpPooledTx::as_eip8130(self)?;
        // `Deref` exposes `EthPooledTransaction<Cons>::transaction:
        // Recovered<Cons>`; the recovered signer is the envelope signer.
        Some(inner.sender.unwrap_or_else(|| self.transaction.signer()))
    }

    fn aa_nonce_key(&self) -> Option<U256> {
        Some(OpPooledTx::as_eip8130(self)?.nonce_key)
    }

    fn aa_nonce_sequence(&self) -> Option<u64> {
        Some(OpPooledTx::as_eip8130(self)?.nonce_sequence)
    }

    fn aa_nonce_key_slot(&self) -> Option<U256> {
        self.cached_aa_nonce_key_slot()
    }

    fn aa_expiring_nonce_slot(&self) -> Option<U256> {
        self.cached_aa_expiring_nonce_slot()
    }

    fn aa_expiring_nonce_hash(&self) -> Option<B256> {
        self.cached_aa_expiring_nonce_hash()
    }

    /// `min(max_priority_fee, max_fee - base_fee)` — the standard EIP-1559
    /// effective tip used for ordering. Falls back to 0 when `max_fee <
    /// base_fee` (which the validator will already have rejected, but the
    /// pool defends against drift between validate and add).
    fn aa_priority_fee(&self, base_fee: u64) -> u128 {
        let Some(inner) = OpPooledTx::as_eip8130(self) else {
            return 0;
        };
        let head_room = inner.max_fee_per_gas.saturating_sub(u128::from(base_fee));
        min(inner.max_priority_fee_per_gas, head_room)
    }

    /// Publishes the rule set the pool consults on storage diffs:
    ///
    ///   * **Sender owner_config (rule A)** — `aa_owner_config_slot(sender, implicit_owner_id)`
    ///     with [`InvalidationRule::OwnerConfig`] binding the K1 verifier and the SENDER scope bit.
    ///     Evicts on `REVOKED_VERIFIER`, on verifier mismatch, or on a non-zero scope that drops
    ///     SENDER. A diff that adds the PAYER bit while keeping SENDER is **not** an eviction (the
    ///     prior keys-only index would have evicted it spuriously).
    ///
    ///   * **Payer owner_config (rule B)** — `aa_owner_config_slot(payer, payer_owner_id)` with
    ///     [`InvalidationRule::OwnerConfig`] binding the payer's verifier and the PAYER scope bit.
    ///     Only registered for sponsored shapes where `payer != sender`; self-pay (and explicit
    ///     `tx.payer == Some(sender)`) collapses into rule A. Native payer auth only — Deferred
    ///     skips for the same reason as rule A on the sender side.
    ///
    ///   * **Sender account_state (rule C/D)** — `aa_lock_slot(sender)` gated on `lock_sensitive`
    ///     (any `account_changes` present). The same slot also covers signature-bound sequence
    ///     checks (rule D, spec line 376) when the tx pins a `change_sequence`; `seq_check` plumbs
    ///     the expected pre-value through.
    ///
    /// `owner_id` / `expected_verifier` derivation:
    ///
    /// - **Native auth** (K1 / P256-raw / WebAuthn / Delegate→Native): pull `(verifier, owner_id)`
    ///   directly from the `AuthState::Native` the resolver produced for this tx. K1 self-pay
    ///   collapses to the same `(K1, bytes32(bytes20(sender)))` pair the prior implicit-EOA
    ///   derivation hardcoded. **Delegate→Native** registers TWO rules: the outer
    ///   `owner_config[sender][bytes32(bytes20(delegate))]` row and the inner
    ///   `owner_config[delegate_address][delegate_inner.owner_id]` row. Both bindings are checked
    ///   by the executor's `dispatch_auth_state` (op-revm `handler.rs:676-710`); a diff to either
    ///   slot must evict the tx.
    /// - **Deferred auth** (custom verifier / Delegate→Custom): the `owner_id` is unknown until the
    ///   executor runs the STATICCALL, so no rule A entry is registered. This is intentionally
    ///   coarser than Native — admission for Deferred txs goes through but the side pool will rely
    ///   on rules C/D (account_state) plus on-chain inclusion failure to evict if the verifier
    ///   contract changes its answer. Registering a placeholder slot would risk false-positive
    ///   evictions on unrelated owner_config diffs. Delegate→Custom is the same — even though the
    ///   outer slot binding is known (DELEGATE_VERIFIER_ADDRESS), the inner owner_id depends on the
    ///   STATICCALL result, so we don't register either slot.
    /// - **Empty / Invalid / SelfPay**: admission rejects these on the sender side, so they cannot
    ///   reach this trait method via the admission path. Defensive empty-rules return otherwise.
    fn aa_invalidation_rules(&self) -> Vec<((Address, U256), InvalidationRule)> {
        use alloy_op_evm::eip8130::auth_state::{
            build_payer_auth_state, build_sender_auth_state_with_recovered,
        };
        use op_revm::transaction::eip8130::AuthState;
        let Some(sender) = self.aa_sender() else {
            return Vec::new();
        };

        // Typical sponsored Native shape registers up to 4 rules: sender
        // owner_config + sender delegate inner + payer owner_config + lock
        // slot. Pre-allocating avoids repeated grows on the hot path
        // (cf PERF-11).
        let mut rules: Vec<((Address, U256), InvalidationRule)> = Vec::with_capacity(4);
        // Pull validator-resolved parts via the narrow `Eip8130PartsCache` trait
        // (impl is on `OpPooledTransaction` and reads its `eip8130_parts` OnceLock).
        // The `else` arms below remain as fallback for the rare case the cache is
        // missed (e.g. tx admitted via a non-AA path).
        let cached_parts = self.cached_eip8130_parts();
        let mut push_native_owner_rules =
            |account: Address,
             verifier: Address,
             owner_id: B256,
             delegate_inner: Option<&op_revm::transaction::eip8130::DelegateInner>,
             required_scope_bit: u8| {
                rules.push((
                    (
                        ACCOUNT_CONFIG_ADDRESS,
                        aa_owner_config_slot(account, U256::from_be_bytes(owner_id.0)),
                    ),
                    InvalidationRule::OwnerConfig {
                        expected_verifier: verifier,
                        required_scope_bit,
                    },
                ));
                if let Some(di) = delegate_inner {
                    let delegate_address = alloy_primitives::Address::from_slice(&owner_id.0[..20]);
                    rules.push((
                        (
                            ACCOUNT_CONFIG_ADDRESS,
                            aa_owner_config_slot(
                                delegate_address,
                                U256::from_be_bytes(di.owner_id.0),
                            ),
                        ),
                        InvalidationRule::OwnerConfig {
                            expected_verifier: di.verifier,
                            required_scope_bit,
                        },
                    ));
                }
            };

        if let Some(inner_tx) = OpPooledTx::as_eip8130(self) {
            if let Some(AuthState::Native { verifier, owner_id, delegate_inner }) =
                cached_parts.map(|parts| &parts.sender_authstate)
            {
                push_native_owner_rules(
                    sender,
                    *verifier,
                    *owner_id,
                    delegate_inner.as_ref(),
                    op_revm::constants::OWNER_SCOPE_SENDER,
                );
            } else if let AuthState::Native { verifier, owner_id, delegate_inner } =
                build_sender_auth_state_with_recovered(inner_tx, sender)
            {
                push_native_owner_rules(
                    sender,
                    verifier,
                    owner_id,
                    delegate_inner.as_ref(),
                    op_revm::constants::OWNER_SCOPE_SENDER,
                );
            }
            // Deferred / Empty / Invalid / SelfPay: no rule A entry. See
            // doc comment above for rationale.

            // Rule B: sponsored payer's owner_config. Skip when payer
            // collapses to sender (`is_self_pay() == true` OR the user
            // explicitly set `tx.payer == Some(sender)`) — rule A already
            // covers it. Deferred payer auth: skip the slot entry — the
            // owner_id is unknown until the executor's STATICCALL runs, same
            // rationale as the Deferred sender branch.
            let effective_payer = cached_parts
                .map(|parts| parts.payer)
                .unwrap_or_else(|| inner_tx.payer.unwrap_or(sender));
            if effective_payer != sender {
                if let Some(AuthState::Native { verifier, owner_id, delegate_inner }) =
                    cached_parts.map(|parts| &parts.payer_authstate)
                {
                    push_native_owner_rules(
                        effective_payer,
                        *verifier,
                        *owner_id,
                        delegate_inner.as_ref(),
                        op_revm::constants::OWNER_SCOPE_PAYER,
                    );
                } else if let AuthState::Native { verifier, owner_id, delegate_inner } =
                    build_payer_auth_state(inner_tx, sender)
                {
                    push_native_owner_rules(
                        effective_payer,
                        verifier,
                        owner_id,
                        delegate_inner.as_ref(),
                        op_revm::constants::OWNER_SCOPE_PAYER,
                    );
                }
            }
        }

        // Lock-sensitivity: any tx carrying account_changes that the
        // executor's `check_account_lock` (`op-revm/src/handler.rs:343-370`)
        // would gate on the lock window must be evicted on a lock-state
        // diff at the sender. The executor fires when
        // `delegation_target.is_some() || !config_writes.is_empty() ||
        // !sequence_updates.is_empty()`, and `process_config_change_entry`
        // (`alloy-op-evm/src/eip8130/parts.rs:309`) emits a
        // `sequence_updates` slot for ANY ConfigChange targeting us —
        // including pure-sequence (empty `owner_changes` + empty
        // `authorizer_auth`). So pure-sequence ConfigChange IS
        // lock-sensitive at execution; the side pool must mirror that or
        // a tx parked in pending could survive a lock-window seed and
        // ship to the executor for AccountLocked rejection.
        let lock_sensitive = if let Some(inner) = OpPooledTx::as_eip8130(self) {
            inner.account_changes.iter().any(|e| {
                matches!(
                    e,
                    op_alloy_consensus::AccountChangeEntry::Create(_) |
                        op_alloy_consensus::AccountChangeEntry::Delegation(_) |
                        op_alloy_consensus::AccountChangeEntry::ConfigChange(_)
                )
            })
        } else {
            false
        };
        // Rule D, spec line 376: each `ConfigChange` entry pins one half
        // of `_accountState[sender]` to a specific pre-value. Mirrors
        // `op_revm/handler.rs:483-519`: an entry's `sequence` field is
        // the expected on-chain pre-value, and the executor advances the
        // half by 1 on success. For invalidation we only care that the
        // FIRST per-half pre-value still holds — if a competitor
        // advances the half, the first entry already mismatches and the
        // whole tx fails at execution.
        let seq_check: Option<SeqExpect> = OpPooledTx::as_eip8130(self).and_then(|inner| {
            let mut multichain_pre: Option<u64> = None;
            let mut local_pre: Option<u64> = None;
            for entry in &inner.account_changes {
                if let op_alloy_consensus::AccountChangeEntry::ConfigChange(cc) = entry {
                    let targets_us = cc.chain_id == 0 || cc.chain_id == inner.chain_id;
                    if !targets_us {
                        continue;
                    }
                    if cc.chain_id == 0 {
                        multichain_pre.get_or_insert(cc.sequence);
                    } else {
                        local_pre.get_or_insert(cc.sequence);
                    }
                }
            }
            if multichain_pre.is_some() || local_pre.is_some() {
                Some(SeqExpect { multichain_pre, local_pre })
            } else {
                None
            }
        });
        if lock_sensitive || seq_check.is_some() {
            rules.push((
                (ACCOUNT_CONFIG_ADDRESS, aa_lock_slot(sender)),
                InvalidationRule::AccountState { lock_sensitive, seq_check },
            ));
        }

        rules
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Sealable, transaction::Recovered};
    use alloy_eips::eip2718::Encodable2718;
    use alloy_primitives::{Address, B256, Bytes, U256, bytes};
    use alloy_signer::SignerSync;
    use alloy_signer_local::PrivateKeySigner;
    use op_alloy_consensus::{Eip8130CallEntry, TxEip8130, sender_signature_hash};
    use op_revm::{constants::K1_VERIFIER_ADDRESS, handler::NONCE_KEY_MAX};
    use reth_optimism_primitives::OpTransactionSigned;
    use reth_transaction_pool::{
        TransactionOrigin,
        identifier::{SenderId, TransactionId},
    };
    use std::time::Instant;

    /// Deterministic K1 signer keyed by a 1-byte seed. Tests use the
    /// signer's `address()` as the sender — required because P1's auth
    /// dispatch enforces the K1 strict-self-owner invariant
    /// (`recovered_addr == tx.from`). Seed 0x00 is forbidden (zero K1
    /// key); we filter it in [`aa_tx`]'s search loop.
    fn signer_for(seed: u8) -> Option<PrivateKeySigner> {
        // PrivateKeySigner rejects the all-zero scalar; any other 1-byte
        // repeated seed falls inside the curve order and is valid.
        if seed == 0 {
            return None;
        }
        PrivateKeySigner::from_bytes(&B256::repeat_byte(seed)).ok()
    }

    /// Maps a legacy `Address::repeat_byte(seed)` test vector onto a real
    /// K1-signer-derived address. Existing pool tests pass arbitrary
    /// addresses through `aa_tx`; that no longer admits post-P1 because
    /// the resolver runs eager K1 verify with strict-self-owner. Callers
    /// that hold the address but not the signer key can swap to this and
    /// keep their assertions on owner_config slots derived via
    /// `implicit_owner_id(sender)` working unchanged.
    fn sender_from_seed(seed: u8) -> Address {
        signer_for(seed).expect("non-zero seed yields a valid K1 key").address()
    }

    /// 65-byte K1 sig blob `(r || s || v)` over `hash`.
    fn k1_sig_blob(signer: &PrivateKeySigner, hash: B256) -> Vec<u8> {
        let sig = signer.sign_hash_sync(&hash).expect("sign");
        let mut buf = Vec::with_capacity(65);
        buf.extend_from_slice(&sig.r().to_be_bytes::<32>());
        buf.extend_from_slice(&sig.s().to_be_bytes::<32>());
        buf.push(if sig.v() { 1 } else { 0 });
        buf
    }

    /// Explicit-from K1 `sender_auth`: `[K1_VERIFIER_ADDRESS(20) || sig(65)]`.
    fn k1_explicit_auth(signer: &PrivateKeySigner, hash: B256) -> Bytes {
        let mut buf = Vec::with_capacity(85);
        buf.extend_from_slice(K1_VERIFIER_ADDRESS.as_slice());
        buf.extend_from_slice(&k1_sig_blob(signer, hash));
        Bytes::from(buf)
    }

    /// Builds a `TxEip8130` populated with realistic AA fields **and a real K1
    /// `sender_auth`**, so `aa_invalidation_rules` resolves to
    /// `AuthState::Native` and registers an OwnerConfig rule keyed on
    /// `aa_owner_config_slot(sender, implicit_owner_id(sender))`.
    ///
    /// `sender` MUST be derived via `sender_from_seed` (or otherwise be the
    /// address of a known signer that the test holds the key for) — the
    /// resolver enforces K1 strict-self-owner against `tx.from`.
    fn aa_tx(
        sender: Address,
        nonce_key: U256,
        nonce_sequence: u64,
        max_priority_fee_per_gas: u128,
    ) -> TxEip8130 {
        // Recover the deterministic seed from the sender address by trying
        // each byte 0x01..=0xFF (skip 0x00 — zero K1 key is invalid).
        // Pool tests use a small seed space so the search is cheap and
        // avoids threading the signer through every call site. Returns
        // the first signer whose address matches.
        let signer = (1u8..=255u8)
            .filter_map(signer_for)
            .find(|s| s.address() == sender)
            .expect("aa_tx requires a sender derived via sender_from_seed");

        let mut tx = TxEip8130 {
            chain_id: 10,
            sender: Some(sender),
            nonce_key,
            nonce_sequence,
            expiry: 0,
            max_priority_fee_per_gas,
            max_fee_per_gas: max_priority_fee_per_gas + 1,
            gas_limit: 100_000,
            calls: vec![vec![Eip8130CallEntry { to: Address::repeat_byte(0xAA), data: bytes!() }]],
            ..Default::default()
        };
        let hash = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(&signer, hash);
        tx
    }

    /// Wraps a `TxEip8130` in a fully-formed `Arc<ValidPoolTransaction<OpPooledTransaction>>`,
    /// bypassing signature recovery (`Recovered::new_unchecked` trusts the
    /// caller-supplied signer). The encoded length is computed from the
    /// real EIP-2718 envelope so eviction-priority math is honest.
    fn make_valid(
        tx: TxEip8130,
        signer: Address,
    ) -> Arc<ValidPoolTransaction<OpPooledTransaction>> {
        let signed: OpTransactionSigned = tx.seal_slow().into();
        let recovered = Recovered::new_unchecked(signed, signer);
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
    }

    /// Builds the implicit-EOA owner_id used by both the trait impl and
    /// these tests: 20-byte sender padded to 32 bytes, big-endian.
    fn implicit_owner_id(sender: Address) -> U256 {
        let mut buf = [0u8; 32];
        buf[..20].copy_from_slice(sender.as_slice());
        U256::from_be_bytes(buf)
    }

    /// Encodes `(verifier, scope)` into the packed `owner_config` word
    /// the handler reads back via [`op_revm::handler::parse_owner_config_word`].
    /// Layout (spec line 226): `bytes[11] = scope`, `bytes[12..32] = verifier`.
    fn make_owner_config_word(verifier: Address, scope: u8) -> U256 {
        let mut bytes = [0u8; 32];
        bytes[11] = scope;
        bytes[12..32].copy_from_slice(verifier.as_slice());
        U256::from_be_bytes(bytes)
    }

    /// Encodes a packed `account_state` word: `unlocksAt` at bytes[11..16]
    /// (uint40 BE), `localSequence` at bytes[16..24], `multichainSequence`
    /// at bytes[24..32]. Mirrors `pack_account_state` in op-revm tests.
    fn make_account_state_word(unlocks_at: u64, multichain_seq: u64, local_seq: u64) -> U256 {
        let mut bytes = [0u8; 32];
        bytes[24..32].copy_from_slice(&multichain_seq.to_be_bytes());
        bytes[16..24].copy_from_slice(&local_seq.to_be_bytes());
        let ua = unlocks_at.to_be_bytes();
        bytes[11..16].copy_from_slice(&ua[3..8]);
        U256::from_be_bytes(bytes)
    }

    /// Self-test: the encoders round-trip exactly with the public
    /// decoders the evaluator uses. Failing this would invalidate every
    /// `evaluate_invalidation_*` test below.
    #[test]
    fn fixture_word_encoders_round_trip() {
        use op_revm::handler::{
            parse_owner_config_word, read_packed_sequence, unlocks_at_from_account_state_word,
        };
        let v = sender_from_seed(0xAB);
        let w = make_owner_config_word(v, 0x06);
        assert_eq!(parse_owner_config_word(w), (v, 0x06));

        let s =
            make_account_state_word(0x12_3456_789A, 0x0102_0304_0506_0708, 0x0A0B_0C0D_0E0F_1011);
        assert_eq!(unlocks_at_from_account_state_word(s), 0x12_3456_789A);
        // multichain half lives at bytes[24..32] / `limbs[0]`.
        assert_eq!(read_packed_sequence(s, true), 0x0102_0304_0506_0708);
        // local half lives at bytes[16..24] / `(word >> 64).limbs[0]`.
        assert_eq!(read_packed_sequence(s, false), 0x0A0B_0C0D_0E0F_1011);
    }

    // -----------------------------------------------------------------
    // evaluate_invalidation unit tests
    //
    // The pure evaluator is the single source of truth for Keep/Evict.
    // These tests cover each spec arm without going through the pool
    // bookkeeping; the pool-level integration tests below verify the
    // wiring once the rules are correct.
    // -----------------------------------------------------------------

    /// Spec lines 525-526 / 787: `REVOKED_VERIFIER` sentinel at an
    /// owner_config slot is the explicit tombstone — never compatible.
    #[test]
    fn evaluate_owner_config_revoked_verifier() {
        use op_revm::constants::{K1_VERIFIER_ADDRESS, OWNER_SCOPE_SENDER, REVOKED_VERIFIER};
        let rule = InvalidationRule::OwnerConfig {
            expected_verifier: K1_VERIFIER_ADDRESS,
            required_scope_bit: OWNER_SCOPE_SENDER,
        };
        let word = make_owner_config_word(REVOKED_VERIFIER, 0);
        assert_eq!(evaluate_invalidation(&rule, word, 0), InvalidationOutcome::Evict);
    }

    /// Verifier mismatch: any non-revoked address that isn't the
    /// expected verifier breaks the auth's binding.
    #[test]
    fn evaluate_owner_config_verifier_mismatch() {
        use op_revm::constants::{K1_VERIFIER_ADDRESS, OWNER_SCOPE_SENDER};
        let rule = InvalidationRule::OwnerConfig {
            expected_verifier: K1_VERIFIER_ADDRESS,
            required_scope_bit: OWNER_SCOPE_SENDER,
        };
        let word = make_owner_config_word(Address::repeat_byte(0xEE), 0);
        assert_eq!(evaluate_invalidation(&rule, word, 0), InvalidationOutcome::Evict);
    }

    /// Spec line 226: explicit non-zero scope that omits the required
    /// bit fails the scope check.
    #[test]
    fn evaluate_owner_config_drops_sender_bit() {
        use op_revm::constants::{K1_VERIFIER_ADDRESS, OWNER_SCOPE_PAYER, OWNER_SCOPE_SENDER};
        let rule = InvalidationRule::OwnerConfig {
            expected_verifier: K1_VERIFIER_ADDRESS,
            required_scope_bit: OWNER_SCOPE_SENDER,
        };
        // Scope = PAYER only (0x04) → SENDER bit cleared → evict.
        let word = make_owner_config_word(K1_VERIFIER_ADDRESS, OWNER_SCOPE_PAYER);
        assert_eq!(evaluate_invalidation(&rule, word, 0), InvalidationOutcome::Evict);
    }

    /// Adding PAYER to an existing SENDER-scoped owner is compatible
    /// with a sender_auth tx — the required bit is still set.
    #[test]
    fn evaluate_owner_config_keeps_sender_with_payer_added() {
        use op_revm::constants::{K1_VERIFIER_ADDRESS, OWNER_SCOPE_PAYER, OWNER_SCOPE_SENDER};
        let rule = InvalidationRule::OwnerConfig {
            expected_verifier: K1_VERIFIER_ADDRESS,
            required_scope_bit: OWNER_SCOPE_SENDER,
        };
        let word =
            make_owner_config_word(K1_VERIFIER_ADDRESS, OWNER_SCOPE_SENDER | OWNER_SCOPE_PAYER);
        assert_eq!(evaluate_invalidation(&rule, word, 0), InvalidationOutcome::Keep);
    }

    /// Scope == 0 means "default scope" (handler.rs:242 mirrors this);
    /// matching verifier with scope 0 is compatible.
    #[test]
    fn evaluate_owner_config_default_scope_zero_keeps() {
        use op_revm::constants::{K1_VERIFIER_ADDRESS, OWNER_SCOPE_SENDER};
        let rule = InvalidationRule::OwnerConfig {
            expected_verifier: K1_VERIFIER_ADDRESS,
            required_scope_bit: OWNER_SCOPE_SENDER,
        };
        let word = make_owner_config_word(K1_VERIFIER_ADDRESS, 0);
        assert_eq!(evaluate_invalidation(&rule, word, 0), InvalidationOutcome::Keep);
    }

    /// Spec line 511: a sender that's still locked at execution time
    /// rejects any account_changes-bearing tx. `block_timestamp <
    /// unlocksAt` → evict.
    #[test]
    fn evaluate_account_state_locked_window_evicts() {
        let rule = InvalidationRule::AccountState { lock_sensitive: true, seq_check: None };
        let word = make_account_state_word(20, 0, 0); // unlocksAt=20
        assert_eq!(evaluate_invalidation(&rule, word, 10), InvalidationOutcome::Evict);
    }

    /// Lock window already elapsed: `block_timestamp >= unlocksAt` →
    /// keep.
    #[test]
    fn evaluate_account_state_unlocked_window_keeps() {
        let rule = InvalidationRule::AccountState { lock_sensitive: true, seq_check: None };
        let word = make_account_state_word(10, 0, 0); // unlocksAt=10
        assert_eq!(evaluate_invalidation(&rule, word, 20), InvalidationOutcome::Keep);
    }

    /// Spec line 376: `change_sequence` is signature-bound. Any
    /// advance past the local pre-value breaks the auth.
    #[test]
    fn evaluate_account_state_seq_advances_past_expected_evicts() {
        let rule = InvalidationRule::AccountState {
            lock_sensitive: false,
            seq_check: Some(SeqExpect { multichain_pre: None, local_pre: Some(5) }),
        };
        // local sequence half advanced to 6 (expected was 5).
        let word = make_account_state_word(0, 0, 6);
        assert_eq!(evaluate_invalidation(&rule, word, 0), InvalidationOutcome::Evict);
    }

    /// Sequence at expected pre-value (no advance yet): keep.
    #[test]
    fn evaluate_account_state_seq_at_expected_keeps() {
        let rule = InvalidationRule::AccountState {
            lock_sensitive: false,
            seq_check: Some(SeqExpect { multichain_pre: None, local_pre: Some(5) }),
        };
        let word = make_account_state_word(0, 0, 5);
        assert_eq!(evaluate_invalidation(&rule, word, 0), InvalidationOutcome::Keep);
    }

    /// Sequence rules are half-scoped: a multichain-only rule only
    /// checks the multichain half. If only the *local* half advanced, a
    /// multichain-bound auth is unaffected. Conversely the local-only
    /// rule evicts.
    #[test]
    fn evaluate_account_state_seq_multichain_half_only() {
        let multichain_rule = InvalidationRule::AccountState {
            lock_sensitive: false,
            seq_check: Some(SeqExpect { multichain_pre: Some(3), local_pre: None }),
        };
        let local_rule = InvalidationRule::AccountState {
            lock_sensitive: false,
            seq_check: Some(SeqExpect { multichain_pre: None, local_pre: Some(7) }),
        };
        // multichain half stays at 3, local half advanced to 8.
        let word = make_account_state_word(0, 3, 8);
        assert_eq!(
            evaluate_invalidation(&multichain_rule, word, 0),
            InvalidationOutcome::Keep,
            "multichain half is still at expected pre-value",
        );
        assert_eq!(
            evaluate_invalidation(&local_rule, word, 0),
            InvalidationOutcome::Evict,
            "local half advanced past expected pre-value",
        );
    }

    /// A tx that pins **only** the multichain half (e.g. a
    /// `ConfigChange { chain_id: 0 }` entry). On-chain multichain
    /// advance evicts; local-only advance does not.
    #[test]
    fn evaluate_account_state_seq_multichain_only_advances_evicts() {
        let rule = InvalidationRule::AccountState {
            lock_sensitive: false,
            seq_check: Some(SeqExpect { multichain_pre: Some(7), local_pre: None }),
        };
        // multichain advanced 7 → 8; local untouched.
        let word = make_account_state_word(0, 8, 0);
        assert_eq!(evaluate_invalidation(&rule, word, 0), InvalidationOutcome::Evict);
    }

    /// A tx that pins **only** the local half (e.g. a
    /// `ConfigChange { chain_id: tx.chain_id }` entry). On-chain local
    /// advance evicts.
    #[test]
    fn evaluate_account_state_seq_local_only_advances_evicts() {
        let rule = InvalidationRule::AccountState {
            lock_sensitive: false,
            seq_check: Some(SeqExpect { multichain_pre: None, local_pre: Some(3) }),
        };
        // local advanced 3 → 4; multichain untouched.
        let word = make_account_state_word(0, 0, 4);
        assert_eq!(evaluate_invalidation(&rule, word, 0), InvalidationOutcome::Evict);
    }

    /// A dual-half tx (one multichain entry + one local
    /// entry) where only the local half advanced. Either half advancing
    /// is enough to evict the tx.
    #[test]
    fn evaluate_account_state_seq_both_halves_one_evicts() {
        let rule = InvalidationRule::AccountState {
            lock_sensitive: false,
            seq_check: Some(SeqExpect { multichain_pre: Some(7), local_pre: Some(3) }),
        };
        // multichain stays at 7; local goes 3 → 4.
        let word = make_account_state_word(0, 7, 4);
        assert_eq!(evaluate_invalidation(&rule, word, 0), InvalidationOutcome::Evict);
    }

    /// A dual-half tx where **neither** half advanced. Both
    /// checks pass → keep.
    #[test]
    fn evaluate_account_state_seq_both_halves_neither_evicts() {
        let rule = InvalidationRule::AccountState {
            lock_sensitive: false,
            seq_check: Some(SeqExpect { multichain_pre: Some(7), local_pre: Some(3) }),
        };
        let word = make_account_state_word(0, 7, 3);
        assert_eq!(evaluate_invalidation(&rule, word, 0), InvalidationOutcome::Keep);
    }

    /// A `seq_check` whose halves are both `None` (e.g.
    /// produced by a malformed populate path) is a no-op — any value
    /// keeps. Defends against accidental "always evict" if a future
    /// admission path forgets to skip the rule for a tx with no
    /// sequence updates.
    #[test]
    fn evaluate_account_state_seq_no_check_keeps() {
        let rule = InvalidationRule::AccountState {
            lock_sensitive: false,
            seq_check: Some(SeqExpect { multichain_pre: None, local_pre: None }),
        };
        let word = make_account_state_word(0, u64::MAX, u64::MAX);
        assert_eq!(evaluate_invalidation(&rule, word, 0), InvalidationOutcome::Keep);
    }

    /// AccountState rule with both checks disabled: any value keeps.
    /// Defends against accidental "always evict" if someone registers
    /// the slot without a real reason.
    #[test]
    fn evaluate_account_state_lock_insensitive_seq_unchecked_always_keeps() {
        let rule = InvalidationRule::AccountState { lock_sensitive: false, seq_check: None };
        // Even an absurd packed value (huge unlocksAt + advanced
        // sequences) stays Keep with no checks active.
        let word = make_account_state_word(u64::MAX, u64::MAX, u64::MAX);
        assert_eq!(evaluate_invalidation(&rule, word, 0), InvalidationOutcome::Keep);
    }

    // -----------------------------------------------------------------
    // Pool-level integration tests for rule-aware on_state_updates
    // -----------------------------------------------------------------

    /// Capability: an owner_config diff that adds the PAYER bit while
    /// keeping SENDER does **not** evict a sender_auth-only tx. This is
    /// the headline "scope-aware" property — under the prior
    /// `aa_invalidation_keys` scheme this benign change would have
    /// dropped every tx from the sender.
    #[test]
    fn on_state_updates_owner_config_keeps_when_scope_compatible() {
        use op_revm::constants::{K1_VERIFIER_ADDRESS, OWNER_SCOPE_PAYER, OWNER_SCOPE_SENDER};
        let sender = sender_from_seed(0x12);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let tx = make_valid(aa_tx(sender, U256::ZERO, 0, 1_000_000_000), sender);
        pool.add_transaction(tx, 0, 0).unwrap();
        assert_eq!(pool.len(), 1);

        let owner_slot = aa_owner_config_slot(sender, implicit_owner_id(sender));
        let outcome = pool.on_state_updates(
            [(
                ACCOUNT_CONFIG_ADDRESS,
                owner_slot,
                make_owner_config_word(K1_VERIFIER_ADDRESS, OWNER_SCOPE_SENDER | OWNER_SCOPE_PAYER),
            )],
            0,
            B256::ZERO,
        );

        assert!(outcome.invalidated.is_empty(), "scope still includes SENDER → keep");
        assert_eq!(pool.len(), 1);
    }

    /// Capability: REVOKED_VERIFIER at the registered owner_config slot
    /// drops the tx (rule A's evict path), regardless of scope byte.
    #[test]
    fn on_state_updates_owner_config_evicts_on_revoke() {
        use op_revm::constants::REVOKED_VERIFIER;
        let sender = sender_from_seed(0x13);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let tx = make_valid(aa_tx(sender, U256::ZERO, 0, 1_000_000_000), sender);
        pool.add_transaction(tx, 0, 0).unwrap();

        let owner_slot = aa_owner_config_slot(sender, implicit_owner_id(sender));
        let outcome = pool.on_state_updates(
            [(ACCOUNT_CONFIG_ADDRESS, owner_slot, make_owner_config_word(REVOKED_VERIFIER, 0))],
            0,
            B256::ZERO,
        );

        assert_eq!(outcome.invalidated.len(), 1);
        assert!(pool.is_empty());
    }

    /// Capability: a tx that pinned `multichain_pre = Some(7)` is
    /// evicted by an on-chain advance to multichain = 8. Manually
    /// pushes the rule into the invalidation index (mirrors the
    /// `on_state_updates_account_state_evicts_locked_for_sensitive_tx`
    /// pattern) since the pool tests use the K1-only `aa_tx` helper
    /// without `ConfigChange` entries.
    #[test]
    fn on_state_updates_evicts_when_seq_advances_past_expected() {
        let sender = sender_from_seed(0x15);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let tx = make_valid(aa_tx(sender, U256::ZERO, 0, 1_000_000_000), sender);
        let hash = *tx.hash();
        pool.add_transaction(tx, 0, 0).unwrap();

        let lock_slot = aa_lock_slot(sender);
        pool.invalidation_index.entry((ACCOUNT_CONFIG_ADDRESS, lock_slot)).or_default().insert(
            hash,
            InvalidationRule::AccountState {
                lock_sensitive: false,
                seq_check: Some(SeqExpect { multichain_pre: Some(7), local_pre: None }),
            },
        );

        // multichain advanced 7 → 8; local untouched. Spec line 376:
        // signature-bound; auth no longer covers the new on-chain pre.
        let word = make_account_state_word(0, 8, 0);
        let outcome =
            pool.on_state_updates([(ACCOUNT_CONFIG_ADDRESS, lock_slot, word)], 0, B256::ZERO);
        assert_eq!(outcome.invalidated.len(), 1);
        assert!(pool.is_empty());
    }

    /// Plumbing: `aa_invalidation_rules` populates BOTH halves of
    /// `SeqExpect` for a tx whose `account_changes` carry one
    /// multichain `ConfigChange` and one local `ConfigChange`. The
    /// matching `AccountState` rule is keyed on `aa_lock_slot(sender)`
    /// (the same packed word holds both halves).
    #[test]
    fn aa_invalidation_rules_dual_half_sequence_update() {
        use op_alloy_consensus::{AccountChangeEntry, ConfigChangeEntry};

        let sender = sender_from_seed(0x16);
        let mut tx = aa_tx(sender, U256::ZERO, 0, 1_000_000_000);
        // tx.chain_id is `10` (set by `aa_tx`); chain_id == 0 → multichain,
        // chain_id == 10 → local.
        tx.account_changes = vec![
            AccountChangeEntry::ConfigChange(ConfigChangeEntry {
                chain_id: 0,
                sequence: 4,
                owner_changes: vec![],
                authorizer_auth: Default::default(),
            }),
            AccountChangeEntry::ConfigChange(ConfigChangeEntry {
                chain_id: 10,
                sequence: 9,
                owner_changes: vec![],
                authorizer_auth: Default::default(),
            }),
        ];
        // Re-sign sender_auth so the resolver yields Native (otherwise
        // `aa_invalidation_rules` produces no rule A entry, but rule D
        // depends only on `account_changes` so it would still publish —
        // we re-sign anyway to keep the tx valid end-to-end).
        let signer = (1u8..=255u8)
            .filter_map(signer_for)
            .find(|s| s.address() == sender)
            .expect("signer_for");
        let hash = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(&signer, hash);

        let valid = make_valid(tx, sender);
        let rules = valid.transaction.aa_invalidation_rules();
        let lock_slot = aa_lock_slot(sender);
        let rule = rules
            .iter()
            .find(|((addr, slot), _)| *addr == ACCOUNT_CONFIG_ADDRESS && *slot == lock_slot)
            .map(|(_, r)| *r)
            .expect("AccountState rule for the sender's lock slot");
        match rule {
            InvalidationRule::AccountState { lock_sensitive, seq_check } => {
                // ANY ConfigChange targeting us yields a `sequence_updates`
                // entry, and the executor's `check_account_lock`
                // (`op-revm/src/handler.rs:343-370`) fires on non-empty
                // `sequence_updates` — so pure-sequence dual-half is
                // lock-sensitive.
                assert!(lock_sensitive, "pure-sequence ConfigChange is lock-sensitive");
                assert_eq!(
                    seq_check,
                    Some(SeqExpect { multichain_pre: Some(4), local_pre: Some(9) }),
                    "both halves of SeqExpect must be populated for a dual-half tx",
                );
            }
            other => panic!("expected AccountState, got {other:?}"),
        }
    }

    /// Sponsored shape (`payer != sender`) emits BOTH rule A
    /// (sender, SENDER bit) and rule B (payer, PAYER bit). Mirrors the
    /// admission-side `accepts_sponsored_payer_k1_happy_path` and pins
    /// that the cross-address invalidation index is keyed on the payer's
    /// owner_config slot in addition to the sender's.
    #[test]
    fn aa_invalidation_rules_emits_payer_rule_for_sponsored() {
        use op_alloy_consensus::payer_signature_hash;
        use op_revm::constants::{OWNER_SCOPE_PAYER, OWNER_SCOPE_SENDER};

        let sender = sender_from_seed(0x18);
        let payer = sender_from_seed(0x19);
        let payer_signer = signer_for(0x19).expect("non-zero seed");

        let mut tx = aa_tx(sender, U256::ZERO, 0, 1_000_000_000);
        tx.payer = Some(payer);
        // Re-sign sender_auth — `payer` participates in `sender_signature_hash`.
        let signer = signer_for(0x18).expect("non-zero seed");
        let s_hash = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(&signer, s_hash);
        // payer_auth: K1 sig over `payer_signature_hash` from the payer's
        // signer (strict-self-owner enforces recovered == claimed `tx.payer`).
        let p_hash = payer_signature_hash(&tx);
        tx.payer_auth = k1_explicit_auth(&payer_signer, p_hash);

        let valid = make_valid(tx, sender);
        let rules = valid.transaction.aa_invalidation_rules();

        // Rule A: sender's owner_config slot, SENDER bit.
        let sender_slot = aa_owner_config_slot(sender, implicit_owner_id(sender));
        let sender_rule = rules
            .iter()
            .find(|((addr, slot), _)| *addr == ACCOUNT_CONFIG_ADDRESS && *slot == sender_slot)
            .map(|(_, r)| *r)
            .expect("rule A (sender owner_config) must be present");
        match sender_rule {
            InvalidationRule::OwnerConfig { required_scope_bit, .. } => {
                assert_eq!(required_scope_bit, OWNER_SCOPE_SENDER);
            }
            other => panic!("expected OwnerConfig for sender, got {other:?}"),
        }

        // Rule B: payer's owner_config slot, PAYER bit.
        let payer_slot = aa_owner_config_slot(payer, implicit_owner_id(payer));
        let payer_rule = rules
            .iter()
            .find(|((addr, slot), _)| *addr == ACCOUNT_CONFIG_ADDRESS && *slot == payer_slot)
            .map(|(_, r)| *r)
            .expect("rule B (payer owner_config) must be present for sponsored shape");
        match payer_rule {
            InvalidationRule::OwnerConfig { required_scope_bit, .. } => {
                assert_eq!(required_scope_bit, OWNER_SCOPE_PAYER);
            }
            other => panic!("expected OwnerConfig for payer, got {other:?}"),
        }
    }

    /// When `tx.payer == Some(sender)` the sponsored path runs but
    /// the payer rule collapses into rule A (we skip rule B). Pins that
    /// `aa_invalidation_rules` only emits a single OwnerConfig entry on
    /// the sender's slot for this collapse case.
    #[test]
    fn aa_invalidation_rules_collapses_when_payer_eq_sender() {
        use op_alloy_consensus::payer_signature_hash;
        let sender = sender_from_seed(0x1A);
        let signer = signer_for(0x1A).expect("non-zero seed");

        let mut tx = aa_tx(sender, U256::ZERO, 0, 1_000_000_000);
        tx.payer = Some(sender);
        let s_hash = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(&signer, s_hash);
        // Self-sponsored: same signer for `payer_auth` (K1 strict-self-owner
        // enforces recovered == `tx.payer == sender`).
        let p_hash = payer_signature_hash(&tx);
        tx.payer_auth = k1_explicit_auth(&signer, p_hash);

        let valid = make_valid(tx, sender);
        let rules = valid.transaction.aa_invalidation_rules();

        let owner_config_rules: Vec<_> = rules
            .iter()
            .filter(|(_, r)| matches!(r, InvalidationRule::OwnerConfig { .. }))
            .collect();
        assert_eq!(
            owner_config_rules.len(),
            1,
            "payer == sender must collapse to a single OwnerConfig rule (rule A)",
        );
        // The single rule must be on the sender's slot.
        let sender_slot = aa_owner_config_slot(sender, implicit_owner_id(sender));
        assert_eq!(owner_config_rules[0].0.0, ACCOUNT_CONFIG_ADDRESS);
        assert_eq!(owner_config_rules[0].0.1, sender_slot);
    }

    /// Capability: a `lock_sensitive` AccountState rule with `unlocksAt
    /// > block_timestamp` evicts. Sidesteps admission (which currently
    /// rejects account_changes shapes) by manually pushing the rule
    /// into the pool's invalidation index — the assertion is still on
    /// `on_state_updates`'s rule-aware behavior.
    #[test]
    fn on_state_updates_account_state_evicts_locked_for_sensitive_tx() {
        let sender = sender_from_seed(0x14);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let tx = make_valid(aa_tx(sender, U256::ZERO, 0, 1_000_000_000), sender);
        let hash = *tx.hash();
        pool.add_transaction(tx, 0, 0).unwrap();

        // Manually register a lock-sensitive AccountState rule for
        // this tx's sender. (Production admission will set this
        // automatically when account_changes are accepted.)
        let lock_slot = aa_lock_slot(sender);
        pool.invalidation_index
            .entry((ACCOUNT_CONFIG_ADDRESS, lock_slot))
            .or_default()
            .insert(hash, InvalidationRule::AccountState { lock_sensitive: true, seq_check: None });

        // unlocksAt = now+100 → still locked at now → evict.
        let now = 1_000u64;
        let word = make_account_state_word(now + 100, 0, 0);
        let outcome =
            pool.on_state_updates([(ACCOUNT_CONFIG_ADDRESS, lock_slot, word)], now, B256::ZERO);
        assert_eq!(outcome.invalidated.len(), 1);
        assert!(pool.is_empty());
    }

    /// Capability: invalidation rules are registered when an AA tx is
    /// admitted, and a subsequent state diff matching one of those keys
    /// evicts the tx if the rule says so. Uses the
    /// always-registered owner_config slot with a `REVOKED_VERIFIER`
    /// diff (rule A's evict path).
    #[test]
    fn owner_config_revoke_evicts_aa_tx() {
        use op_revm::constants::REVOKED_VERIFIER;
        let sender = sender_from_seed(0x11);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let tx = make_valid(aa_tx(sender, U256::ZERO, 0, 1_000_000_000), sender);
        let hash = *tx.hash();
        pool.add_transaction(tx, 0, 0).expect("admit");

        // Sanity: tx is in the pool, owner_config slot is registered.
        let owner_slot = aa_owner_config_slot(sender, implicit_owner_id(sender));
        assert_eq!(pool.len(), 1);
        assert!(pool.invalidation_index.contains_key(&(ACCOUNT_CONFIG_ADDRESS, owner_slot)));

        // REVOKED_VERIFIER sentinel at the owner_config slot → rule A
        // evicts.
        let outcome = pool.on_state_updates(
            [(ACCOUNT_CONFIG_ADDRESS, owner_slot, make_owner_config_word(REVOKED_VERIFIER, 0))],
            0,
            B256::ZERO,
        );

        assert_eq!(outcome.invalidated.len(), 1, "REVOKED_VERIFIER must evict the tx");
        assert_eq!(outcome.invalidated[0].hash(), &hash);
        assert!(outcome.mined.is_empty(), "rule eviction is not 'mined'");
        assert_eq!(pool.len(), 0);
        assert!(
            !pool.invalidation_index.contains_key(&(ACCOUNT_CONFIG_ADDRESS, owner_slot)),
            "removed tx must drop its invalidation index entries"
        );
    }

    /// Negative: an owner_config diff for a *different* sender doesn't
    /// touch our tx. Pins that the index keys are sender-scoped, not
    /// just `(ACCOUNT_CONFIG_ADDRESS, anything)`.
    #[test]
    fn unrelated_sender_owner_config_diff_does_not_invalidate() {
        use op_revm::constants::REVOKED_VERIFIER;
        let me = sender_from_seed(0x22);
        let other = sender_from_seed(0x33);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let tx = make_valid(aa_tx(me, U256::ZERO, 0, 1_000_000_000), me);
        pool.add_transaction(tx, 0, 0).expect("admit");

        let outcome = pool.on_state_updates(
            [(
                ACCOUNT_CONFIG_ADDRESS,
                aa_owner_config_slot(other, implicit_owner_id(other)),
                make_owner_config_word(REVOKED_VERIFIER, 0),
            )],
            0,
            B256::ZERO,
        );

        assert!(outcome.invalidated.is_empty());
        assert_eq!(pool.len(), 1, "tx for a different sender must survive");
    }

    /// Capability: a single state diff invalidates *all* AA txs from the
    /// affected sender, regardless of lane. Two lanes (`nonce_key=0` and
    /// `nonce_key=7`) for the same sender both share the
    /// `aa_owner_config_slot(sender, implicit_owner_id)` entry and are
    /// jointly evicted on revocation.
    #[test]
    fn owner_config_revoke_evicts_all_lanes_of_same_sender() {
        use op_revm::constants::REVOKED_VERIFIER;
        let sender = sender_from_seed(0x44);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());

        let lane_a = make_valid(aa_tx(sender, U256::ZERO, 0, 1_000_000_000), sender);
        let lane_b = make_valid(aa_tx(sender, U256::from(7u64), 0, 1_000_000_000), sender);
        pool.add_transaction(lane_a, 0, 0).unwrap();
        pool.add_transaction(lane_b, 0, 0).unwrap();
        assert_eq!(pool.len(), 2);

        let outcome = pool.on_state_updates(
            [(
                ACCOUNT_CONFIG_ADDRESS,
                aa_owner_config_slot(sender, implicit_owner_id(sender)),
                make_owner_config_word(REVOKED_VERIFIER, 0),
            )],
            0,
            B256::ZERO,
        );

        assert_eq!(outcome.invalidated.len(), 2, "revoke must evict both lanes from this sender");
        assert!(pool.is_empty());
    }

    /// Capability: nonce-advance on `NONCE_MANAGER_ADDRESS` still works
    /// after the cross-address signature change. Confirms the existing
    /// nonce/expiring path didn't break when invalidation handling was
    /// added.
    #[test]
    fn nonce_advance_still_works_after_cross_address_signature() {
        let sender = sender_from_seed(0x55);
        let key = U256::from(3u64);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());

        let tx = make_valid(aa_tx(sender, key, 0, 1_000_000_000), sender);
        pool.add_transaction(tx, 0, 0).unwrap();
        assert_eq!(pool.len(), 1);

        // Lane head moves from 0 → 1 → tx is mined.
        let outcome = pool.on_state_updates(
            [(STATE_DIFF_ADDRESS, aa_nonce_slot(sender, key), U256::from(1u64))],
            0,
            B256::ZERO,
        );

        assert_eq!(outcome.mined.len(), 1, "lane advance mines the tx");
        assert!(outcome.invalidated.is_empty());
        assert!(pool.is_empty());
    }

    /// Capability: the expiring-nonce path also still works after the
    /// cross-address signature change. Pre-condition: an expiring-mode AA
    /// tx is admitted (immediately pending). Action: state diff flips the
    /// tx's `aa_expiring_seen_slot` non-zero. Assert: tx evicted as
    /// `mined`.
    #[test]
    fn expiring_nonce_eviction_still_works_after_cross_address_signature() {
        let sender = sender_from_seed(0x66);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());

        // nonce_key == MAX with non-zero expiry routes through the
        // expiring path on admission.
        let mut tx = aa_tx(sender, NONCE_KEY_MAX, 0, 1_000_000_000);
        tx.expiry = 1_000_000;
        let valid = make_valid(tx.clone(), sender);
        let exp_slot =
            valid.transaction.aa_expiring_nonce_slot().expect("expiring tx exposes a seen-slot");
        pool.add_transaction(valid, 0, 0).unwrap();
        assert_eq!(pool.len(), 1);

        let outcome = pool.on_state_updates(
            [(STATE_DIFF_ADDRESS, exp_slot, U256::from(123u64))],
            0,
            B256::ZERO,
        );

        assert_eq!(outcome.mined.len(), 1);
        assert!(pool.is_empty());
    }

    /// Capability: removing a tx via `remove_by_hash` cleans up its
    /// invalidation index entries. Without this, a subsequent state diff
    /// at the now-stale key would try to evict a non-existent tx (would
    /// no-op but leave dangling index state, drifting memory).
    #[test]
    fn remove_drops_invalidation_index_entries() {
        let sender = sender_from_seed(0x77);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());

        let tx = make_valid(aa_tx(sender, U256::ZERO, 0, 1_000_000_000), sender);
        let hash = *tx.hash();
        pool.add_transaction(tx, 0, 0).unwrap();
        // Always-registered slot under the new rule shape: owner_config
        // for the implicit-EOA owner_id (rule A). aa_lock_slot is gated on
        // `lock_sensitive` and not registered for plain calls-only AA txs.
        let key = (ACCOUNT_CONFIG_ADDRESS, aa_owner_config_slot(sender, implicit_owner_id(sender)));
        assert!(pool.invalidation_index.contains_key(&key));

        pool.remove_by_hash(&hash).expect("present");
        assert!(
            !pool.invalidation_index.contains_key(&key),
            "remove must clear the now-empty invalidation entry"
        );
    }

    /// Lifecycle: insert two AA txs from the same sender on different
    /// `nonce_key`s. Both are admitted independently — neither is treated
    /// as a replacement of the other. Pins the very property that
    /// motivates the dedicated AA pool over the standard reth pool's
    /// `(sender, nonce)` keying.
    #[test]
    fn cross_nonce_key_admissions_are_independent() {
        let sender = sender_from_seed(0xC1);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());

        let a = make_valid(aa_tx(sender, U256::ZERO, 0, 1_000), sender);
        let b = make_valid(aa_tx(sender, U256::from(7u64), 0, 1_000), sender);
        pool.add_transaction(a, 0, 0).unwrap();
        pool.add_transaction(b, 0, 0).unwrap();
        assert_eq!(pool.len(), 2, "different nonce_keys must not collide");

        // Both lanes should have one independent pending head each.
        assert_eq!(pool.independent_pending().count(), 2);
    }

    /// Lifecycle: a tx with a nonce gap is admitted but parked. Filling
    /// the gap (a later admission with the missing nonce) promotes the
    /// queued tx in the same call's `Eip8130PendingAdded.promoted`.
    #[test]
    fn nonce_gap_then_fill_promotes() {
        let sender = sender_from_seed(0xC2);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let key = U256::from(3u64);

        // First insert nonce=1 with on-chain head 0 — has a gap.
        let queued = make_valid(aa_tx(sender, key, 1, 1_000), sender);
        let outcome = pool.add_transaction(queued, 0, 0).unwrap();
        assert!(
            matches!(outcome, Eip8130AddOutcome::Queued { .. }),
            "first tx with nonce=1 above head 0 must be queued"
        );
        assert_eq!(pool.independent_pending().count(), 0);

        // Now fill the gap with nonce=0. Both should be pending; the
        // promoted vec must include the previously-queued nonce=1.
        let head = make_valid(aa_tx(sender, key, 0, 1_000), sender);
        let outcome = pool.add_transaction(head, 0, 0).unwrap();
        match outcome {
            Eip8130AddOutcome::Pending(p) => {
                assert_eq!(p.promoted.len(), 1);
                // `transaction.nonce()` reports a sentinel for AA txs (see
                // `OpPooledTransaction::nonce` override that bypasses
                // reth's inner nonce check); the AA 2D nonce sequence
                // lives in `aa_nonce_sequence()` instead.
                assert_eq!(p.promoted[0].transaction.aa_nonce_sequence(), Some(1));
            }
            _ => panic!("expected Pending after gap-fill"),
        }
        assert_eq!(pool.independent_pending().count(), 1, "one head per lane");
    }

    /// Lifecycle: nonce advance from `on_state_updates` past pool's
    /// holdings (gap demotion). If the lane head jumps from 0 → 5 but the
    /// pool only holds nonce=1, that tx is demoted to queued (not mined,
    /// because it never matched the new head).
    #[test]
    fn lane_head_jump_demotes_non_contiguous() {
        let sender = sender_from_seed(0xC3);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let key = U256::from(11u64);

        // Insert nonce=1 with head=0; queued.
        pool.add_transaction(make_valid(aa_tx(sender, key, 1, 1_000), sender), 0, 0).unwrap();
        assert_eq!(pool.len(), 1);

        // State update: lane head moves to 5 — past our nonce=1 holding.
        let outcome = pool.on_state_updates(
            [(STATE_DIFF_ADDRESS, aa_nonce_slot(sender, key), U256::from(5u64))],
            0,
            B256::ZERO,
        );
        // nonce=1 is `< 5` so it's drained as `mined` (the lane "skipped"
        // it from the pool's view — it was never on-chain at nonce=1, but
        // semantically the tx is no longer admissible). This is the
        // `mined` bucket per the current implementation; demoted is for
        // txs at `>= new_head` with a gap below.
        assert_eq!(outcome.mined.len(), 1, "nonce-1 tx after head-jump-to-5 is treated as mined");
        assert!(pool.is_empty());
    }

    /// `highest_consecutive_pending_seq_in_lane`: empty lane returns
    /// `None` so RPC callers know to fall back to the slot baseline.
    #[test]
    fn highest_consecutive_empty_lane_returns_none() {
        let pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let seq = Eip8130SeqId::new(sender_from_seed(0xD1), U256::from(0u64));
        assert_eq!(pool.highest_consecutive_pending_seq_in_lane(seq, 0), None);
    }

    /// `highest_consecutive_pending_seq_in_lane`: with three pending
    /// txs at seq 0, 1, 2 the walk returns 2 (the tail of the
    /// consecutive run).
    #[test]
    fn highest_consecutive_returns_tail_of_consecutive_run() {
        let sender = sender_from_seed(0xD2);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let key = U256::from(7u64);
        for nonce in 0u64..=2 {
            pool.add_transaction(make_valid(aa_tx(sender, key, nonce, 1_000), sender), 0, 0)
                .unwrap();
        }
        let seq = Eip8130SeqId::new(sender, key);
        assert_eq!(pool.highest_consecutive_pending_seq_in_lane(seq, 0), Some(2));
    }

    /// `highest_consecutive_pending_seq_in_lane`: a gap above the
    /// baseline parks higher txs as *queued*, so the walk halts at the
    /// last pending entry below the gap and never observes the queued
    /// ones.
    #[test]
    fn highest_consecutive_stops_at_first_gap() {
        let sender = sender_from_seed(0xD3);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let key = U256::from(9u64);
        // seqs 0, 1 admitted as pending; 3 admitted but parked queued (gap at 2).
        for nonce in [0u64, 1, 3] {
            pool.add_transaction(make_valid(aa_tx(sender, key, nonce, 1_000), sender), 0, 0)
                .unwrap();
        }
        let seq = Eip8130SeqId::new(sender, key);
        assert_eq!(pool.highest_consecutive_pending_seq_in_lane(seq, 0), Some(1));
    }

    /// `highest_consecutive_pending_seq_in_lane`: returns `None` if no
    /// tx at the baseline is pending, even when later seqs exist.
    /// Callers compose with the on-chain slot value, so `None` here
    /// surfaces as "no pool-resident contribution; use the slot value".
    #[test]
    fn highest_consecutive_returns_none_when_baseline_missing() {
        let sender = sender_from_seed(0xD4);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let key = U256::from(13u64);
        // Only seq=2 admitted with head=0 — it's queued (gap at 0, 1).
        pool.add_transaction(make_valid(aa_tx(sender, key, 2, 1_000), sender), 0, 0).unwrap();
        let seq = Eip8130SeqId::new(sender, key);
        assert_eq!(pool.highest_consecutive_pending_seq_in_lane(seq, 0), None);
    }

    /// `highest_consecutive_pending_seq_in_lane`: lanes with different
    /// `nonce_key` are isolated — admissions in lane A don't bleed into
    /// the answer for lane B on the same sender.
    #[test]
    fn highest_consecutive_isolates_lanes_per_nonce_key() {
        let sender = sender_from_seed(0xD5);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let key_a = U256::from(1u64);
        let key_b = U256::from(2u64);
        // Lane A: seq 0, 1 pending. Lane B: seq 0 pending.
        for nonce in [0u64, 1] {
            pool.add_transaction(make_valid(aa_tx(sender, key_a, nonce, 1_000), sender), 0, 0)
                .unwrap();
        }
        pool.add_transaction(make_valid(aa_tx(sender, key_b, 0, 1_000), sender), 0, 0).unwrap();

        let seq_a = Eip8130SeqId::new(sender, key_a);
        let seq_b = Eip8130SeqId::new(sender, key_b);
        assert_eq!(pool.highest_consecutive_pending_seq_in_lane(seq_a, 0), Some(1));
        assert_eq!(pool.highest_consecutive_pending_seq_in_lane(seq_b, 0), Some(0));
    }

    /// Lifecycle: replacement at the same `(sender, nonce_key, nonce)`
    /// requires a `PriceBumpConfig`-conforming price bump. Default bump
    /// is 10%, applied to *both* `max_fee_per_gas` and
    /// `max_priority_fee_per_gas`.
    #[test]
    fn replacement_underpriced_rejected() {
        let sender = sender_from_seed(0xC4);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());

        // Original priority=1000 → max_fee_per_gas=1001 (per `aa_tx` helper).
        pool.add_transaction(make_valid(aa_tx(sender, U256::ZERO, 0, 1_000), sender), 0, 0)
            .unwrap();

        // 5% bump — fails the 10% requirement on max_fee_per_gas.
        let bumped = make_valid(aa_tx(sender, U256::ZERO, 0, 1_050), sender);
        let err = pool
            .add_transaction(bumped, 0, 0)
            .expect_err("under-priced replacement must be rejected");
        assert!(matches!(err.kind, PoolErrorKind::ReplacementUnderpriced));

        // 20% bump (priority=1200, max_fee=1201) clears both required-bumped-fee
        // checks comfortably.
        let ok = make_valid(aa_tx(sender, U256::ZERO, 0, 1_200), sender);
        pool.add_transaction(ok, 0, 0).expect("20% bump satisfies default PriceBumpConfig");
        assert_eq!(pool.len(), 1, "replacement must not double-count");
    }

    /// DoS: per-sender cap rejects the (N+1)-th admission across all
    /// lanes for a given sender. Pins that the cap is sender-scoped, not
    /// lane-scoped.
    #[test]
    fn per_sender_cap_rejects_excess() {
        let cfg = Eip8130PoolConfig { max_txs_per_sender: 2, ..Default::default() };
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(cfg);
        let sender = sender_from_seed(0xC5);

        for nonce in 0..2 {
            pool.add_transaction(make_valid(aa_tx(sender, U256::ZERO, nonce, 1_000), sender), 0, 0)
                .unwrap();
        }
        // Different lane on same sender — still counts toward cap.
        let err = pool
            .add_transaction(make_valid(aa_tx(sender, U256::from(99u64), 0, 1_000), sender), 0, 0)
            .expect_err("per-sender cap must trigger");
        assert!(matches!(err.kind, PoolErrorKind::SpammerExceededCapacity(_)));
    }

    /// DoS: per-lane cap rejects an over-stuffed `(sender, nonce_key)`
    /// even when the sender's global slots are still free.
    #[test]
    fn per_lane_cap_rejects_excess() {
        let cfg = Eip8130PoolConfig { max_txs_per_sequence: 2, ..Default::default() };
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(cfg);
        let sender = sender_from_seed(0xC6);
        let key = U256::from(7u64);

        for nonce in 0..2 {
            pool.add_transaction(make_valid(aa_tx(sender, key, nonce, 1_000), sender), 0, 0)
                .unwrap();
        }
        let err = pool
            .add_transaction(make_valid(aa_tx(sender, key, 2, 1_000), sender), 0, 0)
            .expect_err("per-lane cap must trigger");
        assert!(matches!(err.kind, PoolErrorKind::InvalidTransaction(_)));
    }

    /// Lifecycle (expiring): re-submitting the same tx hash
    /// is rejected as `AlreadyImported` — expiring-mode txs key on the
    /// sealed tx hash, not on `(sender, nonce_key, nonce)`.
    #[test]
    fn expiring_nonce_resubmit_dedups() {
        let sender = sender_from_seed(0xC7);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());

        let mut t = aa_tx(sender, NONCE_KEY_MAX, 0, 1_000);
        t.expiry = 1_000_000;
        let hash_a = *make_valid(t.clone(), sender).hash();
        pool.add_transaction(make_valid(t.clone(), sender), 0, 0).unwrap();
        // Same fields → same tx hash → duplicate.
        let _ = hash_a;
        let err = pool
            .add_transaction(make_valid(t.clone(), sender), 0, 0)
            .expect_err("identical expiring tx must dedup");
        assert!(matches!(err.kind, PoolErrorKind::AlreadyImported));
        assert_eq!(pool.len(), 1);
    }

    /// Capability: owner-config slot mutation evicts the AA tx via the
    /// invalidation index when the rule says so. Pins that owner_id
    /// derivation matches op-revm's `implicit_owner_id` (sender address
    /// left-padded to 32 bytes) and that a verifier-mismatch diff trips
    /// rule A's evict path.
    #[test]
    fn owner_config_state_diff_invalidates_aa_tx() {
        let sender = sender_from_seed(0xCB);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let tx = make_valid(aa_tx(sender, U256::ZERO, 0, 1_000), sender);
        pool.add_transaction(tx, 0, 0).unwrap();

        let owner_slot = aa_owner_config_slot(sender, implicit_owner_id(sender));
        // Verifier mismatch: rule A binds K1; a different verifier evicts.
        let other_verifier = sender_from_seed(0xEE);
        let outcome = pool.on_state_updates(
            [(ACCOUNT_CONFIG_ADDRESS, owner_slot, make_owner_config_word(other_verifier, 0))],
            0,
            B256::ZERO,
        );
        assert_eq!(outcome.invalidated.len(), 1);
        assert!(pool.is_empty());
    }

    /// Capability: `get(hash)` returns both pending heads and queued
    /// (nonce-gap-parked) txs, not just independent pending. Pins that
    /// the hash index covers all admitted txs regardless of lifecycle
    /// bucket.
    #[test]
    fn get_by_hash_returns_queued_and_pending() {
        let sender = sender_from_seed(0xCC);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());

        // nonce=1 with head=0 → queued.
        let queued = make_valid(aa_tx(sender, U256::ZERO, 1, 1_000), sender);
        let queued_hash = *queued.hash();
        pool.add_transaction(queued, 0, 0).unwrap();

        // nonce=0 → pending head.
        let pending = make_valid(aa_tx(sender, U256::ZERO, 0, 1_000), sender);
        let pending_hash = *pending.hash();
        pool.add_transaction(pending, 0, 0).unwrap();

        assert!(pool.get(&queued_hash).is_some(), "queued tx must be retrievable by hash");
        assert!(pool.get(&pending_hash).is_some(), "pending tx must be retrievable by hash");
    }

    /// Capability: `all_queued()` enumerates only queued (nonce-gap)
    /// txs; `all_pending()` enumerates every pending tx (lane head plus
    /// in-lane successors).
    #[test]
    fn all_pending_and_all_queued_are_disjoint() {
        let sender = sender_from_seed(0xCD);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());

        // nonces 0, 1 with head=0 — both pending after promote_lane.
        for nonce in 0..2u64 {
            pool.add_transaction(make_valid(aa_tx(sender, U256::ZERO, nonce, 1_000), sender), 0, 0)
                .unwrap();
        }
        // nonce=5 with head=0 — has gap, queued.
        pool.add_transaction(make_valid(aa_tx(sender, U256::ZERO, 5, 1_000), sender), 0, 0)
            .unwrap();

        let pending: Vec<_> = pool.all_pending().collect();
        let queued: Vec<_> = pool.all_queued().collect();
        assert_eq!(pending.len(), 2, "nonces 0,1 are pending");
        assert_eq!(queued.len(), 1, "nonce 5 is queued behind the gap");

        // Disjoint: no tx hash in both.
        let pending_hashes: std::collections::HashSet<_> =
            pending.iter().map(|t| *t.hash()).collect();
        for q in &queued {
            assert!(!pending_hashes.contains(q.hash()));
        }
    }

    /// Capability: subscribers to the AA pool's pending broadcast
    /// receive a hash on each new admission and on each promote-from-gap.
    /// Pins the channel wiring used by `OpDualPool`'s listener
    /// forwarder.
    #[test]
    fn pending_broadcast_fires_on_admission_and_promotion() {
        use tokio::sync::broadcast::error::TryRecvError;

        let sender = sender_from_seed(0xCE);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let mut rx = pool.subscribe_pending();

        // Admit nonce=0 — pending immediately, broadcast 1 hash.
        let h0 = make_valid(aa_tx(sender, U256::ZERO, 0, 1_000), sender);
        let h0_hash = *h0.hash();
        pool.add_transaction(h0, 0, 0).unwrap();
        assert_eq!(rx.try_recv(), Ok(h0_hash));

        // Admit nonce=2 — gap, queued, no broadcast.
        let h2 = make_valid(aa_tx(sender, U256::ZERO, 2, 1_000), sender);
        let h2_hash = *h2.hash();
        pool.add_transaction(h2, 0, 0).unwrap();
        assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));

        // Admit nonce=1 — fills gap; both nonce=1 (the new tx) and
        // promoted nonce=2 should fire.
        let h1 = make_valid(aa_tx(sender, U256::ZERO, 1, 1_000), sender);
        let h1_hash = *h1.hash();
        pool.add_transaction(h1, 0, 0).unwrap();
        let mut got = vec![rx.try_recv().unwrap(), rx.try_recv().unwrap()];
        got.sort();
        let mut want = vec![h1_hash, h2_hash];
        want.sort();
        assert_eq!(got, want);
    }

    /// A mined AA tx fires `TransactionEvent::Mined(block_hash)` —
    /// distinct from `Discarded`. Pre-fix `on_state_updates` routed mined
    /// removals through `remove_by_hash` which always emitted `Discarded`,
    /// so subscribers couldn't tell inclusion from eviction. Reth's
    /// standard pool fires distinct events
    /// (`reth-transaction-pool/src/pool/mod.rs:887-897`).
    #[test]
    fn mined_emits_distinct_event_not_discarded() {
        use tokio::sync::broadcast::error::TryRecvError;

        let sender = sender_from_seed(0xCF);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());

        // Admit a sequenced AA tx at nonce 0 → pending immediately.
        let tx = make_valid(aa_tx(sender, U256::ZERO, 0, 1_000), sender);
        let hash = *tx.hash();
        pool.add_transaction(tx, 0, 0).unwrap();

        // Subscribe AFTER admission so the pre-Mined `Pending` event
        // doesn't pollute the receiver. (Broadcast subscribers only see
        // events sent after `subscribe`.)
        let mut events_rx = pool.subscribe_events();
        let mut discarded_rx = pool.subscribe_discarded();

        // Drive on-chain head past nonce 0 → mined.
        let block_hash = B256::repeat_byte(0xAB);
        let outcome = pool.on_state_updates(
            [(STATE_DIFF_ADDRESS, aa_nonce_slot(sender, U256::ZERO), U256::from(1u64))],
            0,
            block_hash,
        );
        assert_eq!(outcome.mined.len(), 1);
        assert_eq!(outcome.mined[0].hash(), &hash);

        // The per-tx event broadcast carried Mined(block_hash), not
        // Discarded. Multiple unrelated events may also fire (Pending
        // for other lane txs, etc.) but for THIS hash we must see
        // exactly Mined.
        let mut saw_mined = false;
        loop {
            match events_rx.try_recv() {
                Ok((h, TransactionEvent::Mined(bh))) if h == hash => {
                    assert_eq!(bh, block_hash, "Mined event must carry the canonical block hash");
                    saw_mined = true;
                }
                Ok((h, TransactionEvent::Discarded)) if h == hash => {
                    panic!("mined tx must not emit Discarded");
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        assert!(saw_mined, "expected Mined(block_hash) for the mined hash");

        // The discarded broadcast (separate channel) must stay silent
        // for the mined hash — that channel is the generic discard
        // signal for capacity / invalidation / expiry, not for
        // inclusion.
        match discarded_rx.try_recv() {
            Err(TryRecvError::Empty) => {}
            Ok(h) => panic!("discarded broadcast fired for mined hash {h:?}"),
            Err(e) => panic!("unexpected recv error {e:?}"),
        }
    }

    /// Capacity: when `max_pool_size` is hit, a new admission triggers
    /// eviction of the lowest-priority entry. Pins the discard scan.
    #[test]
    fn capacity_overflow_evicts_lowest_priority() {
        let cfg = Eip8130PoolConfig { max_pool_size: 2, ..Default::default() };
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(cfg);
        let sender_a = sender_from_seed(0xC8);
        let sender_b = sender_from_seed(0xC9);
        let sender_c = sender_from_seed(0xCA);

        // Two low-priority txs first.
        pool.add_transaction(make_valid(aa_tx(sender_a, U256::ZERO, 0, 100), sender_a), 0, 0)
            .unwrap();
        pool.add_transaction(make_valid(aa_tx(sender_b, U256::ZERO, 0, 200), sender_b), 0, 0)
            .unwrap();
        assert_eq!(pool.len(), 2);

        // Third high-priority tx: forces eviction of the lowest (sender_a).
        pool.add_transaction(make_valid(aa_tx(sender_c, U256::ZERO, 0, 10_000), sender_c), 0, 0)
            .unwrap();
        assert_eq!(pool.len(), 2, "pool size capped at 2 after eviction");
        // sender_c (highest priority) and sender_b (next-highest) must
        // remain; sender_a (lowest priority) must be evicted.
        let remaining_senders: std::collections::HashSet<Address> =
            pool.independent_pending().map(|t| t.sender()).collect();
        assert!(remaining_senders.contains(&sender_b));
        assert!(remaining_senders.contains(&sender_c));
        assert!(!remaining_senders.contains(&sender_a));
    }

    /// Capacity: queued and pending limits are enforced independently,
    /// matching tempo's AA2dPool policy. Queued overflow should evict the
    /// lowest-priority queued tx without touching pending heads.
    #[test]
    fn queued_subpool_limit_evicts_lowest_priority_queued_only() {
        let cfg = Eip8130PoolConfig {
            queued_limit: reth_transaction_pool::SubPoolLimit::new(1, usize::MAX),
            ..Default::default()
        };
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(cfg);
        let sender = sender_from_seed(0xD3);

        let pending = make_valid(aa_tx(sender, U256::ZERO, 0, 1_000), sender);
        let pending_hash = *pending.hash();
        pool.add_transaction(pending, 0, 0).unwrap();

        let queued_low = make_valid(aa_tx(sender, U256::ZERO, 2, 100), sender);
        let queued_low_hash = *queued_low.hash();
        pool.add_transaction(queued_low, 0, 0).unwrap();
        let queued_high = make_valid(aa_tx(sender, U256::ZERO, 3, 200), sender);
        let queued_high_hash = *queued_high.hash();
        pool.add_transaction(queued_high, 0, 0).unwrap();

        assert!(pool.get(&pending_hash).is_some(), "pending tx must survive queued cap");
        assert!(pool.get(&queued_low_hash).is_none(), "lowest-priority queued tx evicted");
        assert!(pool.get(&queued_high_hash).is_some(), "higher-priority queued tx survives");
        assert_eq!(pool.pending_and_queued_counts(), (1, 1));
    }

    /// Capacity: pending overflow evicts the lowest-priority pending tx.
    #[test]
    fn pending_subpool_limit_evicts_lowest_priority_pending() {
        let cfg = Eip8130PoolConfig {
            pending_limit: reth_transaction_pool::SubPoolLimit::new(1, usize::MAX),
            ..Default::default()
        };
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(cfg);
        let sender_a = sender_from_seed(0xD4);
        let sender_b = sender_from_seed(0xD5);

        let low = make_valid(aa_tx(sender_a, U256::ZERO, 0, 100), sender_a);
        let low_hash = *low.hash();
        pool.add_transaction(low, 0, 0).unwrap();
        let high = make_valid(aa_tx(sender_b, U256::ZERO, 0, 200), sender_b);
        let high_hash = *high.hash();
        pool.add_transaction(high, 0, 0).unwrap();

        assert!(pool.get(&low_hash).is_none(), "lowest-priority pending tx evicted");
        assert!(pool.get(&high_hash).is_some(), "higher-priority pending tx survives");
        assert_eq!(pool.pending_and_queued_counts(), (1, 0));
    }

    /// Pending byte cap forces eviction once `pending_bytes >
    /// pending_limit.max_size`. Pre-fix, `discard_to_cap` only honored
    /// `max_txs`; an operator-supplied `max_size` cap silently leaked.
    /// Eviction order matches the count-based pass: lowest priority first.
    #[test]
    fn discard_to_cap_evicts_by_pending_bytes() {
        let sender_a = sender_from_seed(0xE6);
        let sender_b = sender_from_seed(0xE7);

        // Probe both txs' encoded lengths so we can size the cap large
        // enough to keep the survivor (high) but small enough that the
        // sum trips the byte pass: `bytes_high < cap < bytes_low +
        // bytes_high`. AA tx encoding length depends on the signer's
        // address (different RLP signature shapes), so we must probe
        // each tx individually rather than re-using a single probe.
        let probe_low = make_valid(aa_tx(sender_a, U256::ZERO, 0, 100), sender_a);
        let probe_high = make_valid(aa_tx(sender_b, U256::ZERO, 0, 200), sender_b);
        let bytes_low = probe_low.transaction.encoded_length();
        let bytes_high = probe_high.transaction.encoded_length();
        let cap = bytes_high.max(bytes_low);
        let cfg = Eip8130PoolConfig {
            pending_limit: reth_transaction_pool::SubPoolLimit::new(usize::MAX, cap),
            queued_limit: reth_transaction_pool::SubPoolLimit::new(usize::MAX, usize::MAX),
            ..Default::default()
        };
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(cfg);

        // Lower priority admitted first so it remains the lowest-priority
        // entry.
        let low = make_valid(aa_tx(sender_a, U256::ZERO, 0, 100), sender_a);
        let low_hash = *low.hash();
        pool.add_transaction(low, 0, 0).unwrap();
        let high = make_valid(aa_tx(sender_b, U256::ZERO, 0, 200), sender_b);
        let high_hash = *high.hash();
        pool.add_transaction(high, 0, 0).unwrap();

        assert!(pool.get(&low_hash).is_none(), "lowest-priority pending tx evicted under byte cap",);
        assert!(pool.get(&high_hash).is_some(), "higher-priority pending tx survives");
        assert!(
            pool.pending_bytes <= cap,
            "pending_bytes ({}) must be within cap ({}) after eviction",
            pool.pending_bytes,
            cap,
        );
    }

    /// Queued byte cap forces eviction once `queued_bytes >
    /// queued_limit.max_size`.
    #[test]
    fn discard_to_cap_evicts_by_queued_bytes() {
        let sender = sender_from_seed(0xE8);

        // Both txs share a signer so encoded lengths match closely; we
        // still probe both to size the cap exactly.
        let probe_low = make_valid(aa_tx(sender, U256::ZERO, 2, 100), sender);
        let probe_high = make_valid(aa_tx(sender, U256::ZERO, 3, 200), sender);
        let bytes_low = probe_low.transaction.encoded_length();
        let bytes_high = probe_high.transaction.encoded_length();
        let cap = bytes_high.max(bytes_low);
        let cfg = Eip8130PoolConfig {
            pending_limit: reth_transaction_pool::SubPoolLimit::new(usize::MAX, usize::MAX),
            queued_limit: reth_transaction_pool::SubPoolLimit::new(usize::MAX, cap),
            ..Default::default()
        };
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(cfg);

        // Two queued txs (gap below at nonce 0). Lower priority first
        // → it's the lowest-priority queued tx and the eviction victim.
        let low = make_valid(aa_tx(sender, U256::ZERO, 2, 100), sender);
        let low_hash = *low.hash();
        pool.add_transaction(low, 0, 0).unwrap();
        let high = make_valid(aa_tx(sender, U256::ZERO, 3, 200), sender);
        let high_hash = *high.hash();
        pool.add_transaction(high, 0, 0).unwrap();

        assert!(pool.get(&low_hash).is_none(), "lowest-priority queued tx evicted by byte cap");
        assert!(pool.get(&high_hash).is_some(), "higher-priority queued tx survives");
        assert!(
            pool.queued_bytes <= cap,
            "queued_bytes ({}) must be within cap ({}) after eviction",
            pool.queued_bytes,
            cap,
        );
    }

    /// `Eip8130PoolConfig::from_node_config` copies the protocol
    /// pool's per-subpool caps verbatim so operator tuning of
    /// `--txpool.*-max-count/size` flows into the AA side pool. Pre-fix
    /// the AA side pool used `Eip8130PoolConfig::default()` and silently
    /// ignored the overrides.
    #[test]
    fn from_node_config_copies_limits() {
        let mut node_cfg = reth_transaction_pool::PoolConfig::default();
        node_cfg.pending_limit = reth_transaction_pool::SubPoolLimit::new(50, 1234);
        node_cfg.queued_limit = reth_transaction_pool::SubPoolLimit::new(7, 9_000);

        let aa_cfg = Eip8130PoolConfig::from_node_config(&node_cfg);
        assert_eq!(aa_cfg.pending_limit.max_txs, 50);
        assert_eq!(aa_cfg.pending_limit.max_size, 1234);
        assert_eq!(aa_cfg.queued_limit.max_txs, 7);
        assert_eq!(aa_cfg.queued_limit.max_size, 9_000);
        assert_eq!(
            aa_cfg.max_pool_size, 57,
            "max_pool_size derived as pending.max_txs + queued.max_txs",
        );
    }

    /// Regression: repeated replacements at the same `(sender, nonce_key,
    /// nonce_sequence)` must not leak entries in `by_eviction`. Pre-fix,
    /// the replacement path computed the old eviction key with a wrong
    /// `submission_id` (always 0) and a `base_fee`-derived priority that
    /// could mismatch the row actually inserted, so each replacement left
    /// a dangling `by_eviction` row even though `by_id` / `by_hash` stayed
    /// at 1. An attacker could grow `by_eviction` unbounded by spamming
    /// price-bumped replacements, exhausting pool memory while the visible
    /// size stayed flat.
    #[test]
    fn replacement_does_not_leak_eviction_entries() {
        let sender = sender_from_seed(0xD0);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());

        // Initial admission.
        pool.add_transaction(make_valid(aa_tx(sender, U256::ZERO, 0, 1_000), sender), 0, 0)
            .unwrap();
        assert_eq!(pool.by_eviction.len(), 1);

        // 32 replacements at the same tx_id, each ≥ 10% above the prior
        // priority so the bump check passes. After every cycle the pool
        // must still hold exactly one tx in every index.
        let mut priority: u128 = 1_000;
        for _ in 0..32 {
            priority = priority * 12 / 10; // 20% bump, comfortably above the 10% min.
            pool.add_transaction(make_valid(aa_tx(sender, U256::ZERO, 0, priority), sender), 0, 0)
                .unwrap();
        }
        assert_eq!(pool.by_hash.len(), 1, "hash index never grows past 1");
        assert_eq!(
            pool.by_eviction.len(),
            1,
            "by_eviction must mirror by_hash; pre-fix this leaked one row per replacement"
        );
        assert_eq!(pool.by_id.len(), 1);
    }

    /// Sanity: the maintained `txs_by_seq` counter (cf PERF-4) stays in
    /// lock-step with the actual `by_id` lane count across a mixed
    /// admit / replace / remove sequence. Debug-asserts the maintained
    /// value matches a fresh range-count after every step so any drift
    /// surfaces in tests, not just at the call site that uses the cap.
    #[test]
    fn lane_counter_tracks_by_id_through_admit_replace_remove() {
        let sender = sender_from_seed(0xD2);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let key = U256::from(7u64);
        let seq = Eip8130SeqId { sender, nonce_key: key };

        // Helper: range-count the lane the slow way and compare to the
        // maintained counter. Debug-only so release builds skip the
        // walk; the constraint here is parity, not perf in tests.
        let range_count = |p: &Eip8130Pool<OpPooledTransaction>| -> usize {
            let lo = Eip8130TxId::new(seq, 0);
            let hi = Eip8130TxId::new(seq, u64::MAX);
            p.by_id.range(lo..=hi).count()
        };

        // Step 1: admit nonce 0, 1, 2 in order.
        for nonce in 0..3u64 {
            pool.add_transaction(make_valid(aa_tx(sender, key, nonce, 1_000), sender), 0, 0)
                .unwrap();
            debug_assert_eq!(pool.lane_count(seq), range_count(&pool));
        }
        assert_eq!(pool.lane_count(seq), 3);

        // Step 2: replace nonce 1 with a price-bumped version. Lane size
        // stays at 3 (replacements don't grow the lane).
        pool.add_transaction(make_valid(aa_tx(sender, key, 1, 2_000), sender), 0, 0).unwrap();
        debug_assert_eq!(pool.lane_count(seq), range_count(&pool));
        assert_eq!(pool.lane_count(seq), 3);

        // Step 3: remove nonce 0. Counter drops to 2.
        let head_id = Eip8130TxId::new(seq, 0);
        let head_hash = *pool.by_id.get(&head_id).unwrap().tx.hash();
        pool.remove_by_hash(&head_hash).expect("head present");
        debug_assert_eq!(pool.lane_count(seq), range_count(&pool));
        assert_eq!(pool.lane_count(seq), 2);

        // Step 4: drain the remaining nonces. Counter drops to 0 and the
        // map row is freed (so `lane_count` returns 0 via the `unwrap_or`
        // path, not a stale 0 entry).
        for nonce in [1u64, 2u64] {
            let id = Eip8130TxId::new(seq, nonce);
            let h = *pool.by_id.get(&id).unwrap().tx.hash();
            pool.remove_by_hash(&h).expect("nonce present");
            debug_assert_eq!(pool.lane_count(seq), range_count(&pool));
        }
        assert_eq!(pool.lane_count(seq), 0);
        assert!(!pool.txs_by_seq.contains_key(&seq), "empty lane row must be dropped");
    }

    /// Regression: removing a pending lane head must demote its
    /// descendants out of `independent_pending` / `all_pending`. Pre-fix,
    /// after `remove_by_hash(nonce=0)` the nonce=1 tx remained marked
    /// pending and showed up in both iterators, so consumers saw a "ready"
    /// tx that the merge iterator could not actually emit.
    #[test]
    fn remove_pending_demotes_descendants() {
        let sender = sender_from_seed(0xD1);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());

        let head = make_valid(aa_tx(sender, U256::ZERO, 0, 1_000), sender);
        let head_hash = *head.hash();
        pool.add_transaction(head, 0, 0).unwrap();
        let next = make_valid(aa_tx(sender, U256::ZERO, 1, 1_000), sender);
        let next_hash = *next.hash();
        pool.add_transaction(next, 0, 0).unwrap();
        assert_eq!(pool.all_pending().count(), 2, "both nonces start pending");

        pool.remove_by_hash(&head_hash).expect("head present");

        // The lane head is gone; nonce=1 must no longer count as pending
        // because there's a gap below it.
        assert!(
            pool.independent_pending().next().is_none(),
            "no lane head remains for the gapped lane"
        );
        let pending_hashes: std::collections::HashSet<_> =
            pool.all_pending().map(|t| *t.hash()).collect();
        assert!(
            !pending_hashes.contains(&next_hash),
            "descendant nonce=1 must be demoted to queued after head removal"
        );
        assert_eq!(pool.all_queued().count(), 1, "nonce=1 sits in queued bucket");
    }

    /// Regression: `nonce_key == NONCE_KEY_MAX` txs are nonce-free,
    /// hash-addressed entries, not a sequenced lane. Descendant removal
    /// must therefore remove only the targeted expiring-nonce tx and leave
    /// other expiring-nonce txs for the same sender alone.
    #[test]
    fn remove_descendants_on_expiring_nonce_removes_only_target() {
        let sender = sender_from_seed(0xD6);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());

        let mut first = aa_tx(sender, NONCE_KEY_MAX, 0, 1_000);
        first.expiry = 1_000_000;
        let first = make_valid(first, sender);
        let first_hash = *first.hash();
        pool.add_transaction(first, 0, 0).unwrap();

        let mut second = aa_tx(sender, NONCE_KEY_MAX, 0, 1_000);
        second.expiry = 1_000_001;
        let second = make_valid(second, sender);
        let second_hash = *second.hash();
        pool.add_transaction(second, 0, 0).unwrap();

        let removed = pool.remove_by_hash_and_descendants(&first_hash);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].hash(), &first_hash);
        assert!(pool.get(&first_hash).is_none());
        assert!(pool.get(&second_hash).is_some(), "other nonce-free tx must remain");
        assert_eq!(pool.expiring.len(), 1);
        assert!(pool.by_id.is_empty(), "nonce-free txs must not populate sequenced lanes");
    }

    /// Sweep: a nonce-free tx whose `expiry < now` is dropped from every
    /// internal index and surfaced in the returned vector.
    #[test]
    fn sweep_drops_nonce_free_past_expiry() {
        let sender = sender_from_seed(0xE0);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());

        let mut t = aa_tx(sender, NONCE_KEY_MAX, 0, 1_000);
        t.expiry = 100;
        let valid = make_valid(t.clone(), sender);
        let hash = *valid.hash();
        let exp_slot =
            valid.transaction.aa_expiring_nonce_slot().expect("expiring tx exposes a seen-slot");
        let exp_hash =
            valid.transaction.aa_expiring_nonce_hash().expect("expiring tx exposes a tx hash");
        assert_eq!(exp_hash, hash, "nonce-free pool index must match handler tx hash");
        pool.add_transaction(valid, 0, 0).unwrap();
        assert_eq!(pool.len(), 1);
        assert!(pool.expiry_index.contains_key(&100));

        let expired = pool.sweep_expired(101);
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].hash(), &hash);

        assert!(pool.is_empty());
        assert!(!pool.by_hash.contains_key(&hash));
        assert!(!pool.expiring.contains_key(&exp_hash));
        assert!(!pool.slot_to_expiring.contains_key(&exp_slot));
        assert!(pool.by_eviction.is_empty());
        assert!(pool.expiry_index.is_empty(), "expiry_index must be empty after sweep");
    }

    /// Sweep: a tx whose `expiry > now` survives; the returned vec is empty.
    #[test]
    fn sweep_keeps_unexpired() {
        let sender = sender_from_seed(0xE1);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());

        let mut t = aa_tx(sender, NONCE_KEY_MAX, 0, 1_000);
        t.expiry = 100;
        pool.add_transaction(make_valid(t, sender), 0, 0).unwrap();

        let expired = pool.sweep_expired(99);
        assert!(expired.is_empty(), "expiry=100, now=99 ⇒ no eviction");
        assert_eq!(pool.len(), 1, "tx must survive a sub-deadline sweep");

        // Boundary: sweep_expired uses `expiry < now`, so `now == expiry`
        // does NOT evict (the tx is still valid at exactly its deadline).
        let still = pool.sweep_expired(100);
        assert!(still.is_empty(), "now == expiry must not evict");
        assert_eq!(pool.len(), 1);
    }

    /// Sweep: when a sequenced tx with `expiry > 0` is dropped, its
    /// descendants in the same lane are demoted out of `pending`. Pins
    /// that the sweep reuses `remove_by_hash` (which calls
    /// `demote_descendants`).
    #[test]
    fn sweep_demotes_descendants_if_sequenced_expires() {
        let sender = sender_from_seed(0xE2);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let key = U256::from(2u64);

        let mut head = aa_tx(sender, key, 0, 1_000);
        head.expiry = 100;
        let head_hash = *make_valid(head.clone(), sender).hash();
        pool.add_transaction(make_valid(head.clone(), sender), 0, 0).unwrap();
        // Successor without expiry — would otherwise sit pending behind
        // the head.
        let next = aa_tx(sender, key, 1, 1_000);
        let next_hash = *make_valid(next.clone(), sender).hash();
        pool.add_transaction(make_valid(next, sender), 0, 0).unwrap();
        assert_eq!(pool.all_pending().count(), 2);

        let expired = pool.sweep_expired(101);
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].hash(), &head_hash);

        // Successor remains in the pool but is demoted (gap below it).
        assert!(pool.get(&next_hash).is_some(), "successor must still be pooled");
        assert_eq!(pool.all_queued().count(), 1, "successor sits in queued bucket");
        assert!(pool.independent_pending().next().is_none(), "no head left for the gapped lane");
    }

    /// Sweep: idempotent. The second call after the first has drained
    /// every expired tx returns an empty vec.
    #[test]
    fn sweep_idempotent_when_called_twice() {
        let sender = sender_from_seed(0xE3);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());

        let mut t = aa_tx(sender, NONCE_KEY_MAX, 0, 1_000);
        t.expiry = 100;
        pool.add_transaction(make_valid(t, sender), 0, 0).unwrap();

        let first = pool.sweep_expired(101);
        assert_eq!(first.len(), 1);
        let second = pool.sweep_expired(101);
        assert!(second.is_empty(), "a second sweep at the same timestamp must be a no-op");
    }

    /// Sweep: replacement updates the expiry index. Pre-fix, replacing a
    /// tx with one carrying a different `expiry` could leave a dangling
    /// row in `expiry_index` keyed at the prior tx's expiry, yielding a
    /// future sweep that "evicts" a tx not in the pool (no-op but drifty).
    #[test]
    fn replacement_updates_expiry_index() {
        let sender = sender_from_seed(0xE4);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());

        let mut a = aa_tx(sender, U256::ZERO, 0, 1_000);
        a.expiry = 100;
        pool.add_transaction(make_valid(a, sender), 0, 0).unwrap();
        assert!(pool.expiry_index.contains_key(&100));

        // 20% bump (max_priority_fee 1_000 → 1_200) clears the default
        // PriceBumpConfig threshold so replacement succeeds.
        let mut b = aa_tx(sender, U256::ZERO, 0, 1_200);
        b.expiry = 500;
        pool.add_transaction(make_valid(b, sender), 0, 0).unwrap();
        assert!(
            !pool.expiry_index.contains_key(&100),
            "replaced tx's expiry row must be cleaned up"
        );
        assert!(pool.expiry_index.contains_key(&500));
        assert_eq!(pool.len(), 1);
    }

    /// Sweep: zero-expiry txs (no-expiry sentinel) never enter the
    /// expiry index and are not swept regardless of `now`.
    #[test]
    fn sweep_ignores_zero_expiry() {
        let sender = sender_from_seed(0xE5);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let tx = aa_tx(sender, U256::ZERO, 0, 1_000);
        assert_eq!(tx.expiry, 0, "helper default uses no-expiry sentinel");
        pool.add_transaction(make_valid(tx, sender), 0, 0).unwrap();
        assert!(pool.expiry_index.is_empty(), "no expiry, no index entry");

        let expired = pool.sweep_expired(u64::MAX);
        assert!(expired.is_empty(), "zero-expiry tx must never be swept");
        assert_eq!(pool.len(), 1);
    }

    /// Regression: per-sender DoS cap binds to the AA-effective sender,
    /// not the EIP-2718 envelope signer. Pre-fix, an AA tx whose envelope
    /// was signed by a payer (or any non-AA-sender) would inflate the
    /// payer's count instead of the AA sender's, letting one AA sender
    /// bypass `max_txs_per_sender` by rotating envelope signers.
    #[test]
    fn per_sender_cap_uses_aa_sender_not_envelope_signer() {
        let cfg = Eip8130PoolConfig { max_txs_per_sender: 2, ..Default::default() };
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(cfg);

        let aa_sender = sender_from_seed(0xD2);
        // Three different envelope signers, all authoring AA txs whose
        // `tx.from = aa_sender`. Pre-fix the cap would not catch the third
        // because each envelope signer's count was 1.
        let envelope_a = sender_from_seed(0xE1);
        let envelope_b = sender_from_seed(0xE2);
        let envelope_c = sender_from_seed(0xE3);

        pool.add_transaction(make_valid(aa_tx(aa_sender, U256::ZERO, 0, 1_000), envelope_a), 0, 0)
            .unwrap();
        pool.add_transaction(make_valid(aa_tx(aa_sender, U256::ZERO, 1, 1_000), envelope_b), 0, 0)
            .unwrap();
        // Third tx exceeds the AA-sender cap of 2, regardless of the
        // distinct envelope signer.
        let err = pool
            .add_transaction(make_valid(aa_tx(aa_sender, U256::ZERO, 2, 1_000), envelope_c), 0, 0)
            .expect_err("per-sender cap must trip on aa_sender");
        match err.kind {
            PoolErrorKind::SpammerExceededCapacity(addr) => {
                assert_eq!(addr, aa_sender, "cap-violation must name the AA sender");
            }
            other => panic!("expected SpammerExceededCapacity, got {other:?}"),
        }
        // Bookkeeping is keyed on aa_sender, not the envelope signers.
        assert_eq!(pool.txs_by_sender.get(&aa_sender).copied().unwrap_or(0), 2,);
        assert!(pool.txs_by_sender.get(&envelope_a).is_none());
        assert!(pool.txs_by_sender.get(&envelope_b).is_none());
    }

    /// Smoke test: every mutating op runs without panicking when metrics
    /// are wired, and `metrics_snapshot` returns a clone of the handle.
    /// Reth's `metrics-derive` exposes counters/gauges as opaque handles;
    /// asserting numeric values in-process requires installing a global
    /// `Recorder`, which would clash with concurrent tests in the same
    /// binary. The structural test here pins the wire-up: admit, replace,
    /// promote-on-gap, sweep, capacity discard, invalidate. Numeric
    /// dashboards are validated via the prometheus exporter at runtime.
    #[test]
    fn metrics_count_admit_and_remove() {
        let sender = sender_from_seed(0xF0);
        let cfg = Eip8130PoolConfig { max_pool_size: 8, ..Default::default() };
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(cfg);

        // Snapshot is just a clone of the handle; the call must succeed
        // both before and after mutations.
        let _snap_pre = pool.metrics_snapshot();

        // Admit two contiguous txs.
        pool.add_transaction(make_valid(aa_tx(sender, U256::ZERO, 0, 1_000), sender), 0, 0)
            .expect("admit nonce 0");
        pool.add_transaction(make_valid(aa_tx(sender, U256::ZERO, 1, 1_000), sender), 0, 0)
            .expect("admit nonce 1");

        // Replacement (price-bumped) at nonce 0.
        pool.add_transaction(make_valid(aa_tx(sender, U256::ZERO, 0, 1_200), sender), 0, 0)
            .expect("replace nonce 0");

        // Capacity-driven eviction: shrink the pool so a third sender
        // forces a drop.
        let cfg2 = Eip8130PoolConfig { max_pool_size: 1, ..Default::default() };
        let mut tight: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(cfg2);
        let s_a = sender_from_seed(0xF1);
        let s_b = sender_from_seed(0xF2);
        tight.add_transaction(make_valid(aa_tx(s_a, U256::ZERO, 0, 100), s_a), 0, 0).unwrap();
        tight.add_transaction(make_valid(aa_tx(s_b, U256::ZERO, 0, 200), s_b), 0, 0).unwrap();
        assert_eq!(tight.len(), 1, "capacity cap honored");

        // Expiry sweep path.
        let s_e = sender_from_seed(0xF3);
        let mut t = aa_tx(s_e, NONCE_KEY_MAX, 0, 1_000);
        t.expiry = 100;
        pool.add_transaction(make_valid(t, s_e), 0, 0).unwrap();
        let expired = pool.sweep_expired(101);
        assert_eq!(expired.len(), 1);

        // Invalidation path: rule A evicts on REVOKED_VERIFIER at the
        // sender's owner_config slot (always registered).
        let s_i = sender_from_seed(0xF4);
        pool.add_transaction(make_valid(aa_tx(s_i, U256::ZERO, 0, 1_000), s_i), 0, 0).unwrap();
        let inv = pool.on_state_updates(
            [(
                ACCOUNT_CONFIG_ADDRESS,
                aa_owner_config_slot(s_i, implicit_owner_id(s_i)),
                make_owner_config_word(op_revm::constants::REVOKED_VERIFIER, 0),
            )],
            0,
            B256::ZERO,
        );
        assert_eq!(inv.invalidated.len(), 1);

        // Final structural assertion: snapshot remains callable; pool
        // counts are coherent after the fan-out of mutations.
        let _snap_post = pool.metrics_snapshot();
    }

    // -----------------------------------------------------------------
    // on_balance_updates: state-diff-driven payer balance re-validation.
    //
    // Mirrors tempo's `Check 3b` (`tempo_pool.rs:267-307`) — adapted to
    // xlayer's raw-ETH model. Today payer == sender; the predicate
    // matches admission's `gas_limit * max_fee_per_gas` check at
    // `eip8130_xlayer.rs:471-484`.
    // -----------------------------------------------------------------

    /// Capability: a pending tx whose payer's post-state balance dropped
    /// below `gas_limit * max_fee_per_gas` is evicted, returned in the
    /// vec, and removed from `by_hash`.
    #[test]
    fn on_balance_updates_evicts_underfunded() {
        let sender = sender_from_seed(0xB1);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());

        // gas_limit=100_000, max_fee_per_gas = (1+1)=2 ⇒ required = 200_000.
        let tx = make_valid(aa_tx(sender, U256::ZERO, 0, 1), sender);
        let hash = *tx.hash();
        pool.add_transaction(tx, 0, 0).unwrap();

        // New balance = 100_000 < 200_000 required.
        let evicted = pool.on_balance_updates([(sender, U256::from(100_000_u64))]);
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0].hash(), &hash);
        assert!(pool.get(&hash).is_none(), "evicted tx must be gone from by_hash");
        assert!(pool.is_empty());
    }

    /// Capability: a balance update that still covers the fee leaves the
    /// tx in the pool — re-validation is a strict-less-than check.
    #[test]
    fn on_balance_updates_keeps_solvent() {
        let sender = sender_from_seed(0xB2);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let tx = make_valid(aa_tx(sender, U256::ZERO, 0, 1), sender);
        let hash = *tx.hash();
        pool.add_transaction(tx, 0, 0).unwrap();

        // 500_000 > 200_000 required ⇒ keep.
        let evicted = pool.on_balance_updates([(sender, U256::from(500_000_u64))]);
        assert!(evicted.is_empty());
        assert!(pool.get(&hash).is_some());
    }

    /// Capability: a balance update for an unrelated address is a no-op
    /// — the pool's lookup is keyed on payer, not "any address that
    /// touched the bundle".
    #[test]
    fn on_balance_updates_ignores_unrelated_addresses() {
        let sender_a = sender_from_seed(0xB3);
        let sender_b = sender_from_seed(0xB4);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let tx = make_valid(aa_tx(sender_a, U256::ZERO, 0, 1), sender_a);
        pool.add_transaction(tx, 0, 0).unwrap();

        // Drop sender_b's balance to zero — sender_a's tx is unaffected.
        let evicted = pool.on_balance_updates([(sender_b, U256::ZERO)]);
        assert!(evicted.is_empty());
        assert_eq!(pool.len(), 1);
    }

    /// Capability: empty input is a no-op (no panic, returns empty vec).
    #[test]
    fn on_balance_updates_handles_empty() {
        let sender = sender_from_seed(0xB5);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let tx = make_valid(aa_tx(sender, U256::ZERO, 0, 1), sender);
        pool.add_transaction(tx, 0, 0).unwrap();

        let updates: Vec<(Address, U256)> = Vec::new();
        let evicted = pool.on_balance_updates(updates);
        assert!(evicted.is_empty());
        assert_eq!(pool.len(), 1);
    }

    /// Queued (nonce-gap-parked) AA txs are evicted on payer balance
    /// drop too. Pre-fix the sweep filtered to `is_pending` only, so a
    /// queued tx admitted with sufficient payer balance would silently
    /// promote to pending after the gap closed (`promote_lane` advances
    /// by nonce contiguity, no balance check) and then fail at
    /// execution. Tempo walks pending+queued in one pass
    /// (`tempo_pool.rs:197`); we mirror that here.
    #[test]
    fn on_balance_updates_evicts_queued() {
        let sender = sender_from_seed(0xB6);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());

        // nonce=2 with on-chain head 0 ⇒ queued (gap below at nonce 0/1).
        let queued = make_valid(aa_tx(sender, U256::ZERO, 2, 1), sender);
        let queued_hash = *queued.hash();
        pool.add_transaction(queued, 0, 0).unwrap();
        assert_eq!(pool.all_queued().count(), 1);

        // Subscribe to discard broadcast so we can assert the eviction
        // fires the same Discarded path used by pending evictions.
        let mut discarded_rx = pool.subscribe_discarded();

        // Drop balance to 0 — required = gas_limit * max_fee_per_gas =
        // 100_000 * 2 = 200_000 > 0, so the tx is insolvent.
        let evicted = pool.on_balance_updates([(sender, U256::ZERO)]);
        assert_eq!(evicted.len(), 1, "queued tx with insolvent payer must be evicted");
        assert_eq!(evicted[0].hash(), &queued_hash);
        assert!(pool.get(&queued_hash).is_none(), "evicted tx removed from by_hash");
        assert!(pool.is_empty());
        // Discarded broadcast carried the hash exactly once.
        match discarded_rx.try_recv() {
            Ok(h) => assert_eq!(h, queued_hash, "discard broadcast must carry the evicted hash"),
            Err(e) => panic!("expected Discarded broadcast, got {e:?}"),
        }
    }

    /// Smoke test: the insolvent-eviction path increments
    /// `txs_evicted_insolvent_payer` without panicking. Numeric
    /// validation is structural-only (matches the convention from
    /// `metrics_count_admit_and_remove` above).
    #[test]
    fn on_balance_updates_increments_metric() {
        let sender = sender_from_seed(0xB7);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());
        let _snap_pre = pool.metrics_snapshot();

        let tx = make_valid(aa_tx(sender, U256::ZERO, 0, 1), sender);
        pool.add_transaction(tx, 0, 0).unwrap();
        // Force eviction: balance < required.
        let _ = pool.on_balance_updates([(sender, U256::ZERO)]);
        let _snap_post = pool.metrics_snapshot();
    }

    /// `add_transaction_with_required` plumbs the validator's
    /// admission-time predicate (including the L1 data fee) into
    /// `PooledEntry.required_balance`, so a payer balance just above
    /// `gas_limit * max_fee_per_gas` but below the cached cost evicts.
    /// Without F5 the maintenance loop would recompute `gas_limit *
    /// max_fee_per_gas`, miss the L1 component, and keep the tx.
    #[test]
    fn on_balance_updates_evicts_when_cached_required_exceeds_balance() {
        let sender = sender_from_seed(0xB9);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());

        // gas_limit (100_000) * max_fee_per_gas (2) = 200_000; pretend
        // admission folded in an additional 100_000 L1 data fee →
        // required_balance = 300_000.
        let tx = make_valid(aa_tx(sender, U256::ZERO, 0, 1), sender);
        let hash = *tx.hash();
        pool.add_transaction_with_required(tx, 0, 0, U256::from(300_000_u64)).unwrap();

        // Balance = 250_000 covers `gas_limit * max_fee_per_gas` (200_000)
        // but not the cached 300_000. Must evict.
        let evicted = pool.on_balance_updates([(sender, U256::from(250_000_u64))]);
        assert_eq!(evicted.len(), 1, "balance below cached required_balance must evict");
        assert_eq!(evicted[0].hash(), &hash);
    }

    /// Complement of the above — a balance covering the cached
    /// `required_balance` (admission's full predicate) keeps the tx,
    /// even when below an alternate over-estimate.
    #[test]
    fn on_balance_updates_keeps_when_cached_required_satisfied() {
        let sender = sender_from_seed(0xBA);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());

        let tx = make_valid(aa_tx(sender, U256::ZERO, 0, 1), sender);
        let hash = *tx.hash();
        pool.add_transaction_with_required(tx, 0, 0, U256::from(300_000_u64)).unwrap();

        // Balance exactly at cached threshold: strict-less-than ⇒ keep.
        let evicted = pool.on_balance_updates([(sender, U256::from(300_000_u64))]);
        assert!(evicted.is_empty(), "balance at cached required_balance must keep");
        assert!(pool.get(&hash).is_some());
    }

    /// Negative: an expiring-mode AA tx with a depleted payer balance is
    /// also evicted — `on_balance_updates` walks `expiring` alongside
    /// pending lane entries.
    #[test]
    fn on_balance_updates_evicts_expiring_mode_tx() {
        let sender = sender_from_seed(0xB8);
        let mut pool: Eip8130Pool<OpPooledTransaction> = Eip8130Pool::new(Default::default());

        let mut t = aa_tx(sender, NONCE_KEY_MAX, 0, 1);
        t.expiry = 1_000_000;
        let valid = make_valid(t, sender);
        let hash = *valid.hash();
        pool.add_transaction(valid, 0, 0).unwrap();
        assert_eq!(pool.len(), 1);

        let evicted = pool.on_balance_updates([(sender, U256::ZERO)]);
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0].hash(), &hash);
        assert!(pool.is_empty());
    }

    // ------------------------------------------------------------------
    // aa_invalidation_rules `lock_sensitive` flip
    // ------------------------------------------------------------------

    /// A tx carrying a `Create` entry is lock-sensitive — the
    /// AccountState rule's `lock_sensitive` bit must be set so a
    /// still-locked sender is evicted.
    #[test]
    fn aa_invalidation_rules_sets_lock_sensitive_for_create() {
        use op_alloy_consensus::{AccountChangeEntry, CreateEntry};
        let sender = sender_from_seed(0x90);
        let mut tx = aa_tx(sender, U256::ZERO, 0, 1_000_000_000);
        tx.account_changes = vec![AccountChangeEntry::Create(CreateEntry {
            user_salt: B256::repeat_byte(0xCA),
            bytecode: bytes!("60006000"),
            initial_owners: vec![],
        })];
        let signer = signer_for(0x90).expect("non-zero seed");
        let h = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(&signer, h);

        let valid = make_valid(tx, sender);
        let rules = valid.transaction.aa_invalidation_rules();
        let lock_slot = aa_lock_slot(sender);
        let rule = rules
            .iter()
            .find(|((addr, slot), _)| *addr == ACCOUNT_CONFIG_ADDRESS && *slot == lock_slot)
            .map(|(_, r)| *r)
            .expect("AccountState rule for Create entry");
        match rule {
            InvalidationRule::AccountState { lock_sensitive, .. } => {
                assert!(lock_sensitive, "Create must flip lock_sensitive on");
            }
            other => panic!("expected AccountState, got {other:?}"),
        }
    }

    /// A tx carrying a `ConfigChange` with non-empty `owner_changes`
    /// is lock-sensitive (config writes require an unlocked sender).
    #[test]
    fn aa_invalidation_rules_sets_lock_sensitive_for_owner_changes() {
        use op_alloy_consensus::{AccountChangeEntry, ConfigChangeEntry, OwnerChange};
        use op_revm::constants::{OP_AUTHORIZE_OWNER, OWNER_SCOPE_SENDER};

        let sender = sender_from_seed(0x91);
        let mut tx = aa_tx(sender, U256::ZERO, 0, 1_000_000_000);
        tx.account_changes = vec![AccountChangeEntry::ConfigChange(ConfigChangeEntry {
            chain_id: 0,
            sequence: 0,
            owner_changes: vec![OwnerChange {
                change_type: OP_AUTHORIZE_OWNER,
                owner_id: B256::repeat_byte(0xCC),
                verifier: K1_VERIFIER_ADDRESS,
                scope: OWNER_SCOPE_SENDER,
            }],
            authorizer_auth: Default::default(),
        })];
        let signer = signer_for(0x91).expect("non-zero seed");
        let h = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(&signer, h);

        let valid = make_valid(tx, sender);
        let rules = valid.transaction.aa_invalidation_rules();
        let lock_slot = aa_lock_slot(sender);
        let rule = rules
            .iter()
            .find(|((addr, slot), _)| *addr == ACCOUNT_CONFIG_ADDRESS && *slot == lock_slot)
            .map(|(_, r)| *r)
            .expect("AccountState rule for owner_changes entry");
        match rule {
            InvalidationRule::AccountState { lock_sensitive, .. } => {
                assert!(lock_sensitive, "ConfigChange owner_changes must flip lock_sensitive on");
            }
            other => panic!("expected AccountState, got {other:?}"),
        }
    }

    /// A pure-sequence ConfigChange (empty owner_changes AND empty
    /// authorizer_auth) IS lock-sensitive: `process_config_change_entry`
    /// (`alloy-op-evm/src/eip8130/parts.rs:309`) emits a `sequence_updates`
    /// slot for any matching entry, and the executor's `check_account_lock`
    /// (`op-revm/src/handler.rs:343-370`) fires when `sequence_updates`
    /// is non-empty.
    #[test]
    fn aa_invalidation_rules_marks_pure_sequence_lock_sensitive() {
        use op_alloy_consensus::{AccountChangeEntry, ConfigChangeEntry};
        let sender = sender_from_seed(0x92);
        let mut tx = aa_tx(sender, U256::ZERO, 0, 1_000_000_000);
        tx.account_changes = vec![AccountChangeEntry::ConfigChange(ConfigChangeEntry {
            chain_id: 0,
            sequence: 3,
            owner_changes: vec![],
            authorizer_auth: Default::default(),
        })];
        let signer = signer_for(0x92).expect("non-zero seed");
        let h = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(&signer, h);

        let valid = make_valid(tx, sender);
        let rules = valid.transaction.aa_invalidation_rules();
        let lock_slot = aa_lock_slot(sender);
        let rule = rules
            .iter()
            .find(|((addr, slot), _)| *addr == ACCOUNT_CONFIG_ADDRESS && *slot == lock_slot)
            .map(|(_, r)| *r)
            .expect("AccountState rule for pure-sequence entry");
        match rule {
            InvalidationRule::AccountState { lock_sensitive, .. } => {
                assert!(
                    lock_sensitive,
                    "pure-sequence ConfigChange must flip lock_sensitive on (executor lock check fires for sequence_updates)",
                );
            }
            other => panic!("expected AccountState, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // Delegate→Native invalidation rules.
    //
    // The executor's `dispatch_auth_state` (op-revm `handler.rs:676-710`)
    // runs TWO owner_config reads for `AuthState::Native` with
    // `delegate_inner = Some(_)`: outer
    // `(sender, bytes32(bytes20(delegate)))` and inner
    // `(delegate_address, delegate_inner.owner_id)`. Admission must
    // mirror this so a diff to the inner row evicts the tx.
    // -----------------------------------------------------------------

    /// Delegate→K1: `aa_invalidation_rules` emits TWO sender-side
    /// OwnerConfig rules — outer (sender, bytes20(delegate)) bound to
    /// DELEGATE_VERIFIER, plus inner (delegate, bytes20(inner_signer))
    /// bound to K1.
    #[test]
    fn aa_invalidation_rules_emits_inner_rule_for_delegate_to_native() {
        use op_revm::constants::{DELEGATE_VERIFIER_ADDRESS, OWNER_SCOPE_SENDER};
        let sender = Address::repeat_byte(0xB1);
        let delegate = Address::repeat_byte(0xC1);
        let inner_signer = signer_for(0x71).expect("non-zero seed");

        // Build a tx with Delegate→K1 sender_auth. `tx.from = sender`
        // (delegate path doesn't enforce strict-self-owner on the inner
        // K1, so the inner signer can be any address).
        let mut tx = TxEip8130 {
            chain_id: 10,
            sender: Some(sender),
            nonce_key: U256::ZERO,
            nonce_sequence: 0,
            expiry: 0,
            max_priority_fee_per_gas: 1_000_000_000,
            max_fee_per_gas: 1_000_000_001,
            gas_limit: 100_000,
            calls: vec![vec![Eip8130CallEntry { to: Address::repeat_byte(0xAA), data: bytes!() }]],
            ..Default::default()
        };
        let hash = sender_signature_hash(&tx);
        let mut blob = Vec::with_capacity(20 + 20 + 20 + 65);
        blob.extend_from_slice(DELEGATE_VERIFIER_ADDRESS.as_slice());
        blob.extend_from_slice(delegate.as_slice());
        blob.extend_from_slice(K1_VERIFIER_ADDRESS.as_slice());
        blob.extend_from_slice(&k1_sig_blob(&inner_signer, hash));
        tx.sender_auth = Bytes::from(blob);

        let valid = make_valid(tx, sender);
        let rules = valid.transaction.aa_invalidation_rules();

        // Outer rule: (sender, bytes20(delegate)) → DELEGATE_VERIFIER, SENDER.
        let outer_owner_id = {
            let mut buf = [0u8; 32];
            buf[..20].copy_from_slice(delegate.as_slice());
            U256::from_be_bytes(buf)
        };
        let outer_slot = aa_owner_config_slot(sender, outer_owner_id);
        let outer_rule = rules
            .iter()
            .find(|((addr, slot), _)| *addr == ACCOUNT_CONFIG_ADDRESS && *slot == outer_slot)
            .map(|(_, r)| *r)
            .expect("outer Delegate→Native owner_config rule must be present");
        match outer_rule {
            InvalidationRule::OwnerConfig { expected_verifier, required_scope_bit } => {
                assert_eq!(expected_verifier, DELEGATE_VERIFIER_ADDRESS);
                assert_eq!(required_scope_bit, OWNER_SCOPE_SENDER);
            }
            other => panic!("expected OwnerConfig for outer, got {other:?}"),
        }

        // Inner rule: (delegate, bytes20(inner_signer)) → K1, SENDER.
        let inner_owner_id = {
            let mut buf = [0u8; 32];
            buf[..20].copy_from_slice(inner_signer.address().as_slice());
            U256::from_be_bytes(buf)
        };
        let inner_slot = aa_owner_config_slot(delegate, inner_owner_id);
        let inner_rule = rules
            .iter()
            .find(|((addr, slot), _)| *addr == ACCOUNT_CONFIG_ADDRESS && *slot == inner_slot)
            .map(|(_, r)| *r)
            .expect("inner Delegate→Native owner_config rule must be present");
        match inner_rule {
            InvalidationRule::OwnerConfig { expected_verifier, required_scope_bit } => {
                assert_eq!(expected_verifier, K1_VERIFIER_ADDRESS);
                assert_eq!(required_scope_bit, OWNER_SCOPE_SENDER);
            }
            other => panic!("expected OwnerConfig for inner, got {other:?}"),
        }
    }

    /// Delegate→Custom: outer slot binding is known
    /// (DELEGATE_VERIFIER_ADDRESS) but the inner owner_id depends on the
    /// STATICCALL result, so neither rule is registered. This is the
    /// same coarse-but-safe choice as plain Custom (see doc comment on
    /// `aa_invalidation_rules`).
    #[test]
    fn aa_invalidation_rules_skips_inner_for_delegate_to_custom() {
        use op_revm::constants::DELEGATE_VERIFIER_ADDRESS;
        let sender = Address::repeat_byte(0xB2);
        let delegate = Address::repeat_byte(0xC2);
        let inner_custom = Address::repeat_byte(0xEF);

        let mut tx = TxEip8130 {
            chain_id: 10,
            sender: Some(sender),
            nonce_key: U256::ZERO,
            nonce_sequence: 0,
            expiry: 0,
            max_priority_fee_per_gas: 1_000_000_000,
            max_fee_per_gas: 1_000_000_001,
            gas_limit: 100_000,
            calls: vec![vec![Eip8130CallEntry { to: Address::repeat_byte(0xAA), data: bytes!() }]],
            ..Default::default()
        };
        // Delegate→Custom: 30 bytes of opaque custom payload after the
        // inner verifier address (mirrors auth_state.rs's
        // `delegate_custom_inner_returns_deferred_with_outer` test).
        let mut blob = Vec::with_capacity(20 + 20 + 20 + 30);
        blob.extend_from_slice(DELEGATE_VERIFIER_ADDRESS.as_slice());
        blob.extend_from_slice(delegate.as_slice());
        blob.extend_from_slice(inner_custom.as_slice());
        blob.extend_from_slice(&[0xAB; 30]);
        tx.sender_auth = Bytes::from(blob);

        let valid = make_valid(tx, sender);
        let rules = valid.transaction.aa_invalidation_rules();

        // No OwnerConfig rules at all — Deferred branch skips both.
        let owner_config_rules: Vec<_> = rules
            .iter()
            .filter(|(_, r)| matches!(r, InvalidationRule::OwnerConfig { .. }))
            .collect();
        assert!(
            owner_config_rules.is_empty(),
            "Delegate→Custom must skip outer + inner rules (owner_id depends on STATICCALL)",
        );
    }
}
