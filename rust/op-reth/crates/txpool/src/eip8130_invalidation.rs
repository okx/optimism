//! State-diff based invalidation for EIP-8130 Account Abstraction transactions.
//!
//! Tracks which storage slots each pending AA transaction depends on, so that
//! when a block's state diff reports changed slots, the affected transactions
//! can be efficiently identified and evicted from the mempool.
//!
//! Ported 1:1 from base/crates/txpool/src/eip8130_invalidation.rs (lines 1-254).
//! The maintenance task (`maintain_eip8130_invalidation`, base lines 256-672)
//! is deferred — it depends on a dual-pool architecture (`Eip8130Pool`) that we
//! have not yet ported. Wiring this index into op-reth's standard pool eviction
//! flow is the next milestone for task #6.

use std::collections::{HashMap, HashSet};

use alloy_primitives::{Address, B256, U256};
use op_alloy_consensus::transaction::eip8130::{
    ACCOUNT_CONFIG_ADDRESS, AccountChangeEntry, NONCE_MANAGER_ADDRESS, TxEip8130, lock_slot,
    nonce_slot, owner_config_slot, sequence_base_slot,
};

/// A (contract address, storage slot) pair that an AA transaction depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InvalidationKey {
    /// The contract whose storage is being watched.
    pub address: Address,
    /// The specific storage slot within that contract.
    pub slot: B256,
}

/// Index that maps invalidation keys to the set of transaction hashes that
/// depend on them. Also tracks per-payer pending counts for sponsored AA txs.
#[derive(Debug, Default)]
pub struct Eip8130InvalidationIndex {
    key_to_txs: HashMap<InvalidationKey, HashSet<B256>>,
    tx_to_keys: HashMap<B256, HashSet<InvalidationKey>>,
    tx_to_payer: HashMap<B256, Address>,
    payer_counts: HashMap<Address, usize>,
}

impl Eip8130InvalidationIndex {
    /// Inserts a transaction and its invalidation keys into the index.
    ///
    /// If `payer` is `Some`, tracks the payer for pending-count enforcement.
    /// Pass `Some(payer)` for sponsored AA txs where payer != sender.
    pub fn insert(
        &mut self,
        tx_hash: B256,
        keys: HashSet<InvalidationKey>,
        payer: Option<Address>,
    ) {
        for key in &keys {
            self.key_to_txs.entry(*key).or_default().insert(tx_hash);
        }
        self.tx_to_keys.insert(tx_hash, keys);
        if let Some(addr) = payer {
            self.tx_to_payer.insert(tx_hash, addr);
            *self.payer_counts.entry(addr).or_default() += 1;
        }
    }

    /// Returns the set of transaction hashes affected by the given key.
    pub fn lookup(&self, key: &InvalidationKey) -> Option<&HashSet<B256>> {
        self.key_to_txs.get(key)
    }

    /// Removes a transaction from the index, cleaning up all associated keys.
    pub fn remove(&mut self, tx_hash: &B256) {
        if let Some(keys) = self.tx_to_keys.remove(tx_hash) {
            for key in &keys {
                if let Some(txs) = self.key_to_txs.get_mut(key) {
                    txs.remove(tx_hash);
                    if txs.is_empty() {
                        self.key_to_txs.remove(key);
                    }
                }
            }
        }
        if let Some(payer) = self.tx_to_payer.remove(tx_hash) {
            if let Some(count) = self.payer_counts.get_mut(&payer) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.payer_counts.remove(&payer);
                }
            }
        }
    }

    /// Returns all transaction hashes invalidated by any of the given keys.
    pub fn invalidated_by(&self, keys: &[InvalidationKey]) -> HashSet<B256> {
        let mut result = HashSet::new();
        for key in keys {
            if let Some(txs) = self.key_to_txs.get(key) {
                result.extend(txs);
            }
        }
        result
    }

    /// Returns the number of pending sponsored txs for a given payer.
    pub fn payer_pending_count(&self, payer: &Address) -> usize {
        self.payer_counts.get(payer).copied().unwrap_or(0)
    }

    /// Returns the number of tracked transactions.
    pub fn len(&self) -> usize {
        self.tx_to_keys.len()
    }

    /// Returns true if there are no tracked transactions.
    pub fn is_empty(&self) -> bool {
        self.tx_to_keys.is_empty()
    }

    /// Returns all tracked transaction hashes.
    pub fn tracked_tx_hashes(&self) -> impl Iterator<Item = &B256> {
        self.tx_to_keys.keys()
    }

    /// Removes all transactions whose hashes are NOT in the given live set.
    ///
    /// Returns the number of stale entries pruned.
    pub fn prune_stale(&mut self, live: &HashSet<B256>) -> usize {
        let stale: Vec<B256> =
            self.tx_to_keys.keys().filter(|hash| !live.contains(*hash)).copied().collect();

        let count = stale.len();
        for hash in stale {
            self.remove(&hash);
        }
        count
    }
}

/// Computes the set of storage slots that this AA transaction depends on.
///
/// A state change to any of these slots should trigger re-validation or eviction.
///
/// `resolved_sender` is the effective sender address (ecrecovered for EOA mode,
/// `tx.from` for configured mode). This must be passed because `tx.from` is
/// `Address::ZERO` in EOA mode.
///
/// When available, pass the resolved `sender_owner_id` and `payer_owner_id`
/// from validation to track the exact owner config slots. Falls back to a
/// hash-based proxy when `None`.
pub fn compute_invalidation_keys(
    tx: &TxEip8130,
    resolved_sender: Address,
    resolved_sender_owner_id: Option<B256>,
    resolved_payer_owner_id: Option<B256>,
) -> HashSet<InvalidationKey> {
    let mut keys = HashSet::new();
    let sender = resolved_sender;

    // 1. Nonce slot — the sender's 2D nonce at (sender, nonce_key)
    let nonce_key_slot = nonce_slot(sender, tx.nonce_key);
    keys.insert(InvalidationKey { address: NONCE_MANAGER_ADDRESS, slot: nonce_key_slot });

    // 2. Sender owner config slot — use the resolved owner_id if available
    //    (from validation), otherwise fall back to keccak256(sender_auth) as
    //    a proxy. The resolved owner_id gives us the exact storage slot.
    if !tx.sender_auth.is_empty() {
        let owner_id = resolved_sender_owner_id
            .unwrap_or_else(|| alloy_primitives::keccak256(&tx.sender_auth));
        let config_slot = owner_config_slot(sender, owner_id);
        keys.insert(InvalidationKey { address: ACCOUNT_CONFIG_ADDRESS, slot: config_slot });
    }

    // 3. Payer owner config — if there's a separate payer, their owner
    //    authorization can be revoked, invalidating the tx.
    if let Some(payer) = tx.payer.filter(|payer| *payer != sender) {
        if tx.payer_auth.is_empty() {
            // `eth_estimateGas` may omit payer auth; no payer owner slot dependency yet.
        } else {
            let payer_owner_id = resolved_payer_owner_id
                .unwrap_or_else(|| alloy_primitives::keccak256(&tx.payer_auth));
            let payer_config_slot = owner_config_slot(payer, payer_owner_id);
            keys.insert(InvalidationKey {
                address: ACCOUNT_CONFIG_ADDRESS,
                slot: payer_config_slot,
            });
        }
    }

    // 4. Account changes — each create entry depends on the target address having
    //    no code, and each config change depends on the sender's lock state and
    //    change sequence.
    for change in &tx.account_changes {
        match change {
            AccountChangeEntry::Create(create) => {
                let deployer_hash = alloy_primitives::keccak256(
                    [
                        sender.as_slice(),
                        create.user_salt.as_slice(),
                        &alloy_primitives::keccak256(&create.bytecode).0,
                    ]
                    .concat(),
                );
                keys.insert(InvalidationKey { address: sender, slot: deployer_hash });
            }
            AccountChangeEntry::ConfigChange(_cc) => {
                let lock_key_slot = lock_slot(sender);
                keys.insert(InvalidationKey {
                    address: ACCOUNT_CONFIG_ADDRESS,
                    slot: lock_key_slot,
                });

                // Both multichain and local sequences are packed into a single
                // slot, so watching the base slot covers both chain_id variants.
                // This also covers authorizer invalidation: if the authorizer
                // is revoked (a config change on the same account), the
                // sequence bumps and this slot changes.
                let seq_slot = sequence_base_slot(sender);
                keys.insert(InvalidationKey { address: ACCOUNT_CONFIG_ADDRESS, slot: seq_slot });
            }
            AccountChangeEntry::Delegation(_) => {}
        }
    }

    // Suppress unused warning for U256 when no AA storage slot variants reference it.
    let _ = U256::ZERO;
    keys
}

/// Given a set of FAL entries (touched storage slots from a block), finds
/// all pending AA transactions that should be invalidated and returns their
/// hashes.
pub fn process_fal(fal: &[(Address, B256)], index: &Eip8130InvalidationIndex) -> HashSet<B256> {
    let mut result = HashSet::new();
    for &(address, slot) in fal {
        let key = InvalidationKey { address, slot };
        if let Some(txs) = index.lookup(&key) {
            result.extend(txs);
        }
    }
    result
}

/// How often (in blocks) the stale-entry pruning pass runs.
const PRUNE_INTERVAL_BLOCKS: u64 = 16;

/// Maintenance loop that evicts EIP-8130 transactions from the pool when the
/// storage slots they depend on change.
///
/// Listens to [`CanonStateNotification`](reth_provider::CanonStateNotification)
/// events and, for each committed block, extracts storage changes for the two
/// AA system contracts ([`ACCOUNT_CONFIG_ADDRESS`] and [`NONCE_MANAGER_ADDRESS`]).
/// Matching transactions are removed from both the pool and the shared
/// invalidation index.
///
/// When `NONCE_MANAGER_ADDRESS` storage slots change, the 2D nonce pool is
/// also updated: the affected sequence lanes advance their `next_nonce` and
/// stale transactions are pruned.
///
/// Ported 1:1 from base/crates/txpool/src/eip8130_invalidation.rs lines 256-424.
/// The `eip8130_pool` parameter (base's dedicated AA `Eip8130Pool`) is not yet
/// ported — its accessors (`invalidate_tiers_for_lock_slots`, `seq_id_for_slot`,
/// `update_sequence_nonce`, `sweep_expired`, `remove_transactions`, `contains`)
/// are stubbed out in this port via `// EIP8130_POOL_TODO:` comments. The
/// surrounding flow (FAL extraction, process_fal → pool.remove_transactions,
/// PRUNE_INTERVAL_BLOCKS sweep) is preserved verbatim so the diff against base
/// remains a mechanical reviewer-friendly patch.
pub async fn maintain_eip8130_invalidation<P, N>(
    pool: P,
    // EIP8130_POOL_TODO: `eip8130_pool: Arc<Eip8130Pool<T>>` parameter dropped
    // until the dual-pool architecture lands. base lines 273, 278.
    mut events: tokio_stream::wrappers::BroadcastStream<reth_provider::CanonStateNotification<N>>,
    index: std::sync::Arc<parking_lot::RwLock<Eip8130InvalidationIndex>>,
) where
    P: reth_transaction_pool::TransactionPool + 'static,
    N: reth_node_api::NodePrimitives,
{
    use alloy_consensus::BlockHeader;
    use futures::StreamExt;
    use tracing::{debug, trace, warn};

    let mut blocks_since_prune: u64 = 0;

    loop {
        let notification = match events.next().await {
            Some(Ok(notification)) => notification,
            Some(Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n))) => {
                warn!(
                    missed = n,
                    "canon state stream lagged, some blocks were not checked for AA invalidation"
                );
                continue;
            }
            None => break,
        };

        blocks_since_prune += 1;

        let tip = notification.tip();
        let _block_timestamp = tip.timestamp(); // EIP8130_POOL_TODO: used by sweep_expired

        let committed = notification.committed();
        let execution_outcome = committed.execution_outcome();

        let mut touched: Vec<(Address, B256)> = Vec::new();
        let mut _nonce_slot_changes: Vec<(B256, U256)> = Vec::new();
        let mut _config_slots: Vec<B256> = Vec::new();

        for (addr, acc) in execution_outcome.bundle_accounts_iter() {
            if addr == NONCE_MANAGER_ADDRESS {
                for (key, slot) in acc.storage.iter() {
                    let slot_key = B256::from(*key);
                    touched.push((addr, slot_key));
                    _nonce_slot_changes.push((slot_key, slot.present_value));
                }
            } else if addr == ACCOUNT_CONFIG_ADDRESS {
                for (key, _slot) in acc.storage.iter() {
                    let slot_key = B256::from(*key);
                    touched.push((addr, slot_key));
                    _config_slots.push(slot_key);
                }
            }
        }

        // EIP8130_POOL_TODO: invalidate cached throughput tiers when lock slots change.
        // base lines 324-334:
        //     if !config_slots.is_empty() {
        //         let n = eip8130_pool.invalidate_tiers_for_lock_slots(&config_slots);
        //         ...
        //     }

        // EIP8130_POOL_TODO: update 2D nonce pool when nonce storage slots change.
        // base lines 336-356:
        //     if !nonce_slot_changes.is_empty() {
        //         for (slot_key, new_value) in &nonce_slot_changes {
        //             if let Some(seq_id) = eip8130_pool.seq_id_for_slot(slot_key) {
        //                 let new_nonce: u64 = new_value.try_into().unwrap_or(u64::MAX);
        //                 let pruned = eip8130_pool.update_sequence_nonce(&seq_id, new_nonce);
        //                 ...
        //             }
        //         }
        //     }

        // Skip invalidation index lookup when the index is empty.
        if index.read().is_empty() && touched.is_empty() {
            continue;
        }

        if !touched.is_empty() {
            let invalidated = {
                let mut idx = index.write();
                let invalidated = process_fal(&touched, &idx);
                for tx_hash in &invalidated {
                    idx.remove(tx_hash);
                }
                invalidated
            };

            if invalidated.is_empty() {
                trace!(
                    touched_slots = touched.len(),
                    "AA storage changes did not match any pending transactions"
                );
            } else {
                debug!(
                    removal_reason = "state_invalidation",
                    count = invalidated.len(),
                    touched_slots = touched.len(),
                    "removed invalidated AA transactions from mempool"
                );
                let _hash_vec: Vec<B256> = invalidated.iter().copied().collect();
                // EIP8130_POOL_TODO: eip8130_pool.remove_transactions(&_hash_vec);
                pool.remove_transactions(invalidated.into_iter().collect());
            }
        }

        // EIP8130_POOL_TODO: Sweep transactions whose `expiry` timestamp has passed.
        // base lines 391-401:
        //     let expired = eip8130_pool.sweep_expired(block_timestamp);
        //     if !expired.is_empty() {
        //         pool.remove_transactions(expired);
        //     }

        // Periodically prune stale index entries for transactions the pool
        // has already dropped (replaced, capacity eviction, expiry).
        if blocks_since_prune >= PRUNE_INTERVAL_BLOCKS {
            blocks_since_prune = 0;

            let idx_guard = index.read();
            if !idx_guard.is_empty() {
                let live: HashSet<B256> = idx_guard
                    .tracked_tx_hashes()
                    // EIP8130_POOL_TODO: || eip8130_pool.contains(hash) (base line 412)
                    .filter(|hash| pool.get(hash).is_some())
                    .copied()
                    .collect();
                drop(idx_guard);

                let pruned = index.write().prune_stale(&live);
                if pruned > 0 {
                    debug!(pruned, "pruned stale AA invalidation index entries");
                }
            }
        }
    }
}
