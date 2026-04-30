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
