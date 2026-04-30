//! Mempool admission validation for EIP-8130 (XLayer AA) transactions.
//!
//! Replicates the structural and minimal-state checks the consensus handler runs in
//! [`op_revm::handler::OpHandler::validate_env`] and the early portion of
//! [`validate_against_state_and_deduct_caller`], so that malformed AA transactions are
//! rejected before they reach the standard pool's pending queue.
//!
//! The MVP intentionally defers the following spec checks; each is annotated with a
//! `#TODO(xlayer-eip8130)` at the call site:
//!
//! - sender ecrecover for `from == None` (EOA mode);
//! - native verifier dispatch (K1 / P256 / WebAuthn / Delegate);
//! - custom verifier `STATICCALL` and admission policy;
//! - payer authentication, payer balance, sponsor flow;
//! - account lock check, AccountConfiguration deployment check, owner-config validation;
//! - nonce-free replay seen-set check.
//!
//! These will be added in subsequent slices; the structural rejections enforced here are
//! sufficient to prevent the most common spec-violating txs from polluting the pool.

use alloy_primitives::{Address, B256, U256};
use op_alloy_consensus::{AccountChangeEntry, TxEip8130, sender_signature_hash};
use op_revm::{
    handler::{NONCE_FREE_MAX_EXPIRY_WINDOW, NONCE_KEY_MAX, aa_expiring_seen_slot},
    precompiles_xlayer::NONCE_MANAGER_ADDRESS,
};
use reth_storage_api::{StateProvider, StateProviderFactory};
use reth_transaction_pool::error::PoolTransactionError;
use std::any::Any;

/// Maximum encoded EIP-2718 size for an AA transaction at mempool ingress.
///
/// Mirrors `MAX_AA_TX_ENCODED_BYTES` used by reference implementations: large enough to
/// admit realistic flows (sponsored swaps, batch ops) but bounded so unbounded RLP cannot
/// exhaust pool memory through structural fan-out.
pub const MAX_AA_TX_ENCODED_BYTES: usize = 256 * 1024;

/// Successful AA validation outcome consumed by the pool admission path.
///
/// Mirrors the relevant subset of `TransactionValidationOutcome::Valid`; the validator
/// adapts these into the standard pool's `Valid { state_nonce, balance, .. }` shape.
#[derive(Debug, Clone)]
pub struct Eip8130ValidationOutcome {
    /// Effective sender (always `tx.from` in the MVP — EOA recovery is deferred).
    pub sender: Address,
    /// Effective payer (always `sender` in the MVP — sponsored payer is deferred).
    pub payer: Address,
    /// On-chain nonce sequence read from `NONCE_MANAGER`. For nonce-free txs (`nonce_key
    /// == NONCE_KEY_MAX`) this is `0` since no per-account sequence exists.
    pub state_nonce: u64,
    /// 2D nonce key from the transaction (used downstream for routing decisions).
    pub nonce_key: U256,
    /// Sender balance read at validation time. The validator uses this for the upstream
    /// `Valid { balance, .. }` field.
    pub balance: U256,
}

/// Errors emitted by [`validate_eip8130_transaction`]. Each variant maps directly to a spec
/// MUST or to a structural bound enforced consistently with the handler.
#[derive(Debug, thiserror::Error)]
pub enum Eip8130ValidationError {
    /// Encoded transaction exceeds the mempool ingress cap.
    #[error("EIP-8130 transaction too large: {size} bytes (limit {limit})")]
    TxTooLarge {
        /// Encoded transaction size in bytes.
        size: usize,
        /// Maximum allowed.
        limit: usize,
    },
    /// `chain_id` does not match the configured network.
    ///
    /// The AA branch in the consensus handler short-circuits the mainnet `validate_env`,
    /// so this check must be re-asserted locally in the mempool — see xlayer-aa.md note
    /// "AA branch bypassed `chain_id` check" (2026-04-21).
    #[error("EIP-8130 chain_id mismatch: expected {expected}, got {got}")]
    ChainIdMismatch {
        /// Network chain id.
        expected: u64,
        /// Transaction's chain id.
        got: u64,
    },
    /// `expiry` is non-zero and lies in the past.
    #[error("EIP-8130 transaction expired (expiry {expiry}, current {current})")]
    Expired {
        /// `tx.expiry`.
        expiry: u64,
        /// Block timestamp at validation time.
        current: u64,
    },
    /// More than [`op_revm::constants::MAX_CALLS_PER_TX`] calls across all phases.
    #[error("EIP-8130 transaction has {count} calls, exceeds limit {limit}")]
    TooManyCalls {
        /// Total calls across phases.
        count: usize,
        /// Maximum allowed.
        limit: usize,
    },
    /// More than [`op_revm::constants::MAX_ACCOUNT_CHANGES_PER_TX`] account-change entries.
    #[error("EIP-8130 transaction has {count} account changes, exceeds limit {limit}")]
    TooManyAccountChanges {
        /// Account-change entry count.
        count: usize,
        /// Maximum allowed.
        limit: usize,
    },
    /// More than one `Create` entry in `account_changes` (spec MUST).
    #[error("EIP-8130 transaction has more than one create entry")]
    MultipleCreateEntries,
    /// Nonce-free tx (`nonce_key == MAX`) without `expiry > 0` (spec MUST — replay
    /// protection requires a non-zero expiry window).
    #[error("EIP-8130 nonce-free transaction requires non-zero expiry")]
    NonceFreeMissingExpiry,
    /// Nonce-free tx with `nonce_sequence != 0` (spec MUST — the `nonce_sequence` field
    /// is unused in nonce-free mode and MUST be zero).
    #[error("EIP-8130 nonce-free transaction requires nonce_sequence == 0, got {got}")]
    NonceFreeNonZeroSequence {
        /// `tx.nonce_sequence`.
        got: u64,
    },
    /// Nonce-free tx whose expiry exceeds the allowed window
    /// (`block_timestamp + NONCE_FREE_MAX_EXPIRY_WINDOW`). The on-chain replay ring
    /// only tolerates expiries within a bounded forward window so that the seen-set
    /// memory cost stays bounded.
    #[error("EIP-8130 nonce-free expiry too far: {expiry} (max {max_allowed})")]
    NonceFreeExpiryTooFar {
        /// `tx.expiry`.
        expiry: u64,
        /// `block_timestamp + NONCE_FREE_MAX_EXPIRY_WINDOW`.
        max_allowed: u64,
    },
    /// Nonce-free tx whose `sender_signature_hash` is already recorded in the
    /// NONCE_MANAGER's expiring-seen ring with an expiry beyond the current block.
    /// Replaying the same nonce-free tx within its window is rejected up-front instead
    /// of letting it consume gas at execution time.
    #[error("EIP-8130 nonce-free transaction already seen (replay)")]
    NonceFreeReplay,
    /// Payer balance does not cover `gas_limit * max_fee_per_gas`.
    ///
    /// AA-aware balance check (mirrors the gas-based balance assertion in base's
    /// `validate_eip8130_transaction`). The L1-data-fee check in the validator is an
    /// additional guard layered on top, gated by `requires_l1_data_gas_fee()`.
    /// Intrinsic-gas accounting is intentionally absent here: until the xlayer-aa
    /// gas-schedule resolver lands, the only universally available bound is
    /// `gas_limit * max_fee_per_gas`.
    #[error("EIP-8130 insufficient payer balance: required {required}, available {available}")]
    InsufficientBalance {
        /// `gas_limit * max_fee_per_gas`.
        required: U256,
        /// On-chain payer balance.
        available: U256,
    },
    /// AA-mode `from == None` (EOA recovery mode) is not yet supported by the mempool.
    #[error("EIP-8130 EOA-recovery mode not yet supported in mempool")]
    EoaRecoveryNotSupported,
    /// Sponsored payer (`payer != None`) is not yet supported by the mempool.
    #[error("EIP-8130 sponsored payer not yet supported in mempool")]
    SponsoredPayerNotSupported,
    /// Non-empty `account_changes` is not yet supported by the mempool path.
    #[error("EIP-8130 account_changes not yet supported in mempool")]
    AccountChangesNotSupported,
    /// On-chain `NONCE_MANAGER` sequence does not match `nonce_sequence` for the lane.
    #[error("EIP-8130 nonce mismatch: on-chain {expected}, tx {got}")]
    NonceMismatch {
        /// On-chain sequence at `aa_nonce_slot(sender, nonce_key)`.
        expected: u64,
        /// `tx.nonce_sequence`.
        got: u64,
    },
    /// State-provider read failure surfaced from the underlying client.
    #[error("EIP-8130 state read failed: {0}")]
    StateError(String),
}

impl PoolTransactionError for Eip8130ValidationError {
    fn is_bad_transaction(&self) -> bool {
        match self {
            // Spec MUSTs and structural caps — sender produced a malformed tx; peer
            // penalization is appropriate.
            Self::TxTooLarge { .. }
            | Self::ChainIdMismatch { .. }
            | Self::TooManyCalls { .. }
            | Self::TooManyAccountChanges { .. }
            | Self::MultipleCreateEntries
            | Self::NonceFreeMissingExpiry
            | Self::NonceFreeNonZeroSequence { .. }
            | Self::NonceFreeExpiryTooFar { .. } => true,
            // Time-window / state-disagreement / replay / unsupported-but-well-formed:
            // not a peer protocol violation. Replay-seen and balance-shortfall in
            // particular can be transient (the on-chain seen ring rotates, balance can
            // be topped up) so we don't penalize peers for forwarding them.
            Self::Expired { .. }
            | Self::NonceMismatch { .. }
            | Self::NonceFreeReplay
            | Self::InsufficientBalance { .. }
            | Self::EoaRecoveryNotSupported
            | Self::SponsoredPayerNotSupported
            | Self::AccountChangesNotSupported
            | Self::StateError(_) => false,
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Validates an EIP-8130 transaction for mempool admission.
///
/// Step numbering mirrors `base/crates/txpool/src/eip8130_validate.rs::validate_eip8130_transaction`
/// so a side-by-side review can confirm parity at each spec stage. Steps marked
/// `[DEFERRED]` are intentionally not implemented yet; each carries a one-line gate
/// stating the prerequisite. The cluster of `*_NotSupported` rejections (step 3 / 10 /
/// 6+7+7b combined) fires before opening state to fail fast on shapes we cannot yet
/// validate, identical in effect to base's per-step branches.
///
/// `encoded_len` is supplied by the caller to avoid a second envelope encoding pass; the
/// validator already holds the encoded form.
pub fn validate_eip8130_transaction<Client>(
    tx: &TxEip8130,
    encoded_len: usize,
    block_timestamp: u64,
    chain_id: u64,
    client: &Client,
) -> Result<Eip8130ValidationOutcome, Eip8130ValidationError>
where
    Client: StateProviderFactory,
{
    // Step 0: encoded-size guard (mempool ingress; not in the EIP itself, present in
    // base to bound RLP fan-out memory cost).
    if encoded_len > MAX_AA_TX_ENCODED_BYTES {
        return Err(Eip8130ValidationError::TxTooLarge {
            size: encoded_len,
            limit: MAX_AA_TX_ENCODED_BYTES,
        });
    }

    // Step 1: structural validation — call count, account-change unit count, ≤1 create
    // entry, nonce-free MUSTs (`expiry > 0`, `nonce_sequence == 0`). The nonce-free
    // MUSTs must come *before* the step-2 timeline check; otherwise `expiry == 0`
    // would only fail by coincidence of `block_ts > 0`. See xlayer-aa.md
    // "Missing structural checks for `nonce_key == NONCE_KEY_MAX`" (2026-04-21).
    let total_calls: usize = tx.calls.iter().map(Vec::len).sum();
    if total_calls > op_revm::constants::MAX_CALLS_PER_TX {
        return Err(Eip8130ValidationError::TooManyCalls {
            count: total_calls,
            limit: op_revm::constants::MAX_CALLS_PER_TX,
        });
    }
    let account_change_units = tx.account_changes.len();
    if account_change_units > op_revm::constants::MAX_ACCOUNT_CHANGES_PER_TX {
        return Err(Eip8130ValidationError::TooManyAccountChanges {
            count: account_change_units,
            limit: op_revm::constants::MAX_ACCOUNT_CHANGES_PER_TX,
        });
    }
    let create_count =
        tx.account_changes.iter().filter(|e| matches!(e, AccountChangeEntry::Create(_))).count();
    if create_count > 1 {
        return Err(Eip8130ValidationError::MultipleCreateEntries);
    }
    if tx.nonce_key == NONCE_KEY_MAX {
        if tx.expiry == 0 {
            return Err(Eip8130ValidationError::NonceFreeMissingExpiry);
        }
        if tx.nonce_sequence != 0 {
            return Err(Eip8130ValidationError::NonceFreeNonZeroSequence {
                got: tx.nonce_sequence,
            });
        }
    }

    // Step 1b: chain_id match — re-asserted locally because the AA branch
    // short-circuits the mainnet validate_env. See xlayer-aa.md "AA branch bypassed
    // chain_id check" (2026-04-21).
    if tx.chain_id != chain_id {
        return Err(Eip8130ValidationError::ChainIdMismatch { expected: chain_id, got: tx.chain_id });
    }

    // Step 2: expiry timeline. `expiry == 0` is the "no expiry" sentinel; only
    // non-zero expiries are subject to the time-window check.
    if tx.expiry != 0 && block_timestamp > tx.expiry {
        return Err(Eip8130ValidationError::Expired {
            expiry: tx.expiry,
            current: block_timestamp,
        });
    }

    // Step 3: resolve sender.
    // [DEFERRED] EOA recovery (`from == None`): requires native sender_auth parsing
    // and ecrecover; the parse helper from base (`parse_sender_auth`) is not yet
    // ported. For now only explicit-from txs are admitted.
    let sender = tx.from.ok_or(Eip8130ValidationError::EoaRecoveryNotSupported)?;

    // Step 10 (early reject): payer resolution.
    // [DEFERRED] sponsored payer (`tx.payer.is_some()`): requires payer-auth parsing
    // and verification + sponsor balance check. Self-pay is the only admitted shape
    // until those land.
    if tx.payer.is_some() {
        return Err(Eip8130ValidationError::SponsoredPayerNotSupported);
    }
    let payer = sender;

    // Steps 6 / 7 / 7b (early reject): lock check, config-op count + sequence,
    // authorizer-chain validation.
    // [DEFERRED] all gated on `account_changes` admission. Each requires distinct
    // state-validation slices (lock state read + parse, config sequence read,
    // authorizer-chain pending-owner overlay) plus the `parse_sender_auth` helper.
    if !tx.account_changes.is_empty() {
        return Err(Eip8130ValidationError::AccountChangesNotSupported);
    }

    // Step 4: open state provider.
    let state =
        client.latest().map_err(|e| Eip8130ValidationError::StateError(e.to_string()))?;

    // Step 5: nonce validation.
    let state_nonce = if tx.nonce_key == NONCE_KEY_MAX {
        // Nonce-free branch:
        //   (a) cap the expiry window so the on-chain seen-set ring stays bounded;
        //   (b) pre-check the `sender_signature_hash`-keyed seen slot — a non-zero
        //       `seen_expiry > block_ts` means a previous tx with the same signing
        //       preimage is still live in the replay window. This rejects replays
        //       up-front instead of letting them burn gas at execution time.
        let max_allowed = block_timestamp.saturating_add(NONCE_FREE_MAX_EXPIRY_WINDOW);
        if tx.expiry > max_allowed {
            return Err(Eip8130ValidationError::NonceFreeExpiryTooFar {
                expiry: tx.expiry,
                max_allowed,
            });
        }
        let sig_hash = sender_signature_hash(tx);
        let seen_slot = aa_expiring_seen_slot(sig_hash);
        let seen_storage_key = B256::from(seen_slot.to_be_bytes());
        let seen_expiry = state
            .storage(NONCE_MANAGER_ADDRESS, seen_storage_key)
            .map_err(|e| Eip8130ValidationError::StateError(e.to_string()))?
            .unwrap_or_default()
            .to::<u64>();
        if seen_expiry != 0 && seen_expiry > block_timestamp {
            return Err(Eip8130ValidationError::NonceFreeReplay);
        }
        // Surface 0 so the standard pool, which keys on `state_nonce == tx.nonce()`,
        // treats the tx as immediately pending (`tx.nonce()` is also 0 for nonce-free
        // by the step-1 MUST enforced above).
        0
    } else {
        // Sequenced branch: `aa_nonce_slot(sender, nonce_key)` of NONCE_MANAGER must
        // equal the tx's `nonce_sequence`. A mismatch means either a gap (queue) or
        // a stale tx — the standard pool treats the latter as `Invalid` and the
        // former as a queue placement upstream.
        let slot_u256 = op_revm::precompiles_xlayer::aa_nonce_slot(sender, tx.nonce_key);
        let storage_key = B256::from(slot_u256.to_be_bytes());
        let on_chain = state
            .storage(NONCE_MANAGER_ADDRESS, storage_key)
            .map_err(|e| Eip8130ValidationError::StateError(e.to_string()))?
            .unwrap_or_default()
            .to::<u64>();
        if on_chain != tx.nonce_sequence {
            return Err(Eip8130ValidationError::NonceMismatch {
                expected: on_chain,
                got: tx.nonce_sequence,
            });
        }
        on_chain
    };

    // Step 8: AA intrinsic-gas computation.
    // [DEFERRED] requires `XLayerAAGasSchedule::for_spec(OpSpecId)` which doesn't yet
    // exist in this codebase. See xlayer-aa.md "Intrinsic gas constants hard-coded
    // in `build_aa_parts` with no fork binding" (2026-04-21). Step 11 below uses a
    // best-effort lower bound (gas_limit * max_fee) that ignores the intrinsic term;
    // the execution-layer handler is the authoritative gas accountant.

    // Step 9: sender authorization (owner_config read + verifier dispatch).
    // [DEFERRED] requires `parse_sender_auth` to extract `owner_id` from
    // `tx.sender_auth`, plus native verifier dispatch (K1/P256/WebAuthn/Delegate) and
    // custom-verifier STATICCALL admission. The handler enforces this at execution
    // time; its absence here means the mempool can pool a tx that the executor would
    // reject. Acceptable for the MVP slice; mandatory before mainnet.

    // Step 11: balance check. AA-aware lower bound is `gas_limit * max_fee_per_gas`.
    // The intrinsic-gas term from base's formula is omitted until step 8 lands. The
    // L1-data-fee guard in the validator wraps this with the L1 component when
    // `requires_l1_data_gas_fee()` is set on the node.
    let balance = state
        .account_balance(&payer)
        .map_err(|e| Eip8130ValidationError::StateError(e.to_string()))?
        .unwrap_or_default();
    let max_gas_cost =
        U256::from(tx.gas_limit).saturating_mul(U256::from(tx.max_fee_per_gas));
    if balance < max_gas_cost {
        return Err(Eip8130ValidationError::InsufficientBalance {
            required: max_gas_cost,
            available: balance,
        });
    }

    // Step 12: invalidation keys (state-diff index for the side-pool eviction task).
    // [DEFERRED] no side pool / invalidation index exists in this slice. The
    // standard reth pool's `on_canonical_state_change` hook plus its built-in nonce
    // tracking covers the cases this slice admits (sequenced, self-pay, no
    // account_changes); when the side pool lands, this fn returns the keys here.

    Ok(Eip8130ValidationOutcome {
        sender,
        payer,
        state_nonce,
        nonce_key: tx.nonce_key,
        balance,
    })
}

#[cfg(test)]
mod xlayer_tests {
    use super::*;
    use alloy_primitives::{Address, U256};
    use op_alloy_consensus::{Eip8130CallEntry, TxEip8130};
    use reth_optimism_primitives::OpPrimitives;
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};

    fn make_tx(chain_id: u64, sender: Address, nonce_sequence: u64) -> TxEip8130 {
        TxEip8130 {
            chain_id,
            from: Some(sender),
            nonce_key: U256::ZERO,
            nonce_sequence,
            expiry: 0,
            max_priority_fee_per_gas: 0,
            max_fee_per_gas: 1,
            gas_limit: 100_000,
            calls: vec![vec![Eip8130CallEntry {
                to: Address::repeat_byte(0x22),
                data: Default::default(),
            }]],
            ..Default::default()
        }
    }

    /// Minimal happy-path admission test: build a self-pay AA tx, seed the NONCE_MANAGER
    /// account so the slot read returns zero, hand it to `validate_eip8130_transaction`,
    /// assert `Ok`. Mirrors the shape of `eip8130_pool::add_single_transaction` in the
    /// reference repo — no async, no chainspec, no validator builder — only the function
    /// under test plus the minimum state provider needed for its two storage reads.
    #[test]
    fn add_single_transaction() {
        const CHAIN_ID: u64 = 10;
        let sender = Address::repeat_byte(0x11);

        let client = MockEthProvider::<OpPrimitives>::new();
        client.add_account(NONCE_MANAGER_ADDRESS, ExtendedAccount::new(0, U256::ZERO));
        client.add_account(sender, ExtendedAccount::new(0, U256::from(1_000_000_000_u64)));

        let tx = make_tx(CHAIN_ID, sender, 0);
        let outcome = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client)
            .expect("self-pay AA tx must pass mempool validation");

        assert_eq!(outcome.sender, sender);
        assert_eq!(outcome.payer, sender);
        assert_eq!(outcome.state_nonce, 0);
        assert_eq!(outcome.nonce_key, U256::ZERO);
    }
}
