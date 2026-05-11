//! Mempool admission validation for EIP-8130 (XLayer AA) transactions.
//!
//! Replicates the structural and minimal-state checks the consensus handler runs in
//! [`op_revm::handler::OpHandler::validate_env`] and the early portion of
//! [`validate_against_state_and_deduct_caller`], so that malformed AA transactions are
//! rejected before they reach the standard pool's pending queue.
//!
//! Auth dispatch (K1 / P256-raw / WebAuthn / Delegate→Native), sponsored payer
//! re-check, sequence_updates and account_changes pre-state, account-lock and
//! owner-config validation are all enforced here. The one remaining
//! deferral is the custom-verifier STATICCALL at admission (P5): when
//! `sender_auth` / `payer_auth` resolves to a Custom verifier, the spec
//! layer admits the tx without running the STATICCALL and lets the
//! executor catch a verifier mismatch at inclusion. The trade-off is one
//! gossip round per replayed-bad-signature attempt; a STATICCALL at
//! admission would shift that cost onto every honest tx.

use alloy_op_evm::eip8130::{
    address::derive_account_address,
    auth_state::{build_payer_auth_state, build_sender_auth_state},
};
use alloy_primitives::{Address, B256, U256, keccak256};
use op_alloy_consensus::{
    AccountChangeEntry, ConfigChangeEntry, TxEip8130, config_change_digest, sender_signature_hash,
};
use op_revm::{
    OpSpecId,
    constants::{
        ACCOUNT_CONFIG_ADDRESS, K1_VERIFIER_ADDRESS, OP_AUTHORIZE_OWNER, OP_REVOKE_OWNER,
        OWNER_SCOPE_CONFIG, OWNER_SCOPE_PAYER, OWNER_SCOPE_SENDER, REVOKED_VERIFIER,
    },
    eip8130_gas::aa_intrinsic_gas,
    eip8130_policy::{PendingOwnerState, pending_owner_state_for_change},
    gas_params::xlayer_gas_params,
    handler::{
        NONCE_FREE_MAX_EXPIRY_WINDOW, NONCE_KEY_MAX, aa_expiring_seen_slot, aa_lock_slot,
        aa_owner_config_slot, parse_owner_config_word, read_packed_sequence,
        unlocks_at_from_account_state_word,
    },
    precompiles_xlayer::NONCE_MANAGER_ADDRESS,
    transaction::eip8130::AuthState,
};
use reth_storage_api::{StateProvider, StateProviderFactory};
use reth_transaction_pool::error::PoolTransactionError;
use std::{any::Any, collections::HashMap};

/// Maximum encoded EIP-2718 size for an AA transaction at mempool ingress.
///
/// Mirrors `MAX_AA_TX_ENCODED_BYTES` used by reference implementations: large enough to
/// admit realistic flows (sponsored swaps, batch ops) but bounded so unbounded RLP cannot
/// exhaust pool memory through structural fan-out.
pub const MAX_AA_TX_ENCODED_BYTES: usize = 256 * 1024;

/// Reject AA txs whose `expiry` is within this many seconds of `block_timestamp` so the
/// tx isn't admitted only to be evicted at peers with slightly newer tips. Matches
/// tempo's `AA_VALID_BEFORE_MIN_SECS` (tempo `crates/transaction-pool/src/validator.rs:36`)
/// and `EVICTION_BUFFER_SECS` (tempo `crates/transaction-pool/src/maintain.rs:35`).
pub const EXPIRY_ADMISSION_BUFFER_SECS: u64 = 3;

/// Maximum bytes of `data` per call entry. Bounds per-tx memory and gossip bandwidth so a
/// single AA tx cannot exhaust pool buffers via a single oversized call. Matches tempo's
/// `MAX_CALL_INPUT_SIZE` (tempo `crates/transaction-pool/src/validator.rs:45`).
pub const MAX_CALL_INPUT_BYTES: usize = 128 * 1024;

// `validate_eip8130_transaction` returns the on-chain nonce sequence as a
// bare `u64` rather than a typed wrapper. Previous revisions threaded a
// two-field struct (state_nonce + required_balance) through the AA
// pre-validator trait; per the user correction (2026-05-11) we instead
// rely on `TransactionValidationOutcome::Valid::state_nonce` (overwritten
// by the AA layer) and recompute `required_balance` at the side-pool
// admission site so the trait surface stays at a single return value.

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
    /// A single call's `data` exceeds [`MAX_CALL_INPUT_BYTES`].
    #[error("EIP-8130 call {call_index} input too large: {size} bytes (limit {limit})")]
    CallInputTooLarge {
        /// Index of the offending call within the flattened call sequence.
        call_index: usize,
        /// Call data length in bytes.
        size: usize,
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
    /// Payer balance does not cover `gas_limit * max_fee_per_gas + l1_data_fee`.
    ///
    /// AA-aware balance check (mirrors the gas-based balance assertion in base's
    /// `validate_eip8130_transaction`). The L1-data-fee component is folded in
    /// by the AA wrapper from the cached `OpL1BlockInfo`; non-AA txs use the
    /// inner OP wrapper's `apply_op_checks` instead. Intrinsic-gas accounting
    /// is intentionally absent here: until the xlayer-aa gas-schedule resolver
    /// lands, the universally available bound is `gas_limit * max_fee_per_gas`.
    #[error("EIP-8130 insufficient payer balance: required {required}, available {available}")]
    InsufficientBalance {
        /// `gas_limit * max_fee_per_gas + l1_data_fee`.
        required: U256,
        /// On-chain payer balance.
        available: U256,
    },
    /// `sender_auth` is malformed at parse time or fails the eager native verification
    /// (signature recovery / inner-binding / strict-self-owner). Carries the resolver's
    /// reason for diagnosis. Spec violation — peer penalization is appropriate.
    #[error("EIP-8130 invalid sender_auth: {0}")]
    InvalidSenderAuth(String),
    /// `sender_auth` is empty in pool admission. Empty is the `eth_estimateGas` escape
    /// hatch the handler accepts only with fee checks disabled — admitting it here
    /// would let an attacker push unauthenticated txs into the gossip mesh.
    #[error("EIP-8130 sender_auth required for pool admission")]
    SenderAuthRequired,
    /// On-chain `owner_config[account][owner_id]` carries the `REVOKED_VERIFIER`
    /// sentinel — the owner has been explicitly tombstoned and cannot authenticate.
    /// State-disagreement (peer may not yet have observed the revoke); not a peer
    /// protocol violation.
    #[error("EIP-8130 sender owner revoked: account={account}, owner_id={owner_id:?}")]
    OwnerRevoked {
        /// The account whose owner_config row is revoked.
        account: Address,
        /// The owner id in the row.
        owner_id: B256,
    },
    /// On-chain `owner_config[account][owner_id]` binds a different verifier than the
    /// one resolved from `sender_auth`. State-disagreement (registry can be re-resolved
    /// upstream); not a peer protocol violation.
    #[error(
        "EIP-8130 owner_config verifier mismatch: account={account}, owner_id={owner_id:?}, expected={expected}, on_chain={on_chain}"
    )]
    OwnerConfigMismatch {
        /// The account whose owner_config row was read.
        account: Address,
        /// The owner id in the row.
        owner_id: B256,
        /// Verifier resolved from `sender_auth`.
        expected: Address,
        /// Verifier on-chain (zero means slot empty + implicit-EOA rule failed).
        on_chain: Address,
    },
    /// On-chain `owner_config` row is non-zero scope but lacks the SENDER bit. The
    /// owner exists but isn't authorized for this side. State-disagreement — not a
    /// peer protocol violation.
    #[error(
        "EIP-8130 owner_config scope missing required bit 0x{required_bit:02x} (on-chain scope 0x{on_chain_scope:02x})"
    )]
    OwnerScopeMissing {
        /// The scope bit that was required (e.g. `OWNER_SCOPE_SENDER`).
        required_bit: u8,
        /// The on-chain scope byte from the owner_config row.
        on_chain_scope: u8,
    },
    /// `payer_auth` is malformed at parse time or fails the eager native verification.
    /// Spec violation — peer penalization is appropriate. Mirrors `InvalidSenderAuth`
    /// for the payer side.
    #[error("EIP-8130 invalid payer_auth: {0}")]
    InvalidPayerAuth(String),
    /// `payer_auth` is empty in pool admission while `tx.payer` is `Some`. Empty is
    /// the `eth_estimateGas` escape the handler accepts only with fee checks
    /// disabled — admitting it here would let an attacker push unauthenticated
    /// sponsored txs into the gossip mesh. Spec violation — peer-violation.
    #[error("EIP-8130 payer_auth required for sponsored pool admission")]
    PayerAuthRequired,
    /// More than one `Delegation` entry in `account_changes`. The spec
    /// allows at most one delegation per tx (a tx targets one account).
    /// Mirrors the executor's handler `validate_env` rejection at
    /// op-revm `handler.rs:1077`. Peer-violation.
    #[error("EIP-8130 transaction has more than one delegation entry")]
    MultipleDelegationEntries,
    /// `Create` entry is present but is not the first entry in
    /// `account_changes`. EIP-8130 spec invariant — Create MUST be the
    /// first entry. Mirrors op-revm `handler.rs:1068`. Peer-violation.
    #[error("EIP-8130 create entry must be first in account_changes")]
    CreateNotFirstEntry,
    /// Create entry's runtime `bytecode.len()` exceeds EIP-170's
    /// `MAX_CODE_SIZE` (24576 bytes). Mirrors op-revm
    /// `handler.rs:1045-1051`. Peer-violation.
    #[error("EIP-8130 create bytecode too large: {size} bytes (limit {limit})")]
    CreateBytecodeTooLarge {
        /// Bytecode length in bytes.
        size: usize,
        /// EIP-170 maximum.
        limit: usize,
    },
    /// A `ConfigChange` entry that targets this chain has zero effective
    /// owner-change ops AND zero `authorizer_auth` AND zero
    /// `sequence_updates` participation — i.e. the entry contributes
    /// nothing yet still bumps `_accountState` and runs authorizer
    /// validation. Mirrors op-revm `handler.rs:1058-1062`'s
    /// `matching_config_change_with_zero_valid_ops` rejection. The
    /// parser sets the `Eip8130AccountChanges` flag; we check the entry
    /// directly here. Peer-violation.
    #[error("EIP-8130 config change entry has no valid ops")]
    EmptyConfigChange,
    /// A `Create` entry targets an address that already has non-empty
    /// code or a non-zero nonce. Mirrors op-revm `handler.rs:1520-1530`
    /// (post-execution replay guard for Create deployment). Reading the
    /// targeted address here surfaces the rejection at admission rather
    /// than at inclusion. State-disagreement (the on-chain account may
    /// be cleaned up by an in-flight tx); not a peer-violation.
    #[error("EIP-8130 create entry collides with existing account: {address}")]
    CreateTargetCollision {
        /// Derived deployment address.
        address: Address,
    },
    /// A `Create` entry's `pre_writes` slot is occupied by a non-zero
    /// owner_config row. Mirrors op-revm `handler.rs:1539-1552`'s
    /// pre_writes replay guard. State-disagreement; not a peer-violation.
    #[error("EIP-8130 create entry pre_write slot occupied: address={address}, slot={slot}")]
    CreateOwnerSlotOccupied {
        /// The address whose owner_config slot is occupied.
        address: Address,
        /// The owner_config storage slot.
        slot: U256,
    },
    /// Sender's `_accountState[sender]` lock half indicates the account
    /// is still within its lock window: `block_timestamp < unlocks_at`.
    /// Mirrors op-revm `handler.rs:343-370`'s `check_account_lock`. The
    /// lock check fires whenever the tx carries any lock-sensitive
    /// account_changes entry: a Create, a ConfigChange with config
    /// writes, OR a Delegation. State-disagreement (the lock may
    /// expire before inclusion in some races; another tx may release
    /// it). Not a peer-violation.
    #[error("EIP-8130 sender account locked until {unlocks_at}")]
    AccountLocked {
        /// `unlocks_at` decoded from the packed account_state word.
        unlocks_at: u64,
    },
    /// `ACCOUNT_CONFIG_ADDRESS` is not deployed (its account `code_hash
    /// == keccak256([])`) but the tx carries config writes that require
    /// it. Mirrors op-revm `handler.rs:470-481`. State-disagreement
    /// (the contract may be deployed by a preceding tx in the same
    /// block); not a peer-violation.
    #[error("EIP-8130 AccountConfiguration not yet deployed for config writes")]
    AccountConfigNotDeployed,
    /// Authorizer-chain validation: a Native `authorizer_validation`
    /// resolved to an `owner_id` whose on-chain `owner_config` row
    /// binds a different verifier than the auth blob's prefix. Mirrors
    /// the verifier-mismatch arm of op-revm
    /// `validate_owner_against_effective_config` (`handler.rs:259-261`)
    /// run from `validate_authorizer_chain` (`handler.rs:824-832`).
    /// State-disagreement — peer may not yet have observed a registry
    /// repointing. Not a peer-violation.
    #[error(
        "EIP-8130 authorizer owner verifier mismatch: owner_id={owner_id}, expected={expected_verifier}, on_chain={on_chain_verifier}"
    )]
    AuthorizerOwnerMismatch {
        /// The owner id derived from the authorizer auth blob.
        owner_id: U256,
        /// Verifier from the auth blob's 20-byte prefix.
        expected_verifier: Address,
        /// Verifier currently bound at `owner_config[sender][owner_id]`.
        on_chain_verifier: Address,
    },
    /// Authorizer-chain validation: the auth blob produced an invalid
    /// `owner_id` (zero owner_id from native verify, or malformed
    /// prefix). Mirrors op-revm `handler.rs:818-822`'s zero-owner_id
    /// rejection. The auth blob is structurally invalid before any
    /// state read — peer-violation.
    #[error("EIP-8130 authorizer produced invalid owner_id (zero or malformed)")]
    AuthorizerInvalidOwnerId,
    /// Authorizer-chain validation: a Native authorizer's owner exists
    /// on-chain but its scope byte lacks the `OWNER_SCOPE_CONFIG` bit.
    /// Mirrors op-revm `handler.rs:262-264` run from
    /// `validate_authorizer_chain`. State-disagreement; not a
    /// peer-violation.
    #[error(
        "EIP-8130 authorizer owner lacks CONFIG scope: owner_id={owner_id}, on_chain_scope=0x{on_chain_scope:02x}"
    )]
    AuthorizerOwnerLacksConfigScope {
        /// The owner id from the authorizer auth blob.
        owner_id: U256,
        /// Scope byte read from `owner_config[sender][owner_id]`.
        on_chain_scope: u8,
    },
    /// A `Delegation` entry requires the sender to be authenticated as
    /// their own EOA self-owner (`owner_id == bytes32(bytes20(sender))`)
    /// via the K1 native verifier. Mirrors op-revm
    /// `check_delegation_requires_eoa_config_owner`
    /// (`handler.rs:384-441`). The auth blob is structurally
    /// incompatible with delegation — peer-violation.
    #[error("EIP-8130 delegation requires sender authenticated as EOA self-owner")]
    DelegationRequiresEoaSelfOwner,
    /// A `Delegation` entry is present but the sender's K1 EOA
    /// self-owner row lacks the `OWNER_SCOPE_CONFIG` bit. Mirrors
    /// op-revm `handler.rs:432-440`'s
    /// `validate_owner_against_effective_config(.., OWNER_SCOPE_CONFIG, ..)`.
    /// State-disagreement; not a peer-violation.
    #[error(
        "EIP-8130 delegation owner lacks CONFIG scope: owner_id={owner_id}, on_chain_scope=0x{on_chain_scope:02x}"
    )]
    DelegationOwnerLacksConfigScope {
        /// The EOA self-owner_id (`bytes32(bytes20(sender))`).
        owner_id: U256,
        /// On-chain scope byte from `owner_config[sender][owner_id]`.
        on_chain_scope: u8,
    },
    /// A `ConfigChange.sequence` of `u64::MAX` would overflow when the
    /// executor computes `new_value = sequence + 1` (mirrored by op-revm
    /// `handler.rs:501-504`'s `checked_sub(1)` on `new_value`). Sender
    /// produced a malformed tx — peer-violation.
    #[error("EIP-8130 config change sequence overflow (sequence == u64::MAX)")]
    SequenceUpdateUnderflow,
    /// On-chain `_accountState[sender]` half does not match the pre-value
    /// the tx pinned via `ConfigChange.sequence` (rule D, spec line 376;
    /// mirrors op-revm `handler.rs:506-516`). State-disagreement: a
    /// competing tx may already have advanced the half on this peer's
    /// view; not a peer-violation.
    #[error(
        "EIP-8130 sequence mismatch (multichain={is_multichain}): on-chain pre={expected_pre}, tx pre={got_pre}"
    )]
    SequenceMismatch {
        /// `true` for the multichain half (`chain_id == 0`); `false` for
        /// the local half.
        is_multichain: bool,
        /// On-chain pre-value at the half (the running expected value
        /// after applying any earlier in-tx updates).
        expected_pre: u64,
        /// `tx_sequence = ConfigChange.sequence` — the pre-value the tx
        /// committed to.
        got_pre: u64,
    },
    /// On-chain `NONCE_MANAGER` sequence does not match `nonce_sequence` for the lane.
    #[error("EIP-8130 nonce mismatch: on-chain {expected}, tx {got}")]
    NonceMismatch {
        /// On-chain sequence at `aa_nonce_slot(sender, nonce_key)`.
        expected: u64,
        /// `tx.nonce_sequence`.
        got: u64,
    },
    /// `gas_limit` is below the EIP-8130 intrinsic gas computed by op-revm.
    #[error("EIP-8130 intrinsic gas too low: required {required}, gas_limit {gas_limit}")]
    IntrinsicGasTooLow {
        /// Intrinsic gas required by the active XLayer gas schedule.
        required: u64,
        /// Transaction gas limit.
        gas_limit: u64,
    },
    /// State-provider read failure surfaced from the underlying client.
    #[error("EIP-8130 state read failed: {0}")]
    StateError(String),
}

impl PoolTransactionError for Eip8130ValidationError {
    fn is_bad_transaction(&self) -> bool {
        match self {
            // Spec MUSTs and structural caps — sender produced a malformed tx; peer
            // penalization is appropriate. `InvalidSenderAuth` and
            // `SenderAuthRequired` are auth-blob malformations the resolver catches at
            // decode time (bad RLP / wrong-length / failed K1 strict-self-owner /
            // empty in pool mode) — same class as the structural caps.
            Self::TxTooLarge { .. } |
            Self::ChainIdMismatch { .. } |
            Self::TooManyCalls { .. } |
            Self::TooManyAccountChanges { .. } |
            Self::CallInputTooLarge { .. } |
            Self::MultipleCreateEntries |
            Self::NonceFreeMissingExpiry |
            Self::NonceFreeNonZeroSequence { .. } |
            Self::NonceFreeExpiryTooFar { .. } |
            Self::IntrinsicGasTooLow { .. } |
            Self::InvalidSenderAuth(_) |
            Self::InvalidPayerAuth(_) |
            // `sequence == u64::MAX` is a malformed tx — `new_value =
            // sequence + 1` overflows. The executor catches this via
            // `checked_sub(1)` on `new_value` (op-revm `handler.rs:503`);
            // surface it as a peer-violation at admission too.
            Self::SequenceUpdateUnderflow |
            Self::SenderAuthRequired |
            Self::PayerAuthRequired |
            // Structural variants — each maps directly to a
            // `validate_env`-class structural reject in op-revm.
            Self::MultipleDelegationEntries |
            Self::CreateNotFirstEntry |
            Self::CreateBytecodeTooLarge { .. } |
            Self::EmptyConfigChange |
            // Authorizer auth blob produced a zero/invalid owner_id at
            // parse time (no state needed). Same class as
            // `InvalidSenderAuth` — malformed auth, peer should not
            // have forwarded.
            Self::AuthorizerInvalidOwnerId |
            // Delegation requires K1 + EOA self-owner_id; mismatch is a
            // structural property of the resolved sender_authstate, not
            // a state question. Same class as `InvalidSenderAuth`.
            Self::DelegationRequiresEoaSelfOwner => true,
            // Time-window / state-disagreement / replay / unsupported-but-well-formed:
            // not a peer protocol violation. Replay-seen and balance-shortfall in
            // particular can be transient (the on-chain seen ring rotates, balance can
            // be topped up) so we don't penalize peers for forwarding them. The
            // owner_config divergence variants are state-disagreement: the peer may
            // simply not have observed the revoke / verifier swap yet.
            Self::Expired { .. } |
            Self::NonceMismatch { .. } |
            Self::NonceFreeReplay |
            Self::InsufficientBalance { .. } |
            Self::OwnerRevoked { .. } |
            Self::OwnerConfigMismatch { .. } |
            Self::OwnerScopeMissing { .. } |
            // `SequenceMismatch` is state-disagreement: a competing tx
            // may already have advanced the sequence on this peer's
            // view. Identical classification to `NonceMismatch`.
            Self::SequenceMismatch { .. } |
            // State-disagreement variants — the on-chain row, lock
            // window, or contract-deployment state may differ between
            // this peer's view and the gossip wave.
            Self::CreateTargetCollision { .. } |
            Self::CreateOwnerSlotOccupied { .. } |
            Self::AccountLocked { .. } |
            Self::AccountConfigNotDeployed |
            Self::AuthorizerOwnerMismatch { .. } |
            Self::AuthorizerOwnerLacksConfigScope { .. } |
            Self::DelegationOwnerLacksConfigScope { .. } |
            Self::StateError(_) => false,
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Outcome of resolving a `ConfigChange.authorizer_auth` blob to an
/// `(verifier, owner_id)` pair for admission-time validation.
///
/// Mirrors the three branches op-revm
/// `validate_authorizer_chain` (`handler.rs:765-844`) handles:
/// - `Native`: native verify recovered an owner_id from the blob — the admission path can
///   re-validate against on-chain `owner_config`. Covers ALL native verifiers (K1, P256-raw,
///   P256-WebAuthn). The executor's per-iteration dispatch at `handler.rs:802-816` gates on
///   `verify_call.is_none()` (Native) vs `verify_call.is_some()` (Custom STATICCALL); the parts
///   builder's `build_authorizer_validation` (alloy-op-evm `parts.rs:457-481`) already populates
///   `owner_id` for the Native branch via `try_native_verify`. We mirror that resolution here.
/// - `DeferredCustom`: the verifier is a custom contract — the executor runs the STATICCALL at
///   inclusion. Only this shape (i.e. `verify_call.is_some()` in the parts builder) defers; Native
///   non-K1 used to be wrongly bucketed here.
/// - `Malformed`: empty / too-short blob, or Delegate-as-authorizer (rejected by the parser at
///   alloy-op-evm `parts.rs:445-455`), or any native-verify Invalid result. Mirrors the parser's
///   `verifier=ZERO, owner_id=ZERO` fallback, which the executor catches via the zero-owner_id
///   reject.
enum AuthorizerResolution {
    Native { verifier: Address, owner_id: U256 },
    DeferredCustom,
    Malformed,
}

/// Resolves a `ConfigChange.authorizer_auth` blob via the same dispatch
/// the parts builder does.
///
/// The dispatch gate is `try_native_verify` (alloy-op-evm
/// `native_verifier.rs:107-115`), exactly mirroring
/// `build_authorizer_validation` (alloy-op-evm `parts.rs:457-481`):
///   - Native verifier (K1 / P256-raw / P256-WebAuthn) succeeded → [`AuthorizerResolution::Native`]
///     with the recovered owner_id;
///   - Native verifier failed (bad sig length / ecrecover failure / etc.) →
///     [`AuthorizerResolution::Malformed`] (parts builder produces a `verifier, owner_id = ZERO`
///     placeholder which the executor's zero-owner_id check rejects at `handler.rs:818-822`);
///   - Unknown verifier address → [`AuthorizerResolution::DeferredCustom`] (custom STATICCALL —
///     executor runs it at `handler.rs:802-813`).
///
/// Was previously coarser ("only K1 is Native; everything else is
/// DeferredCustom"), which wrongly admitted P256-raw / WebAuthn
/// authorizers without re-validating their on-chain `owner_config` row.
fn resolve_authorizer_owner(sender: Address, cc: &ConfigChangeEntry) -> AuthorizerResolution {
    use alloy_op_evm::eip8130::native_verifier::{NativeVerifyResult, try_native_verify};
    use op_revm::constants::DELEGATE_VERIFIER_ADDRESS;
    if cc.authorizer_auth.is_empty() || cc.authorizer_auth.len() < 20 {
        return AuthorizerResolution::Malformed;
    }
    let verifier = Address::from_slice(&cc.authorizer_auth[..20]);
    if verifier == DELEGATE_VERIFIER_ADDRESS {
        // alloy-op-evm `parts.rs:445-455`: Delegate-as-authorizer is
        // rejected — nesting Delegate within CONFIG-scope auth is
        // not part of EIP-8130's authorizer surface.
        return AuthorizerResolution::Malformed;
    }
    let verifier_data = cc.authorizer_auth.slice(20..);
    let digest = config_change_digest(sender, cc);
    match try_native_verify(verifier, &verifier_data, digest) {
        NativeVerifyResult::Verified(owner_id) => {
            AuthorizerResolution::Native { verifier, owner_id: U256::from_be_bytes(owner_id.0) }
        }
        // Mirrors the parts builder's `verifier=*, owner_id=ZERO` shape
        // for an Invalid native sig (alloy-op-evm `parts.rs:464-469`),
        // which the executor's zero-owner_id check rejects at
        // `handler.rs:818-822`. We surface as `AuthorizerInvalidOwnerId`.
        NativeVerifyResult::Invalid(_) => AuthorizerResolution::Malformed,
        // Custom verifier — executor runs STATICCALL at inclusion
        // (op-revm `handler.rs:802-813`).
        NativeVerifyResult::Unsupported => AuthorizerResolution::DeferredCustom,
    }
}

/// Validates a Native authorizer's owner against on-chain
/// `owner_config[sender][owner_id]`, honoring the pending overlay
/// (same-tx ConfigChange writes that earlier authorizers already
/// approved).
///
/// Mirrors op-revm `validate_owner_against_effective_config`
/// (`handler.rs:201-266`) with `allow_implicit_eoa = true` (the
/// executor's `validate_authorizer_chain` calls into it with
/// `true` at `handler.rs:824-832`). Errors map to:
/// - pending Revoked / VerifierMismatch / MissingScope → `AuthorizerOwnerMismatch` /
///   `AuthorizerOwnerLacksConfigScope` depending on which condition failed;
/// - on-chain REVOKED_VERIFIER / verifier mismatch / missing CONFIG scope → same mapping.
fn validate_authorizer_owner_against_state(
    state: &dyn StateProvider,
    sender: Address,
    owner_id: U256,
    expected_verifier: Address,
    pending_overlay: &HashMap<U256, PendingOwnerState>,
) -> Result<(), Eip8130ValidationError> {
    use op_revm::eip8130_policy::{PendingOwnerValidationError, validate_pending_owner_state};
    if let Some(pending) = pending_overlay.get(&owner_id) {
        return match validate_pending_owner_state(pending, expected_verifier, OWNER_SCOPE_CONFIG) {
            Ok(()) => Ok(()),
            Err(PendingOwnerValidationError::Revoked) => {
                // A pending revoke wins — surface as mismatch with
                // verifier=REVOKED_VERIFIER for diagnostic clarity.
                Err(Eip8130ValidationError::AuthorizerOwnerMismatch {
                    owner_id,
                    expected_verifier,
                    on_chain_verifier: REVOKED_VERIFIER,
                })
            }
            Err(PendingOwnerValidationError::VerifierMismatch { actual, .. }) => {
                Err(Eip8130ValidationError::AuthorizerOwnerMismatch {
                    owner_id,
                    expected_verifier,
                    on_chain_verifier: actual,
                })
            }
            Err(PendingOwnerValidationError::MissingScope { .. }) => {
                Err(Eip8130ValidationError::AuthorizerOwnerLacksConfigScope {
                    owner_id,
                    on_chain_scope: 0,
                })
            }
        };
    }
    let slot = aa_owner_config_slot(sender, owner_id);
    let key = B256::from(slot.to_be_bytes());
    let word = state
        .storage(ACCOUNT_CONFIG_ADDRESS, key)
        .map_err(|e| Eip8130ValidationError::StateError(e.to_string()))?
        .unwrap_or_default();
    let (on_chain_verifier, on_chain_scope) = parse_owner_config_word(word);
    if on_chain_verifier == REVOKED_VERIFIER {
        return Err(Eip8130ValidationError::AuthorizerOwnerMismatch {
            owner_id,
            expected_verifier,
            on_chain_verifier,
        });
    }
    if on_chain_verifier == Address::ZERO {
        // Implicit-EOA fallback: only valid when expected_verifier is K1
        // AND owner_id == bytes32(bytes20(sender)). Mirrors op-revm
        // `handler.rs:243-257`.
        let implicit_owner_id = {
            let mut buf = [0u8; 32];
            buf[..20].copy_from_slice(sender.as_slice());
            U256::from_be_bytes(buf)
        };
        if owner_id == implicit_owner_id && expected_verifier == K1_VERIFIER_ADDRESS {
            return Ok(());
        }
        return Err(Eip8130ValidationError::AuthorizerOwnerMismatch {
            owner_id,
            expected_verifier,
            on_chain_verifier: Address::ZERO,
        });
    }
    if on_chain_verifier != expected_verifier {
        return Err(Eip8130ValidationError::AuthorizerOwnerMismatch {
            owner_id,
            expected_verifier,
            on_chain_verifier,
        });
    }
    // Scope == 0 = "default scope" (handler.rs:262); only a non-zero
    // scope that omits the CONFIG bit is a fail.
    if on_chain_scope != 0 && (on_chain_scope & OWNER_SCOPE_CONFIG) == 0 {
        return Err(Eip8130ValidationError::AuthorizerOwnerLacksConfigScope {
            owner_id,
            on_chain_scope,
        });
    }
    Ok(())
}

/// Validates the sender's K1 EOA self-owner row carries CONFIG scope
/// (or qualifies as implicit-EOA / pending-overlay-authorized).
///
/// Mirrors the second half of op-revm
/// `check_delegation_requires_eoa_config_owner` (`handler.rs:432-440`):
/// `validate_owner_against_effective_config(sender, eoa_owner_id, K1,
/// OWNER_SCOPE_CONFIG, allow_implicit_eoa = true, pending_overlay)`.
fn validate_delegation_owner_scope(
    state: &dyn StateProvider,
    sender: Address,
    eoa_owner_id: U256,
    pending_overlay: &HashMap<U256, PendingOwnerState>,
) -> Result<(), Eip8130ValidationError> {
    use op_revm::eip8130_policy::validate_pending_owner_state;
    if let Some(pending) = pending_overlay.get(&eoa_owner_id) {
        return match validate_pending_owner_state(pending, K1_VERIFIER_ADDRESS, OWNER_SCOPE_CONFIG)
        {
            Ok(()) => Ok(()),
            Err(_) => Err(Eip8130ValidationError::DelegationOwnerLacksConfigScope {
                owner_id: eoa_owner_id,
                on_chain_scope: 0,
            }),
        };
    }
    let slot = aa_owner_config_slot(sender, eoa_owner_id);
    let key = B256::from(slot.to_be_bytes());
    let word = state
        .storage(ACCOUNT_CONFIG_ADDRESS, key)
        .map_err(|e| Eip8130ValidationError::StateError(e.to_string()))?
        .unwrap_or_default();
    let (on_chain_verifier, on_chain_scope) = parse_owner_config_word(word);
    if on_chain_verifier == REVOKED_VERIFIER {
        return Err(Eip8130ValidationError::DelegationOwnerLacksConfigScope {
            owner_id: eoa_owner_id,
            on_chain_scope,
        });
    }
    if on_chain_verifier == Address::ZERO {
        // Implicit-EOA fallback: K1 + bytes20 owner_id automatically
        // satisfies the binding (handler.rs:243-257). The CONFIG scope
        // is implicit too — same handler logic.
        return Ok(());
    }
    if on_chain_verifier != K1_VERIFIER_ADDRESS {
        return Err(Eip8130ValidationError::DelegationOwnerLacksConfigScope {
            owner_id: eoa_owner_id,
            on_chain_scope,
        });
    }
    if on_chain_scope != 0 && (on_chain_scope & OWNER_SCOPE_CONFIG) == 0 {
        return Err(Eip8130ValidationError::DelegationOwnerLacksConfigScope {
            owner_id: eoa_owner_id,
            on_chain_scope,
        });
    }
    Ok(())
}

/// Validates one Native owner_config binding `(account, owner_id) → (verifier, scope)`.
///
/// Mirrors op-revm `validate_owner_against_effective_config`
/// (`handler.rs:201-266`) for the no-pending-overrides slice — the
/// admission path doesn't run a same-tx pending overlay for sender / payer
/// auth (only authorizer-chain validation does, and that path uses
/// [`validate_authorizer_owner_against_state`]). Used for validate_eip8130_transaction:
///   - Step 9 outer sender slot;
///   - Step 9 inner sender slot (Delegate→Native);
///   - Step 9b outer payer slot;
///   - Step 9b inner payer slot (Delegate→Native).
///
/// Three-way classification mirrors `dispatch_auth_state`'s Native arm
/// (op-revm `handler.rs:676-710`): REVOKED → `OwnerRevoked`, verifier
/// mismatch (or empty slot without implicit-EOA fallback) →
/// `OwnerConfigMismatch`, non-zero scope without the required bit →
/// `OwnerScopeMissing`.
fn validate_native_auth_owner_config_slot(
    state: &dyn StateProvider,
    account: Address,
    owner_id: B256,
    expected_verifier: Address,
    required_scope_bit: u8,
) -> Result<(), Eip8130ValidationError> {
    let slot = aa_owner_config_slot(account, U256::from_be_bytes(owner_id.0));
    let storage_key = B256::from(slot.to_be_bytes());
    let word = state
        .storage(ACCOUNT_CONFIG_ADDRESS, storage_key)
        .map_err(|e| Eip8130ValidationError::StateError(e.to_string()))?
        .unwrap_or_default();
    let (on_chain_verifier, on_chain_scope) = parse_owner_config_word(word);

    if on_chain_verifier == REVOKED_VERIFIER {
        return Err(Eip8130ValidationError::OwnerRevoked { account, owner_id });
    }

    // Empty slot: implicit-EOA fallback is the only way through. Mirrors
    // op-revm `handler.rs:243-257` — only K1 verifier with `owner_id ==
    // bytes32(bytes20(account))` qualifies. Anything else (P256 / WebAuthn
    // / Delegate, or a different owner_id) requires explicit registration.
    if on_chain_verifier == Address::ZERO {
        let implicit_owner_id = {
            let mut buf = [0u8; 32];
            buf[..20].copy_from_slice(account.as_slice());
            B256::from(buf)
        };
        let implicit_eoa_ok =
            owner_id == implicit_owner_id && expected_verifier == K1_VERIFIER_ADDRESS;
        if !implicit_eoa_ok {
            return Err(Eip8130ValidationError::OwnerConfigMismatch {
                account,
                owner_id,
                expected: expected_verifier,
                on_chain: Address::ZERO,
            });
        }
        return Ok(());
    }

    if on_chain_verifier != expected_verifier {
        return Err(Eip8130ValidationError::OwnerConfigMismatch {
            account,
            owner_id,
            expected: expected_verifier,
            on_chain: on_chain_verifier,
        });
    }
    if on_chain_scope != 0 && (on_chain_scope & required_scope_bit) == 0 {
        return Err(Eip8130ValidationError::OwnerScopeMissing {
            required_bit: required_scope_bit,
            on_chain_scope,
        });
    }
    Ok(())
}

/// Validates an EIP-8130 transaction for mempool admission.
///
/// Step numbering mirrors
/// `base/crates/txpool/src/eip8130_validate.rs::validate_eip8130_transaction` so a side-by-side
/// review can confirm parity at each spec stage. The single surviving
/// deferral is the custom-verifier STATICCALL at admission (P5) — see the
/// module docstring for the trade-off; the executor catches at inclusion.
///
/// `encoded_len` is supplied by the caller to avoid a second envelope encoding pass; the
/// validator already holds the encoded form.
///
/// `l1_data_fee` is the L1-data-fee component (computed by the wrapper from the cached
/// [`crate::OpL1BlockInfo`]). The balance check folds it into the payer-keyed required
/// minimum so sponsored shapes are charged on the correct account. Callers that don't
/// want the L1 component (tests, dev-mode nodes with `require_l1_data_gas_fee=false`)
/// pass [`U256::ZERO`].
pub fn validate_eip8130_transaction<Client>(
    tx: &TxEip8130,
    encoded_len: usize,
    block_timestamp: u64,
    chain_id: u64,
    client: &Client,
    l1_data_fee: U256,
) -> Result<u64, Eip8130ValidationError>
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
    // Per-call data cap. Without this an attacker can pin a single call near the
    // 256 KiB envelope cap and burn pool memory regardless of `MAX_CALLS_PER_TX`.
    // `call_index` is the position in the flattened call sequence (across phases).
    let mut call_index = 0usize;
    for phase in &tx.calls {
        for call in phase {
            if call.data.len() > MAX_CALL_INPUT_BYTES {
                return Err(Eip8130ValidationError::CallInputTooLarge {
                    call_index,
                    size: call.data.len(),
                    limit: MAX_CALL_INPUT_BYTES,
                });
            }
            call_index += 1;
        }
    }
    // Mirror execution's unit accounting (alloy_op_evm::eip8130::account_change_units
    // and op_revm::handler check at parts.account_changes.account_change_units).
    // Spec: Create = 1 + initial_owners.len(); ConfigChange = owner_changes.len();
    // Delegation = 1. Counting top-level entries here would let a ConfigChange
    // with N owner_changes pass admission but be rejected by execution.
    let account_change_units = alloy_op_evm::eip8130::account_change_units(tx);
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
        return Err(Eip8130ValidationError::ChainIdMismatch {
            expected: chain_id,
            got: tx.chain_id,
        });
    }

    // Step 2: expiry timeline. `expiry == 0` is the "no expiry" sentinel; only
    // non-zero expiries are subject to the time-window check. The propagation
    // buffer (`EXPIRY_ADMISSION_BUFFER_SECS`) rejects txs that would expire at
    // peers with slightly newer tips before the gossip wave settles.
    if tx.expiry != 0 && block_timestamp.saturating_add(EXPIRY_ADMISSION_BUFFER_SECS) >= tx.expiry {
        return Err(Eip8130ValidationError::Expired {
            expiry: tx.expiry,
            current: block_timestamp,
        });
    }

    // Step 3: resolve sender via the AuthState dispatch from
    // `alloy_op_evm::eip8130::auth_state::build_sender_auth_state`. This shares one
    // resolution path with the executor (`OpHandler::dispatch_auth_state`'s Native
    // arm) so the mempool's view of `sender` stays consistent with execution. The
    // on-chain `owner_config` re-check happens in Step 9 once the state provider is
    // open. Empty/Invalid are rejected eagerly here so we don't waste a state read.
    //
    // - EOA mode (`tx.from == None`, bare K1 sig): `Native { owner_id =
    //   bytes32(bytes20(recovered_addr)), .. }` — `sender` is the recovered address.
    // - Explicit-from (`tx.from = Some(addr)`): `Native { .. }` for K1 / P256-raw / WebAuthn /
    //   Delegate→Native after eager native verify. `Deferred { .. }` for custom verifier (and
    //   Delegate→Custom). `sender` is `tx.from` for both.
    // - `Empty` / `Invalid(reason)` reject up-front. Empty in the pool is the `eth_estimateGas`
    //   shape — admitting it would let an attacker push unauthenticated txs into the gossip mesh.
    let sender_auth = build_sender_auth_state(tx);
    let sender = match (&sender_auth, tx.from) {
        (AuthState::Invalid(reason), _) => {
            return Err(Eip8130ValidationError::InvalidSenderAuth(reason.clone()));
        }
        (AuthState::Empty, _) => return Err(Eip8130ValidationError::SenderAuthRequired),
        // Defensive: handler treats SelfPay as a payer-only state. The sender
        // resolver must never produce SelfPay; surface it as an invalid auth so
        // an upstream wiring bug is caught here rather than silently passed on.
        (AuthState::SelfPay, _) => {
            return Err(Eip8130ValidationError::InvalidSenderAuth(
                "sender_auth resolved to SelfPay (impossible for sender side)".into(),
            ));
        }
        // TODO: custom verifier (Deferred) admission is not yet supported.
        // Until the validator can run the verifier's STATICCALL at pool ingress,
        // reject all Deferred shapes — both explicit-from custom verifier and
        // Delegate→Custom. Tests pinned to this behavior live under
        // `rejects_deferred_custom_verifier` / `rejects_delegate_to_custom`.
        (AuthState::Deferred { .. }, _) => {
            return Err(Eip8130ValidationError::InvalidSenderAuth(
                "TODO: custom verifier (Deferred) sender_auth admission not yet supported".into(),
            ));
        }
        // Explicit-from: trust the claimed `tx.from` (the resolver already
        // enforced K1 strict-self-owner against it).
        (_, Some(addr)) => addr,
        // EOA mode: `tx.from = None`, sender is the recovered K1 address. The
        // resolver already verified the signature and produced
        // `Native { owner_id = bytes32(bytes20(recovered)), .. }`.
        (AuthState::Native { owner_id, .. }, None) => Address::from_slice(&owner_id.0[..20]),
    };

    // Step 10: payer resolution. P2 admits sponsored shapes — same shape as the
    // sender-side dispatch in Step 3.
    //
    // `is_self_pay()` semantics (op_alloy_consensus `xlayer.rs:371`): true iff
    // `tx.payer.is_none()`. So `tx.payer == Some(sender)` is *not* self-pay at
    // the type level — we surface this distinction explicitly: the resolver
    // produces `AuthState::Native/Deferred/Empty/Invalid` for that shape and
    // we apply the same predicate as a sponsored payer that happens to be the
    // sender. The Step-9-style `(payer, owner_id)` check below is skipped when
    // `effective_payer == sender` (rule A already covers it).
    //
    // - SelfPay (`tx.payer.is_none()`): no further checks; balance falls back to sender via
    //   `effective_payer`.
    // - Empty in pool admission with `tx.payer.is_some()`: same estimateGas escape we reject for
    //   sender_auth — `PayerAuthRequired`.
    // - Invalid: parse / native-verify / strict-self-owner failure → peer-violation.
    // - Native / Deferred: admit; on-chain owner_config re-check happens below for Native +
    //   sponsored, executor STATICCALL handles Deferred.
    let effective_payer = tx.payer.unwrap_or(sender);
    let payer_auth = build_payer_auth_state(tx);
    match &payer_auth {
        AuthState::Invalid(reason) => {
            return Err(Eip8130ValidationError::InvalidPayerAuth(reason.clone()));
        }
        AuthState::Empty => {
            // Reaches Empty only via `is_self_pay() == false` per the resolver
            // (`auth_state.rs:122` short-circuits SelfPay first). Sponsored shape
            // with empty `payer_auth` — reject as the estimateGas escape.
            return Err(Eip8130ValidationError::PayerAuthRequired);
        }
        // TODO: custom verifier (Deferred) admission is not yet supported on
        // the payer side either. Reject until the validator can run the
        // verifier's STATICCALL at pool ingress. Pinned by
        // `rejects_sponsored_payer_deferred`.
        AuthState::Deferred { .. } => {
            return Err(Eip8130ValidationError::InvalidPayerAuth(
                "TODO: custom verifier (Deferred) payer_auth admission not yet supported".into(),
            ));
        }
        AuthState::SelfPay | AuthState::Native { .. } => {
            // SelfPay: nothing to validate beyond the sender-side checks.
            // Native: admitted here; the post-state-open re-check (after the
            // balance read) covers it against on-chain
            // owner_config[payer][owner_id].
        }
    }
    let payer = effective_payer;

    // Steps 6 / 7 / 7b: account_changes structural validation.
    //
    // Admitted only the carve-out (ConfigChange with empty
    // `owner_changes` AND empty `authorizer_auth`); P4 widens to:
    //   * `Create` entries (≤1, MUST be first if present),
    //   * `ConfigChange` with `owner_changes` and/or `authorizer_auth`,
    //   * `Delegation` entries (≤1, EOA-self-owner gated),
    // matching the surface op-revm's `validate_env` accepts. State checks
    // (lock window, ACCOUNT_CONFIG deployed, authorizer chain, Create
    // collision) run once the StateProvider is open at Step 12+.
    let mut create_first_entry_violation = false;
    let mut delegation_count = 0usize;
    let mut seen_non_create = false;
    for entry in &tx.account_changes {
        match entry {
            // Create: the parser flags `create_not_first_entry` on every
            // non-leading Create; we mirror it here so the rejection lands
            // before any state read. EIP-170 size cap fires as soon as the
            // Create is encountered.
            AccountChangeEntry::Create(c) => {
                if seen_non_create {
                    create_first_entry_violation = true;
                }
                if c.bytecode.len() > op_revm::revm::primitives::eip170::MAX_CODE_SIZE {
                    return Err(Eip8130ValidationError::CreateBytecodeTooLarge {
                        size: c.bytecode.len(),
                        limit: op_revm::revm::primitives::eip170::MAX_CODE_SIZE,
                    });
                }
            }
            AccountChangeEntry::ConfigChange(cc) => {
                seen_non_create = true;
                // P3 carve-out (empty `owner_changes` AND empty
                // `authorizer_auth`): pure-sequence entry — admit. Reading
                // op-revm `handler.rs:1058` literally would reject this
                // (the parser flags `matching_config_change_with_zero_valid_ops`
                // for any entry with no known op codes), but P3 admission
                // explicitly opens this shape on the assumption the
                // executor's pure-sequence path stays compatible. P4
                // preserves that invariant.
                //
                // Strict-spec rejection: a matching entry with non-empty
                // `owner_changes` whose only ops carry unknown
                // `change_type` codes still bumps sequence + runs
                // authorizer validation without producing any
                // `_ownerConfig` write — useful only for replay shaping.
                // Mirror op-revm `handler.rs:1058-1062`'s flag rejection
                // for that sub-case.
                let targets_us = cc.chain_id == 0 || cc.chain_id == tx.chain_id;
                if targets_us && !cc.owner_changes.is_empty() {
                    let has_known_op = cc
                        .owner_changes
                        .iter()
                        .any(|op| matches!(op.change_type, OP_AUTHORIZE_OWNER | OP_REVOKE_OWNER));
                    if !has_known_op {
                        return Err(Eip8130ValidationError::EmptyConfigChange);
                    }
                }
            }
            AccountChangeEntry::Delegation(_) => {
                seen_non_create = true;
                delegation_count = delegation_count.saturating_add(1);
                if delegation_count > 1 {
                    return Err(Eip8130ValidationError::MultipleDelegationEntries);
                }
            }
        }
    }
    if create_first_entry_violation {
        return Err(Eip8130ValidationError::CreateNotFirstEntry);
    }

    // Step 4: open state provider.
    let state = client.latest().map_err(|e| Eip8130ValidationError::StateError(e.to_string()))?;

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
        // Sequenced branch: `aa_nonce_slot(sender, nonce_key)` of NONCE_MANAGER is the
        // lane's next-expected nonce. Admission accepts any `nonce_sequence >= on_chain`
        // — the side pool's queued bucket parks future-nonce txs until the gap fills —
        // and rejects only stale (`< on_chain`) txs. Treating gaps as `NonceMismatch`
        // here would defeat the 2D pool's queued path entirely (the side pool never
        // sees the future-nonce admissions it's designed to hold).
        let slot_u256 = op_revm::precompiles_xlayer::aa_nonce_slot(sender, tx.nonce_key);
        let storage_key = B256::from(slot_u256.to_be_bytes());
        let on_chain = state
            .storage(NONCE_MANAGER_ADDRESS, storage_key)
            .map_err(|e| Eip8130ValidationError::StateError(e.to_string()))?
            .unwrap_or_default()
            .to::<u64>();
        if tx.nonce_sequence < on_chain {
            return Err(Eip8130ValidationError::NonceMismatch {
                expected: on_chain,
                got: tx.nonce_sequence,
            });
        }
        on_chain
    };

    // Step 8: AA intrinsic-gas computation. Use the same canonical
    // `Eip8130Parts` builder the execution layer runs, so admission and
    // execution agree on `account_changes_cost` (Create code placement,
    // ConfigChange config_writes, Delegation, authorizer SLOADs, etc.).
    // A partial `Eip8130Parts` with a zeroed `account_changes` block
    // would under-count intrinsic gas for any tx widening admission past
    // the self-pay / no-account_changes shape and let txs through that
    // the executor will reject for intrinsic-gas underestimation.
    let parts = alloy_op_evm::eip8130::eip8130_parts(tx, sender);
    let gas_params = xlayer_gas_params(OpSpecId::XLAYER_V1);
    let intrinsic_gas = aa_intrinsic_gas(&parts, &gas_params);
    if tx.gas_limit < intrinsic_gas {
        return Err(Eip8130ValidationError::IntrinsicGasTooLow {
            required: intrinsic_gas,
            gas_limit: tx.gas_limit,
        });
    }

    // Step 9: sender authorization — re-validate `Native` outcomes against on-chain
    // `owner_config[sender][owner_id]`. Mirrors the predicate the handler runs at
    // execution time in `validate_owner_against_effective_config` (op-revm
    // `handler.rs:201`); admitting a tx whose owner has been revoked / repointed
    // since the resolver ran would let it consume gossip bandwidth only to fail at
    // inclusion. `Deferred` (custom verifier) admits without an EVM round-trip
    // here — the handler runs the STATICCALL at execution. Doing the STATICCALL
    // at admission is a future option (P5) but out of scope for P1.
    //
    // Delegate→Native: the executor's `dispatch_auth_state` runs TWO owner_config
    // reads (op-revm `handler.rs:676-710`) — outer
    // `owner_config[sender][bytes32(bytes20(delegate_address))]` AND inner
    // `owner_config[delegate_address][delegate_inner.owner_id]`. We mirror both
    // here so a Delegate→Native tx whose inner registration is missing or
    // repointed on chain is rejected at admission instead of consuming gossip
    // bandwidth only to fail at inclusion.
    if let AuthState::Native { verifier, owner_id, delegate_inner } = &sender_auth {
        validate_native_auth_owner_config_slot(
            state.as_ref(),
            sender,
            *owner_id,
            *verifier,
            OWNER_SCOPE_SENDER,
        )?;
        // Delegate→Native inner: read `owner_config[delegate_address]
        // [delegate_inner.owner_id]` and validate against `delegate_inner.verifier`.
        // The handler does the same at `handler.rs:697-708`.
        if let Some(inner) = delegate_inner {
            let delegate_address = Address::from_slice(&owner_id.0[..20]);
            validate_native_auth_owner_config_slot(
                state.as_ref(),
                delegate_address,
                inner.owner_id,
                inner.verifier,
                OWNER_SCOPE_SENDER,
            )?;
        }
    }

    // Step 9b: payer authorization re-check (sponsored shapes only). Mirrors
    // the sender-side block above but keyed on the payer's owner_config row
    // and the PAYER scope bit. Skipped when `effective_payer == sender` —
    // rule A already covers that side. Deferred payer auth (custom verifier)
    // skips the slot read; the executor runs the STATICCALL at execution time.
    //
    // The implicit-EOA fallback applies symmetrically: the executor's
    // `validate_native_verifier_owner` (op-revm `handler.rs:303`) calls into
    // `validate_owner_against_effective_config` with `allow_implicit_eoa = true`
    // for both sides — an empty payer slot with K1 + `bytes20(payer)` is
    // admitted under the same rule.
    //
    // Delegate→Native inner: same two-slot dance as the sender side
    // (op-revm `handler.rs:697-708`), but using OWNER_SCOPE_PAYER and the
    // payer's effective address.
    if effective_payer != sender {
        if let AuthState::Native { verifier, owner_id, delegate_inner } = &payer_auth {
            validate_native_auth_owner_config_slot(
                state.as_ref(),
                effective_payer,
                *owner_id,
                *verifier,
                OWNER_SCOPE_PAYER,
            )?;
            if let Some(inner) = delegate_inner {
                let delegate_address = Address::from_slice(&owner_id.0[..20]);
                validate_native_auth_owner_config_slot(
                    state.as_ref(),
                    delegate_address,
                    inner.owner_id,
                    inner.verifier,
                    OWNER_SCOPE_PAYER,
                )?;
            }
        }
        // Deferred / SelfPay / Empty / Invalid: nothing to do here. SelfPay is
        // unreachable on this branch (`effective_payer != sender` implies
        // `tx.payer = Some(distinct_addr)` which routes through the resolver's
        // sponsored path, never SelfPay). Empty/Invalid were already rejected
        // at Step 10.
    }

    // Step 11: balance check. AA-aware lower bound is
    // `gas_limit * max_fee_per_gas + l1_data_fee`, keyed on the payer.
    //
    // The OP wrapper's `apply_op_checks` skips the L1 guard for AA txs (see
    // `OpTransactionValidator::apply_op_checks`) because that guard reads
    // the envelope-signer's balance, which is the wrong account on
    // sponsored shapes. The fee component is folded in here against the
    // resolved payer instead.
    let balance = state
        .account_balance(&payer)
        .map_err(|e| Eip8130ValidationError::StateError(e.to_string()))?
        .unwrap_or_default();
    let max_gas_cost = U256::from(tx.gas_limit)
        .saturating_mul(U256::from(tx.max_fee_per_gas))
        .saturating_add(l1_data_fee);
    if balance < max_gas_cost {
        return Err(Eip8130ValidationError::InsufficientBalance {
            required: max_gas_cost,
            available: balance,
        });
    }

    // Step 12: account_state validation — sequence_updates pre-state
    // (rule D, spec line 376) AND lock-window check (rule C, spec line
    // 511). Both halves come from the same packed slot
    // (`aa_lock_slot(sender)`), so we read it once and dispatch.
    //
    // Lock-sensitive shape mirrors op-revm `check_account_lock`
    // (`handler.rs:343-370`), which fires when
    // `delegation_target.is_some() || !config_writes.is_empty() ||
    // !sequence_updates.is_empty()`. ANY ConfigChange entry produces a
    // `sequence_updates` slot in `Eip8130Parts` (see
    // `process_config_change_entry`), so pure-sequence ConfigChange
    // (empty `owner_changes` AND empty `authorizer_auth`) IS
    // lock-sensitive at execution: spec line 511 requires reject when
    // the account is locked. Admitting these in a locked window would
    // let the mempool ship txs the executor will reject for
    // AccountLocked.
    // Collapse the per-shape predicates into one pass over
    // `account_changes` (cf PERF-14). The per-entry validation loops
    // further down still need entry-by-entry access, so they stay
    // separate; only the boolean predicates merge here.
    let mut has_create_entry = false;
    let mut has_delegation_entry = false;
    let mut has_config_writes = false;
    let mut has_any_config_change = false;
    for entry in &tx.account_changes {
        match entry {
            AccountChangeEntry::Create(_) => has_create_entry = true,
            AccountChangeEntry::Delegation(_) => has_delegation_entry = true,
            AccountChangeEntry::ConfigChange(cc) => {
                has_any_config_change = true;
                if !cc.owner_changes.is_empty() || !cc.authorizer_auth.is_empty() {
                    has_config_writes = true;
                }
            }
        }
    }
    let lock_sensitive = has_create_entry || has_delegation_entry || has_any_config_change;

    if lock_sensitive {
        let seq_slot = aa_lock_slot(sender);
        let seq_storage_key = B256::from(seq_slot.to_be_bytes());
        let packed = state
            .storage(ACCOUNT_CONFIG_ADDRESS, seq_storage_key)
            .map_err(|e| Eip8130ValidationError::StateError(e.to_string()))?
            .unwrap_or_default();

        // Lock window: reject when block_timestamp < unlocks_at. The
        // packed word's high bytes carry `unlocks_at` (uint40); op-revm
        // `unlocks_at_from_account_state_word` is the canonical decoder.
        let unlocks_at = unlocks_at_from_account_state_word(packed);
        if block_timestamp < unlocks_at {
            return Err(Eip8130ValidationError::AccountLocked { unlocks_at });
        }

        // Sequence pre-state (existing P3 logic). Mirrors op-revm
        // `handler.rs:483-519`: maintain running per-half expected
        // pre-values starting from on-chain, and each in-tx update bumps
        // the running expected by 1 (so a tx with two multichain entries
        // pins pre=N, then N+1).
        if has_any_config_change {
            let mut expected_mc = read_packed_sequence(packed, true);
            let mut expected_local = read_packed_sequence(packed, false);
            for entry in &tx.account_changes {
                if let AccountChangeEntry::ConfigChange(cc) = entry {
                    // Skip entries that don't target us — executor's
                    // `change_targets_us` (alloy-op-evm `parts.rs:389`)
                    // drops them silently from `sequence_updates`.
                    let targets_us = cc.chain_id == 0 || cc.chain_id == tx.chain_id;
                    if !targets_us {
                        continue;
                    }
                    // `new_value = sequence + 1` — guard against u64 wrap.
                    // Executor catches via `checked_sub(1)` on `new_value`
                    // (op-revm `handler.rs:501-504`).
                    if cc.sequence == u64::MAX {
                        return Err(Eip8130ValidationError::SequenceUpdateUnderflow);
                    }
                    let is_multichain = cc.chain_id == 0;
                    let running =
                        if is_multichain { &mut expected_mc } else { &mut expected_local };
                    if cc.sequence != *running {
                        return Err(Eip8130ValidationError::SequenceMismatch {
                            is_multichain,
                            expected_pre: *running,
                            got_pre: cc.sequence,
                        });
                    }
                    *running = running.saturating_add(1);
                }
            }
        }
    }

    // Step 12b: ACCOUNT_CONFIG deployment check. Mirrors op-revm
    // `validate_config_change_preconditions` (`handler.rs:470-481`):
    // when the tx carries config writes, the AccountConfiguration
    // contract MUST be deployed (its account `code_hash !=
    // keccak256([])`). The executor caches this via an `AtomicBool`
    // global so subsequent inclusions skip the read; mempool
    // admission re-checks per call (cheap; one basic_account read).
    // Pure-sequence config changes (no `owner_changes`) do NOT require
    // ACCOUNT_CONFIG deployment — they only touch `_accountState`, not
    // `_ownerConfig`, so `has_seq_updates` alone short-circuits the
    // executor's helper at `handler.rs:483-485`. They are still
    // lock-sensitive at Step 12 above, since `check_account_lock` fires
    // on any non-empty `sequence_updates`.
    if has_config_writes {
        let acc = state
            .basic_account(&ACCOUNT_CONFIG_ADDRESS)
            .map_err(|e| Eip8130ValidationError::StateError(e.to_string()))?;
        let empty_code_hash = keccak256([]);
        let has_code = match acc {
            Some(a) => a.bytecode_hash.is_some_and(|h| h != empty_code_hash),
            None => false,
        };
        if !has_code {
            return Err(Eip8130ValidationError::AccountConfigNotDeployed);
        }
    }

    // Step 12c: Create entry replay-collision check. Mirrors op-revm
    // `handler.rs:1516-1552`: the deployed address derives from
    // `(deployer, user_salt, bytecode, sorted owners)` — not bound to
    // sender — so an already-on-chain Create tuple could be re-submitted
    // and overwrite the deployed account's code / owner_config. The
    // EIP-684-style guard requires:
    //   (a) target's `code_hash == keccak256([])` AND `nonce == 0`;
    //   (b) every `pre_writes` slot (the initial-owner `_ownerConfig`
    //       slots) currently zero on chain.
    //
    // Balance is intentionally not checked (handler comment block
    // at handler.rs:1507-1515): pre-funding a CREATE2 address is a
    // legitimate counterfactual-funding pattern; checking balance
    // would let a 1-wei DoS block any Create.
    for entry in &tx.account_changes {
        if let AccountChangeEntry::Create(c) = entry {
            let derived = derive_account_address(
                ACCOUNT_CONFIG_ADDRESS,
                c.user_salt,
                &c.bytecode,
                &c.initial_owners,
            );
            let acc = state
                .basic_account(&derived)
                .map_err(|e| Eip8130ValidationError::StateError(e.to_string()))?;
            let empty_code_hash = keccak256([]);
            let collision = match acc {
                Some(a) => a.nonce != 0 || a.bytecode_hash.is_some_and(|h| h != empty_code_hash),
                None => false,
            };
            if collision {
                return Err(Eip8130ValidationError::CreateTargetCollision { address: derived });
            }
            // pre_writes slots: each initial owner registers
            // `owner_config[derived][owner.owner_id] = (verifier, scope)`.
            // Mirror the parser's slot derivation
            // (`alloy-op-evm/src/eip8130/parts.rs:294-300` via
            // `aa_owner_config_slot`).
            for owner in &c.initial_owners {
                let slot = aa_owner_config_slot(derived, U256::from_be_bytes(owner.owner_id.0));
                let key = B256::from(slot.to_be_bytes());
                let current = state
                    .storage(ACCOUNT_CONFIG_ADDRESS, key)
                    .map_err(|e| Eip8130ValidationError::StateError(e.to_string()))?
                    .unwrap_or_default();
                if !current.is_zero() {
                    return Err(Eip8130ValidationError::CreateOwnerSlotOccupied {
                        address: derived,
                        slot,
                    });
                }
            }
        }
    }

    // Step 12d: authorizer-chain validation (Native). Mirrors op-revm
    // `validate_authorizer_chain` (`handler.rs:765-844`). For each
    // ConfigChange entry that targets us:
    //   - parse the `authorizer_auth` blob (verifier prefix + payload);
    //   - if Delegate verifier or malformed → mirror the parser's fallback (verifier=ZERO,
    //     owner_id=ZERO) which the executor catches via the zero-owner_id reject
    //     (`handler.rs:818-822`);
    //   - if K1 native verify recovers a non-zero `owner_id` → re-validate `(sender, owner_id, K1,
    //     OWNER_SCOPE_CONFIG)` against on-chain `owner_config` with the per-iteration
    //     `pending_overlay` (so a later authorizer can validate against an owner the earlier
    //     ConfigChange's `owner_changes` just authorized in this same tx);
    //   - if custom verifier (Native verify returned Unsupported) → admit deferred (P5 = STATICCALL
    //     admission). The executor runs the STATICCALL at inclusion.
    //   - non-K1 Native verifiers (P256-raw, WebAuthn) require a real keypair to produce a valid
    //     signature; for admission we treat them like Custom (admit-and-defer to the executor)
    //     since the test surface for those is in the executor's domain.
    //
    // Pending-overlay shape: `HashMap<U256, PendingOwnerState>`
    // accumulates owner_changes as we walk validations in order. Each
    // ConfigChange's owner_changes are pushed AFTER its authorizer
    // validates; a later validation observes prior changes.
    let mut pending_overlay: HashMap<U256, PendingOwnerState> = HashMap::new();
    for entry in &tx.account_changes {
        let AccountChangeEntry::ConfigChange(cc) = entry else { continue };
        let targets_us = cc.chain_id == 0 || cc.chain_id == tx.chain_id;
        if !targets_us {
            continue;
        }
        // The parser emits an authorizer_validation slot only for
        // entries that target us. Even if `authorizer_auth.is_empty()`,
        // the parser produces a `verifier=ZERO, owner_id=ZERO`
        // placeholder which the executor's zero-owner_id check
        // rejects (`handler.rs:818-822`). Mirror that here only if the
        // entry actually carries config writes — pure-sequence entries
        // don't run authorizer validation in the executor (their
        // `authorizer_validations` slot has no `verify_call` AND
        // `owner_id == 0` AND `owner_changes.is_empty()`, so the
        // executor's `skip` branch (`handler.rs:794-800`) takes over).
        let has_writes = !cc.owner_changes.is_empty() || !cc.authorizer_auth.is_empty();
        if !has_writes {
            // Pure-sequence carve-out — no authorizer validation. Skip.
            continue;
        }

        // Resolve the authorizer's owner_id (Native eager verify only;
        // Custom defers to executor STATICCALL).
        let resolved = resolve_authorizer_owner(sender, cc);
        match resolved {
            AuthorizerResolution::Native { verifier, owner_id } => {
                if owner_id.is_zero() {
                    return Err(Eip8130ValidationError::AuthorizerInvalidOwnerId);
                }
                validate_authorizer_owner_against_state(
                    state.as_ref(),
                    sender,
                    owner_id,
                    verifier,
                    &pending_overlay,
                )?;
            }
            AuthorizerResolution::DeferredCustom => {
                // STATICCALL at admission deferred. The executor
                // runs `validate_authorizer_chain`'s STATICCALL path
                // at inclusion (op-revm `handler.rs:802-816`).
            }
            AuthorizerResolution::Malformed => {
                // Parser fallback: verifier=ZERO / owner_id=ZERO. The
                // executor's zero-owner_id check rejects (handler.rs:818).
                // We surface as `AuthorizerInvalidOwnerId` so the peer
                // is penalized for forwarding a malformed auth blob.
                return Err(Eip8130ValidationError::AuthorizerInvalidOwnerId);
            }
        }

        // Apply this entry's owner_changes to the overlay so the next
        // validation in the chain can authorize against pending state.
        // Mirrors op-revm `handler.rs:834-840`'s `pending_owner_state_for_change`.
        for op in &cc.owner_changes {
            if let Some(state) =
                pending_owner_state_for_change(op.change_type, op.verifier, op.scope)
            {
                pending_overlay.insert(U256::from_be_bytes(op.owner_id.0), state);
            }
        }
    }

    // Step 12e: Delegation gating. Mirrors op-revm
    // `check_delegation_requires_eoa_config_owner` (`handler.rs:384-441`).
    // When a Delegation entry is present:
    //   (a) sender's resolved AuthState MUST be Native with
    //       `owner_id == bytes32(bytes20(sender))` (the EOA
    //       self-owner pattern). Custom verifier / non-K1 Native /
    //       Empty / Invalid all fail the structural check.
    //   (b) That owner row MUST carry `OWNER_SCOPE_CONFIG`, honoring
    //       the implicit-EOA fallback (verifier=ZERO + K1 + bytes20
    //       owner_id) AND the pending overlay (so a same-tx
    //       ConfigChange that just authorized the EOA self-owner
    //       with CONFIG scope can satisfy the check).
    if has_delegation_entry {
        // (a) Structural check: K1 Native + EOA self-owner.
        let eoa_owner_id = {
            let mut buf = [0u8; 32];
            buf[..20].copy_from_slice(sender.as_slice());
            B256::from(buf)
        };
        let eoa_owner_id_u = U256::from_be_bytes(eoa_owner_id.0);
        let is_eoa_self_owner = matches!(
            &sender_auth,
            AuthState::Native { verifier, owner_id, .. }
                if *verifier == K1_VERIFIER_ADDRESS && *owner_id == eoa_owner_id
        );
        if !is_eoa_self_owner {
            return Err(Eip8130ValidationError::DelegationRequiresEoaSelfOwner);
        }

        // (b) CONFIG-scope check against effective state (overlay-aware).
        // Sender's K1 EOA self-owner row may be empty on-chain (implicit
        // EOA), authorized via this tx's authorizer_auth, or already
        // registered with CONFIG scope. The executor's helper allows
        // the implicit-EOA fallback only when verifier=ZERO AND K1 AND
        // bytes20 owner_id — which is the case here.
        validate_delegation_owner_scope(state.as_ref(), sender, eoa_owner_id_u, &pending_overlay)?;
    }

    // Step 13: invalidation rules. The side pool derives currently-admitted
    // invalidation rules via `Eip8130PoolTx::aa_invalidation_rules`
    // (rule A: sender owner_config; rule B as of P2: payer owner_config for
    // sponsored shapes; rule D as of P3: per-half sequence binding via
    // `SeqExpect`). Widening admission turns on the remaining rules — none
    // of which require changes to this validator, only to
    // `aa_invalidation_rules`:
    //   * `account_changes` carrying lock-sensitive ops (rule C): set `lock_sensitive: true` on the
    //     `AccountState` entry at `aa_lock_slot(sender)` so a still-locked sender evicts.

    // `max_gas_cost` (the admission-time payer-balance predicate) is recomputed
    // at the side-pool admission site from `compute_l1_data_fee` + tx fields;
    // no need to surface it here. F5.
    Ok(state_nonce)
}

#[cfg(test)]
mod xlayer_tests {
    use super::*;
    use alloy_primitives::{Address, B256, Bytes, U256};
    use alloy_signer::SignerSync;
    use alloy_signer_local::PrivateKeySigner;
    use op_alloy_consensus::{ConfigChangeEntry, Eip8130CallEntry, TxEip8130};
    use op_revm::constants::{K1_VERIFIER_ADDRESS, OWNER_SCOPE_PAYER, OWNER_SCOPE_SENDER};
    use reth_optimism_primitives::OpPrimitives;
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};

    /// Deterministic K1 signer used as the AA sender across post-Step-3 tests.
    fn signer_for(seed: u8) -> PrivateKeySigner {
        PrivateKeySigner::from_bytes(&B256::repeat_byte(seed)).expect("valid K1 key")
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
    /// Resolver yields `Native { verifier = K1, owner_id = bytes20(signer) }`.
    fn k1_explicit_auth(signer: &PrivateKeySigner, hash: B256) -> Bytes {
        let mut buf = Vec::with_capacity(85);
        buf.extend_from_slice(K1_VERIFIER_ADDRESS.as_slice());
        buf.extend_from_slice(&k1_sig_blob(signer, hash));
        Bytes::from(buf)
    }

    /// Implicit-EOA `owner_id = bytes32(bytes20(account))`.
    fn implicit_owner_id(account: Address) -> B256 {
        let mut buf = [0u8; 32];
        buf[..20].copy_from_slice(account.as_slice());
        B256::from(buf)
    }

    /// Packs `(verifier, scope)` into the on-chain `owner_config` word layout
    /// `parse_owner_config_word` decodes (op-revm `handler.rs:118`).
    fn pack_owner_config(verifier: Address, scope: u8) -> U256 {
        let mut bytes = [0u8; 32];
        bytes[11] = scope;
        bytes[12..32].copy_from_slice(verifier.as_slice());
        U256::from_be_bytes(bytes)
    }

    /// Builds an explicit-from K1 AA tx whose `sender_auth` is a real K1 sig over
    /// `sender_signature_hash`. The on-chain owner_config slot is empty by default in
    /// `fresh_client`, so admission relies on the implicit-EOA fallback (K1 +
    /// `owner_id = bytes20(sender)`) — same shape the prior `make_tx` exercised
    /// post-recover. Tests that pin specific post-Step-3 errors need to call this
    /// instead of constructing `TxEip8130` directly.
    fn make_signed_tx(chain_id: u64, signer: &PrivateKeySigner, nonce_sequence: u64) -> TxEip8130 {
        let mut tx = TxEip8130 {
            chain_id,
            from: Some(signer.address()),
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
        };
        let hash = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(signer, hash);
        tx
    }

    /// Back-compat shim for the structural pre-Step-3 tests (chain_id /
    /// expired / too-many-calls / oversized-tx). These are rejected before
    /// the auth dispatch fires, so a real K1 sig isn't required — but we
    /// keep the same signature so test changes are surgical.
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
        let signer = signer_for(0x11);
        let sender = signer.address();

        let client = MockEthProvider::<OpPrimitives>::new();
        client.add_account(NONCE_MANAGER_ADDRESS, ExtendedAccount::new(0, U256::ZERO));
        client.add_account(sender, ExtendedAccount::new(0, U256::from(1_000_000_000_u64)));

        let tx = make_signed_tx(CHAIN_ID, &signer, 0);
        let state_nonce = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect("self-pay AA tx must pass mempool validation");

        // Validator now returns just the on-chain `state_nonce`; the side-pool
        // admission site recomputes `required_balance` from tx fields plus
        // `OpAaTransactionValidator::compute_l1_data_fee`.
        assert_eq!(state_nonce, 0);
        let _ = sender; // resolved from `signer.address()` for fixture construction only
    }

    fn fresh_client(sender: Address) -> MockEthProvider<OpPrimitives> {
        let client = MockEthProvider::<OpPrimitives>::new();
        client.add_account(NONCE_MANAGER_ADDRESS, ExtendedAccount::new(0, U256::ZERO));
        client.add_account(sender, ExtendedAccount::new(0, U256::from(1_000_000_000_u64)));
        client
    }

    /// Spec: encoded tx exceeding `MAX_AA_TX_ENCODED_BYTES` is rejected
    /// before any further work (no state read).
    #[test]
    fn rejects_oversized_tx() {
        const CHAIN_ID: u64 = 10;
        let sender = Address::repeat_byte(0x11);
        let client = fresh_client(sender);

        let tx = make_tx(CHAIN_ID, sender, 0);
        let err = validate_eip8130_transaction(
            &tx,
            MAX_AA_TX_ENCODED_BYTES + 1,
            0,
            CHAIN_ID,
            &client,
            U256::ZERO,
        )
        .expect_err("tx exceeding mempool size cap must be rejected");
        assert!(matches!(err, Eip8130ValidationError::TxTooLarge { .. }));
    }

    /// Spec: chain_id mismatch is rejected. The AA branch short-circuits
    /// the mainnet validate_env so the mempool re-asserts.
    #[test]
    fn rejects_chain_id_mismatch() {
        const CHAIN_ID: u64 = 10;
        let sender = Address::repeat_byte(0x11);
        let client = fresh_client(sender);

        // Tx claims chain_id=999 but pool is configured for 10.
        let tx = make_tx(999, sender, 0);
        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("chain_id mismatch must be rejected");
        assert!(matches!(err, Eip8130ValidationError::ChainIdMismatch { .. }));
    }

    /// Spec: a non-zero `expiry` in the past rejects the tx with `Expired`.
    /// `expiry == 0` is the "no expiry" sentinel and bypasses this check
    /// (tested by the happy path).
    #[test]
    fn rejects_expired_tx() {
        const CHAIN_ID: u64 = 10;
        let sender = Address::repeat_byte(0x11);
        let client = fresh_client(sender);

        let mut tx = make_tx(CHAIN_ID, sender, 0);
        tx.expiry = 100;
        // current block timestamp 200 > expiry 100 → expired.
        let err = validate_eip8130_transaction(&tx, 1024, 200, CHAIN_ID, &client, U256::ZERO)
            .expect_err("expired tx must be rejected");
        assert!(matches!(err, Eip8130ValidationError::Expired { .. }));
    }

    /// Spec: more than `MAX_CALLS_PER_TX` calls across all phases is
    /// rejected as a structural bound violation.
    #[test]
    fn rejects_too_many_calls() {
        const CHAIN_ID: u64 = 10;
        let sender = Address::repeat_byte(0x11);
        let client = fresh_client(sender);

        let mut tx = make_tx(CHAIN_ID, sender, 0);
        // Pack one phase with calls beyond the cap.
        let limit = op_revm::constants::MAX_CALLS_PER_TX;
        tx.calls =
            vec![vec![Eip8130CallEntry { to: Address::ZERO, data: Default::default() }; limit + 1]];
        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("over-cap call count must be rejected");
        assert!(matches!(err, Eip8130ValidationError::TooManyCalls { .. }));
    }

    /// Cap covers execution-unit count, not top-level entries. A single
    /// ConfigChange with `MAX + 1` owner_changes is `MAX + 1` units (mirrors
    /// `alloy_op_evm::eip8130::account_change_units`) so it must reject —
    /// before this fix it slipped through the top-level-len check.
    #[test]
    fn rejects_config_change_with_unit_count_exceeding_cap() {
        use op_alloy_consensus::OwnerChange;
        use op_revm::constants::{OP_AUTHORIZE_OWNER, OWNER_SCOPE_CONFIG};
        const CHAIN_ID: u64 = 10;
        let sender = Address::repeat_byte(0x11);
        let client = fresh_client(sender);

        let limit = op_revm::constants::MAX_ACCOUNT_CHANGES_PER_TX;
        let owner_changes = (0..limit + 1)
            .map(|i| OwnerChange {
                change_type: OP_AUTHORIZE_OWNER,
                owner_id: B256::repeat_byte((i & 0xff) as u8),
                verifier: K1_VERIFIER_ADDRESS,
                scope: OWNER_SCOPE_CONFIG,
            })
            .collect();
        let mut tx = make_tx(CHAIN_ID, sender, 0);
        tx.account_changes = vec![AccountChangeEntry::ConfigChange(ConfigChangeEntry {
            chain_id: CHAIN_ID,
            sequence: 0,
            owner_changes,
            authorizer_auth: Default::default(),
        })];

        let err = validate_eip8130_transaction(&tx, 8192, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("over-cap ConfigChange units must reject");
        match err {
            Eip8130ValidationError::TooManyAccountChanges { count, limit: cap } => {
                assert_eq!(count, limit + 1);
                assert_eq!(cap, limit);
            }
            other => panic!("expected TooManyAccountChanges, got {other:?}"),
        }
    }

    /// Boundary: Create with `MAX - 1` initial_owners is exactly `MAX` units
    /// (Create itself contributes 1). The cap check uses `>`, so this must
    /// not trip `TooManyAccountChanges`. Asserts the counter, not full
    /// admission — the rest of the pipeline (auth, state) needs heavier
    /// fixtures already covered elsewhere.
    #[test]
    fn admits_create_with_initial_owners_under_cap() {
        use op_alloy_consensus::CreateEntry;
        let limit = op_revm::constants::MAX_ACCOUNT_CHANGES_PER_TX;
        let owners = (0..limit - 1)
            .map(|i| op_alloy_consensus::Owner {
                owner_id: B256::repeat_byte((i & 0xff) as u8),
                verifier: K1_VERIFIER_ADDRESS,
                scope: OWNER_SCOPE_SENDER,
            })
            .collect();
        let mut tx = make_tx(10, Address::repeat_byte(0x11), 0);
        tx.account_changes = vec![AccountChangeEntry::Create(CreateEntry {
            user_salt: B256::ZERO,
            bytecode: Bytes::from_static(&[0x60, 0x00]),
            initial_owners: owners,
        })];

        let units = alloy_op_evm::eip8130::account_change_units(&tx);
        assert_eq!(units, limit, "Create with MAX-1 owners = MAX units");
        assert!(units <= limit, "must not exceed cap");
    }

    /// Cross-check: admission's unit counter equals execution's count.
    /// Asserts the helper we call (`account_change_units`) is the same one
    /// the handler reads from `parts.account_changes.account_change_units`.
    #[test]
    fn unit_counter_matches_execution() {
        use op_alloy_consensus::{CreateEntry, DelegationEntry, OwnerChange};
        use op_revm::constants::{OP_AUTHORIZE_OWNER, OWNER_SCOPE_CONFIG};

        // Mixed: Create(1 + 2 owners) + ConfigChange(3 ops) + Delegation(1) = 7
        let entries = vec![
            AccountChangeEntry::Create(CreateEntry {
                user_salt: B256::ZERO,
                bytecode: Bytes::from_static(&[0x60, 0x00]),
                initial_owners: vec![
                    op_alloy_consensus::Owner {
                        owner_id: B256::repeat_byte(0xA1),
                        verifier: K1_VERIFIER_ADDRESS,
                        scope: OWNER_SCOPE_SENDER,
                    },
                    op_alloy_consensus::Owner {
                        owner_id: B256::repeat_byte(0xA2),
                        verifier: K1_VERIFIER_ADDRESS,
                        scope: OWNER_SCOPE_SENDER,
                    },
                ],
            }),
            AccountChangeEntry::ConfigChange(ConfigChangeEntry {
                chain_id: 10,
                sequence: 0,
                owner_changes: vec![
                    OwnerChange {
                        change_type: OP_AUTHORIZE_OWNER,
                        owner_id: B256::repeat_byte(0xB1),
                        verifier: K1_VERIFIER_ADDRESS,
                        scope: OWNER_SCOPE_CONFIG,
                    },
                    OwnerChange {
                        change_type: OP_AUTHORIZE_OWNER,
                        owner_id: B256::repeat_byte(0xB2),
                        verifier: K1_VERIFIER_ADDRESS,
                        scope: OWNER_SCOPE_CONFIG,
                    },
                    OwnerChange {
                        change_type: OP_AUTHORIZE_OWNER,
                        owner_id: B256::repeat_byte(0xB3),
                        verifier: K1_VERIFIER_ADDRESS,
                        scope: OWNER_SCOPE_CONFIG,
                    },
                ],
                authorizer_auth: Default::default(),
            }),
            AccountChangeEntry::Delegation(DelegationEntry { target: Address::repeat_byte(0xCD) }),
        ];
        let mut tx = make_tx(10, Address::repeat_byte(0x11), 0);
        tx.account_changes = entries;

        let units = alloy_op_evm::eip8130::account_change_units(&tx);
        assert_eq!(units, 1 + 2 + 3 + 1, "expected 7 units, got {units}");

        // Through `eip8130_parts`: the handler reads
        // `parts.account_changes.account_change_units` (op-revm
        // `handler.rs:1025`). Asserting both sides converge proves admission
        // and execution share the same counter.
        let parts = alloy_op_evm::eip8130::eip8130_parts(&tx, Address::repeat_byte(0x11));
        assert_eq!(parts.account_changes.account_change_units, units);
    }

    /// Spec: AA tx gas_limit must cover op-revm's EIP-8130 intrinsic gas.
    #[test]
    fn rejects_aa_intrinsic_gas_too_low() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x11);
        let sender = signer.address();
        let client = fresh_client(sender);

        // Build a signed tx, then knock gas_limit below intrinsic. Re-signing
        // isn't required — `gas_limit` is part of `sender_signature_hash`, but
        // the intrinsic-gas check fires after Step 3 either way; the resolver
        // only needs the original sig to be well-formed against the *as-built*
        // hash. Mutating `gas_limit` post-sign breaks the resolver's binding,
        // so we re-sign instead to keep Step 3 passing and Step 8 firing.
        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.gas_limit = 1;
        let hash = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(&signer, hash);
        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("gas_limit below AA intrinsic gas must be rejected");
        assert!(matches!(err, Eip8130ValidationError::IntrinsicGasTooLow { .. }));
    }

    /// Admission's intrinsic-gas computation must use the canonical
    /// `Eip8130Parts` builder (`alloy_op_evm::eip8130::eip8130_parts`)
    /// so `account_changes_cost` + `bytecode_cost` are billed for a
    /// Create entry. Under the previous partial-parts shape (which
    /// zeroed out `account_changes`), a `gas_limit` covering only the
    /// AA-base + payload would have admitted — letting the executor's
    /// initial-gas check reject downstream. Setting `gas_limit` between
    /// the old (no-create) intrinsic and the full intrinsic must reject
    /// with `IntrinsicGasTooLow`.
    #[test]
    fn intrinsic_gas_accounts_for_create_entry() {
        use op_alloy_consensus::CreateEntry;
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x71);
        let sender = signer.address();
        let client = fresh_client(sender);

        // Bytecode_cost = base(32_000) + 200 * len. 1024 bytes → 32_000 + 204_800
        // = 236_800 gas just for the deployed code, far above the
        // default 100_000 gas_limit make_signed_tx sets.
        let bytecode = vec![0x00u8; 1024];
        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.account_changes = vec![AccountChangeEntry::Create(CreateEntry {
            user_salt: B256::repeat_byte(0xC1),
            bytecode: Bytes::copy_from_slice(&bytecode),
            initial_owners: vec![],
        })];
        re_sign_sender_auth(&mut tx, &signer);

        // Below full intrinsic: rejects.
        let err = validate_eip8130_transaction(&tx, 4096, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("Create entry intrinsic gas must include bytecode_cost");
        match err {
            Eip8130ValidationError::IntrinsicGasTooLow { required, gas_limit } => {
                assert!(
                    required > gas_limit,
                    "required ({required}) must exceed gas_limit ({gas_limit})",
                );
                // Lower-bound sanity: the canonical builder bills at least
                // `bytecode_cost` (32_000 base + 200*1024) on top of AA
                // base + payload, which is far above 100_000.
                assert!(
                    required > 200_000,
                    "required gas should reflect Create cost, got {required}"
                );
            }
            other => panic!("expected IntrinsicGasTooLow, got {other:?}"),
        }

        // Above full intrinsic: admits.
        tx.gas_limit = 500_000;
        re_sign_sender_auth(&mut tx, &signer);
        validate_eip8130_transaction(&tx, 4096, 0, CHAIN_ID, &client, U256::ZERO)
            .expect("Create entry must admit when gas_limit covers full intrinsic");
    }

    /// Counterpart for Delegation entries: the canonical builder bills
    /// `aa_delegation_gas` per delegation, which the partial-parts shape
    /// silently dropped. Drives the regression by sizing `gas_limit` to
    /// admit the same tx with `account_changes = []` but reject once
    /// the Delegation entry is present.
    #[test]
    fn intrinsic_gas_accounts_for_delegation_entry() {
        use op_alloy_consensus::DelegationEntry;
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x72);
        let sender = signer.address();
        let client = fresh_client(sender);

        // First: discover the baseline intrinsic gas with no
        // account_changes. `make_signed_tx` builds a tx with default
        // gas_limit = 100_000, well above baseline; admit confirms.
        let baseline_tx = make_signed_tx(CHAIN_ID, &signer, 0);
        validate_eip8130_transaction(&baseline_tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect("baseline self-pay tx must admit");

        // Now: identical tx with a Delegation entry, sized so the
        // gap is precisely `aa_delegation_gas`. Reading the rejection's
        // `required` field tells us the exact full intrinsic gas.
        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.account_changes = vec![AccountChangeEntry::Delegation(DelegationEntry {
            target: Address::repeat_byte(0xCD),
        })];
        tx.gas_limit = 1; // forced under-spec to surface the required value.
        re_sign_sender_auth(&mut tx, &signer);

        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("under-specced gas_limit must reject");
        let required_with_delegation = match err {
            Eip8130ValidationError::IntrinsicGasTooLow { required, .. } => required,
            other => panic!("expected IntrinsicGasTooLow, got {other:?}"),
        };

        // Sanity: `aa_delegation_gas` (4_600 in XLAYER_V1) is non-zero,
        // so the full intrinsic must exceed the baseline. The partial-
        // parts shape would have produced `required ≈ baseline` (no
        // Delegation surcharge); the full builder produces `required ≥
        // baseline + 4_600`.
        let baseline_required = {
            let mut bare = make_signed_tx(CHAIN_ID, &signer, 0);
            bare.gas_limit = 1;
            re_sign_sender_auth(&mut bare, &signer);
            match validate_eip8130_transaction(&bare, 1024, 0, CHAIN_ID, &client, U256::ZERO) {
                Err(Eip8130ValidationError::IntrinsicGasTooLow { required, .. }) => required,
                other => panic!("expected IntrinsicGasTooLow on bare tx, got {other:?}"),
            }
        };
        assert!(
            required_with_delegation > baseline_required,
            "full builder must bill aa_delegation_gas on top of baseline ({required_with_delegation} vs {baseline_required})",
        );
    }

    /// Counterpart for ConfigChange-with-authorizer: the canonical
    /// builder runs `authorizer_verification_gas` (per-verifier gas +
    /// SLOAD) plus `aa_config_change_per_op_gas` per `owner_changes`
    /// op — none of which the partial-parts shape billed.
    #[test]
    fn intrinsic_gas_accounts_for_config_change_with_authorizer() {
        use op_alloy_consensus::OwnerChange;
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x73);
        let sender = signer.address();
        let client = fresh_client(sender);
        // Lock window 0 (unlocked), seq pre-values 0.
        seed_account_state(&client, sender, 0, 0, 0);

        // Baseline: same `make_signed_tx` with no account_changes,
        // under-specced gas_limit, surface required.
        let baseline_required = {
            let mut bare = make_signed_tx(CHAIN_ID, &signer, 0);
            bare.gas_limit = 1;
            re_sign_sender_auth(&mut bare, &signer);
            match validate_eip8130_transaction(&bare, 4096, 0, CHAIN_ID, &client, U256::ZERO) {
                Err(Eip8130ValidationError::IntrinsicGasTooLow { required, .. }) => required,
                other => panic!("expected IntrinsicGasTooLow on bare tx, got {other:?}"),
            }
        };

        // Authorizer-bearing ConfigChange with one owner_change op.
        // Sign authorizer over `config_change_digest(sender, change)`.
        let mut cc = ConfigChangeEntry {
            chain_id: 0,
            sequence: 0,
            owner_changes: vec![OwnerChange {
                change_type: OP_AUTHORIZE_OWNER,
                owner_id: B256::repeat_byte(0x55),
                verifier: K1_VERIFIER_ADDRESS,
                scope: OWNER_SCOPE_SENDER,
            }],
            authorizer_auth: Default::default(),
        };
        cc.authorizer_auth = k1_authorizer_auth(&signer, sender, &cc);

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.account_changes = vec![AccountChangeEntry::ConfigChange(cc)];
        tx.gas_limit = 1;
        re_sign_sender_auth(&mut tx, &signer);

        let err = validate_eip8130_transaction(&tx, 4096, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("ConfigChange entry intrinsic gas must reject under-specced gas_limit");
        let required_with_cc = match err {
            Eip8130ValidationError::IntrinsicGasTooLow { required, .. } => required,
            other => panic!("expected IntrinsicGasTooLow, got {other:?}"),
        };
        assert!(
            required_with_cc > baseline_required,
            "full builder must bill per-op + authorizer gas ({required_with_cc} vs {baseline_required})",
        );
    }

    /// Spec: nonce-free txs (`nonce_key == NONCE_KEY_MAX`) MUST have
    /// `expiry > 0` (replay-protection requires a non-zero window).
    #[test]
    fn rejects_nonce_free_zero_expiry() {
        const CHAIN_ID: u64 = 10;
        let sender = Address::repeat_byte(0x11);
        let client = fresh_client(sender);

        let mut tx = make_tx(CHAIN_ID, sender, 0);
        tx.nonce_key = NONCE_KEY_MAX;
        tx.expiry = 0;
        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("nonce-free tx with expiry==0 must be rejected");
        assert!(matches!(err, Eip8130ValidationError::NonceFreeMissingExpiry));
    }

    /// Spec: nonce-free txs MUST have `nonce_sequence == 0` — the field
    /// is unused in nonce-free mode and any other value is consensus-
    /// invalid.
    #[test]
    fn rejects_nonce_free_nonzero_sequence() {
        const CHAIN_ID: u64 = 10;
        let sender = Address::repeat_byte(0x11);
        let client = fresh_client(sender);

        let mut tx = make_tx(CHAIN_ID, sender, 0);
        tx.nonce_key = NONCE_KEY_MAX;
        tx.expiry = 100;
        tx.nonce_sequence = 1;
        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("nonce-free tx with nonce_sequence != 0 must be rejected");
        assert!(matches!(err, Eip8130ValidationError::NonceFreeNonZeroSequence { .. }));
    }

    /// Spec: nonce-free `expiry` must be ≤ `block_ts + MAX_EXPIRY_WINDOW`
    /// so the on-chain seen-set ring stays bounded.
    #[test]
    fn rejects_nonce_free_expiry_too_far() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x11);
        let sender = signer.address();
        let client = fresh_client(sender);

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.nonce_key = NONCE_KEY_MAX;
        tx.expiry = NONCE_FREE_MAX_EXPIRY_WINDOW + 1; // block_ts=0 + window+1 > limit
        let hash = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(&signer, hash);
        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("nonce-free expiry beyond window must be rejected");
        assert!(matches!(err, Eip8130ValidationError::NonceFreeExpiryTooFar { .. }));
    }

    /// Spec: sequenced AA tx whose `nonce_sequence` is below the on-chain
    /// `aa_nonce_slot(sender, key)` value is stale and rejected.
    /// Future-nonce admissions (`nonce_sequence > on_chain`) are accepted
    /// for queued placement; only `<` triggers `NonceMismatch`.
    #[test]
    fn rejects_nonce_mismatch() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x11);
        let sender = signer.address();
        let client = fresh_client(sender);

        // Pre-populate NONCE_MANAGER's slot for (sender, 0) with value=5.
        // The mock stores generic slots via `add_account_with_storage`;
        // we pre-seed the slot directly.
        let slot = U256::from_be_bytes(
            op_revm::precompiles_xlayer::aa_nonce_slot(sender, U256::ZERO).to_be_bytes::<32>(),
        );
        client.add_account(
            NONCE_MANAGER_ADDRESS,
            ExtendedAccount::new(0, U256::ZERO)
                .extend_storage([(B256::from(slot.to_be_bytes::<32>()), U256::from(5u64))]),
        );

        let tx = make_signed_tx(CHAIN_ID, &signer, 0); // tx claims sequence=0 but on-chain head=5 → stale
        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("stale nonce must be rejected");
        assert!(matches!(err, Eip8130ValidationError::NonceMismatch { .. }));
    }

    /// Admission: `nonce_sequence == on_chain` is the lane head; admitted
    /// without rewriting `state_nonce` (already covered by happy path) but
    /// pinned here to lock the equality boundary against future drift.
    #[test]
    fn accepts_nonce_sequence_equal_to_on_chain() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x11);
        let sender = signer.address();
        let client = fresh_client(sender);

        // Pre-populate NONCE_MANAGER slot with on_chain=3.
        let slot = U256::from_be_bytes(
            op_revm::precompiles_xlayer::aa_nonce_slot(sender, U256::ZERO).to_be_bytes::<32>(),
        );
        client.add_account(
            NONCE_MANAGER_ADDRESS,
            ExtendedAccount::new(0, U256::ZERO)
                .extend_storage([(B256::from(slot.to_be_bytes::<32>()), U256::from(3u64))]),
        );

        let tx = make_signed_tx(CHAIN_ID, &signer, 3); // tx.nonce_sequence == on_chain
        let state_nonce = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect("equal-nonce admission must pass");
        assert_eq!(state_nonce, 3);
    }

    /// Admission: `nonce_sequence > on_chain` is a future-nonce tx parked
    /// in the side pool's queued bucket. Pre-fix this was rejected as
    /// `NonceMismatch`, defeating the queued path entirely.
    #[test]
    fn accepts_future_nonce_sequence_for_queueing() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x11);
        let sender = signer.address();
        let client = fresh_client(sender);

        // on_chain=3, tx.nonce_sequence=7 → future nonce (gap of 4).
        let slot = U256::from_be_bytes(
            op_revm::precompiles_xlayer::aa_nonce_slot(sender, U256::ZERO).to_be_bytes::<32>(),
        );
        client.add_account(
            NONCE_MANAGER_ADDRESS,
            ExtendedAccount::new(0, U256::ZERO)
                .extend_storage([(B256::from(slot.to_be_bytes::<32>()), U256::from(3u64))]),
        );

        let tx = make_signed_tx(CHAIN_ID, &signer, 7);
        let state_nonce = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect("future-nonce admission must pass for queued placement");
        // Returns the on-chain head, not the tx's sequence — the queued bucket
        // uses both to position the tx behind the gap.
        assert_eq!(state_nonce, 3);
    }

    /// Admission: `nonce_sequence < on_chain` is the stale case;
    /// rejection identical to `rejects_nonce_mismatch` but pinned with
    /// distinct values so the boundary asymmetry stays explicit.
    #[test]
    fn rejects_stale_nonce_sequence() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x11);
        let sender = signer.address();
        let client = fresh_client(sender);

        // on_chain=10, tx.nonce_sequence=2 → stale.
        let slot = U256::from_be_bytes(
            op_revm::precompiles_xlayer::aa_nonce_slot(sender, U256::ZERO).to_be_bytes::<32>(),
        );
        client.add_account(
            NONCE_MANAGER_ADDRESS,
            ExtendedAccount::new(0, U256::ZERO)
                .extend_storage([(B256::from(slot.to_be_bytes::<32>()), U256::from(10u64))]),
        );

        let tx = make_signed_tx(CHAIN_ID, &signer, 2);
        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("stale nonce must be rejected");
        match err {
            Eip8130ValidationError::NonceMismatch { expected, got } => {
                assert_eq!(expected, 10);
                assert_eq!(got, 2);
            }
            other => panic!("expected NonceMismatch, got {other:?}"),
        }
    }

    /// Spec: payer balance < `gas_limit * max_fee_per_gas` rejects.
    #[test]
    fn rejects_insufficient_balance() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x11);
        let sender = signer.address();
        let client = MockEthProvider::<OpPrimitives>::new();
        client.add_account(NONCE_MANAGER_ADDRESS, ExtendedAccount::new(0, U256::ZERO));
        // Sender has only 1 wei — far less than 100k gas * 1 wei tip.
        client.add_account(sender, ExtendedAccount::new(0, U256::from(1_u64)));

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.gas_limit = 100_000;
        tx.max_fee_per_gas = 1_000_000_000; // 1 gwei
        let hash = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(&signer, hash);
        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("under-funded payer must be rejected");
        assert!(matches!(err, Eip8130ValidationError::InsufficientBalance { .. }));
    }

    /// With a non-zero `l1_data_fee` argument, the balance check
    /// folds it into `required` and rejects when the payer covers L2 gas
    /// but not the L1 component.
    #[test]
    fn balance_check_includes_l1_data_fee() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x11);
        let sender = signer.address();
        let client = MockEthProvider::<OpPrimitives>::new();
        client.add_account(NONCE_MANAGER_ADDRESS, ExtendedAccount::new(0, U256::ZERO));
        // Payer covers `gas_limit * max_fee_per_gas` exactly (100_000 *
        // 1 wei = 100_000 wei) but not the additional 1_000 wei L1 fee.
        let l2_cost = 100_000_u64 * 1_u64;
        client.add_account(sender, ExtendedAccount::new(0, U256::from(l2_cost)));

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.gas_limit = 100_000;
        tx.max_fee_per_gas = 1;
        let hash = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(&signer, hash);
        let l1_fee = U256::from(1_000_u64);
        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, l1_fee)
            .expect_err("balance covering L2 but not L1 must reject");
        match err {
            Eip8130ValidationError::InsufficientBalance { required, available } => {
                assert_eq!(required, U256::from(l2_cost) + l1_fee);
                assert_eq!(available, U256::from(l2_cost));
            }
            other => panic!("expected InsufficientBalance, got {other:?}"),
        }
    }

    /// Payer covering both L2 gas and the L1 data fee passes the
    /// balance check.
    #[test]
    fn balance_check_passes_when_payer_covers_l1_l2_total() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x11);
        let sender = signer.address();
        let client = MockEthProvider::<OpPrimitives>::new();
        client.add_account(NONCE_MANAGER_ADDRESS, ExtendedAccount::new(0, U256::ZERO));
        let l2_cost = 100_000_u64 * 1_u64;
        let l1_fee = U256::from(1_000_u64);
        client.add_account(
            sender,
            ExtendedAccount::new(0, U256::from(l2_cost) + l1_fee + U256::from(1_u64)),
        );

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.gas_limit = 100_000;
        tx.max_fee_per_gas = 1;
        let hash = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(&signer, hash);
        // The validator no longer surfaces `required_balance` — the side-pool
        // admission site recomputes it from tx fields plus the cached
        // `OpL1BlockInfo`. Here we only assert admission succeeds; the
        // balance-fold-in is covered by the rejection case above and by the
        // dual-pool test that drives `add_transaction_with_required`.
        validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, l1_fee)
            .expect("balance covering L1+L2 total must admit");
        let _ = sender;
        let _ = l2_cost;
    }

    /// Admission: tx whose `expiry` lands inside the propagation buffer is rejected
    /// up-front. block_ts=100, buffer=3, expiry=102 → 100+3 >= 102 → reject.
    #[test]
    fn rejects_tx_with_expiry_within_buffer_window() {
        const CHAIN_ID: u64 = 10;
        let sender = Address::repeat_byte(0x11);
        let client = fresh_client(sender);

        let mut tx = make_tx(CHAIN_ID, sender, 0);
        tx.expiry = 102;
        let err = validate_eip8130_transaction(&tx, 1024, 100, CHAIN_ID, &client, U256::ZERO)
            .expect_err("near-expiry tx within buffer must be rejected");
        assert!(matches!(err, Eip8130ValidationError::Expired { .. }));
    }

    /// Admission: tx whose `expiry` is just outside the buffer is admitted.
    /// block_ts=100, buffer=3, expiry=104 → 100+3 < 104 → accept.
    #[test]
    fn accepts_tx_with_expiry_just_outside_buffer() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x11);
        let sender = signer.address();
        let client = fresh_client(sender);

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.expiry = 104;
        // expiry is part of `sender_signature_hash`; re-sign post-mutation.
        let hash = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(&signer, hash);
        validate_eip8130_transaction(&tx, 1024, 100, CHAIN_ID, &client, U256::ZERO)
            .expect("expiry just outside buffer must be admitted");
    }

    /// Admission: `expiry == 0` is the "no expiry" sentinel for sequenced-tx mode
    /// and must continue to bypass the buffer check.
    #[test]
    fn still_accepts_zero_expiry() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x11);
        let sender = signer.address();
        let client = fresh_client(sender);

        let tx = make_signed_tx(CHAIN_ID, &signer, 0);
        assert_eq!(tx.expiry, 0);
        validate_eip8130_transaction(&tx, 1024, 100, CHAIN_ID, &client, U256::ZERO)
            .expect("zero-expiry sequenced tx must be admitted");
    }

    /// Spec: a single call whose `data.len() > MAX_CALL_INPUT_BYTES` is rejected
    /// as a structural bound violation.
    #[test]
    fn rejects_oversized_call_input() {
        const CHAIN_ID: u64 = 10;
        let sender = Address::repeat_byte(0x11);
        let client = fresh_client(sender);

        let mut tx = make_tx(CHAIN_ID, sender, 0);
        tx.calls = vec![vec![Eip8130CallEntry {
            to: Address::repeat_byte(0x22),
            data: vec![0u8; MAX_CALL_INPUT_BYTES + 1].into(),
        }]];
        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("over-cap call input must be rejected");
        assert!(matches!(err, Eip8130ValidationError::CallInputTooLarge { .. }));
    }

    /// Admission: a call with `data.len() == MAX_CALL_INPUT_BYTES` is the boundary
    /// case and must be admitted (cap is exclusive on the `>` side).
    #[test]
    fn accepts_call_input_at_limit() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x11);
        let sender = signer.address();
        let client = MockEthProvider::<OpPrimitives>::new();
        client.add_account(NONCE_MANAGER_ADDRESS, ExtendedAccount::new(0, U256::ZERO));
        // Cover the intrinsic gas for a 128 KiB calldata payload.
        client.add_account(sender, ExtendedAccount::new(0, U256::from(u128::MAX)));

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.gas_limit = 100_000_000;
        tx.calls = vec![vec![Eip8130CallEntry {
            to: Address::repeat_byte(0x22),
            data: vec![0u8; MAX_CALL_INPUT_BYTES].into(),
        }]];
        // `calls` participates in `sender_signature_hash`; re-sign.
        let hash = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(&signer, hash);
        validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect("call input at the limit must be admitted");
    }

    // ------------------------------------------------------------------
    // AuthState dispatch — non-K1 verifiers + on-chain owner_config.
    //
    // Each test seeds the on-chain `owner_config[sender][owner_id]` slot
    // (or leaves it empty to exercise the implicit-EOA fallback) and
    // pins the validator's verdict against the resolver's `AuthState`
    // outcome.
    // ------------------------------------------------------------------

    /// Seed `client` with ACCOUNT_CONFIG_ADDRESS storage so a single
    /// `(sender, owner_id) → owner_config` row reads back as `(verifier, scope)`.
    /// `MockEthProvider::add_account_with_storage` overwrites the bare
    /// account entry, so we re-seed `NONCE_MANAGER_ADDRESS` and the sender
    /// balance afterwards to keep the post-Step-3 reads happy.
    fn seed_owner_config(
        client: &MockEthProvider<OpPrimitives>,
        sender: Address,
        owner_id: B256,
        verifier: Address,
        scope: u8,
    ) {
        let slot = aa_owner_config_slot(sender, U256::from_be_bytes(owner_id.0));
        let key = B256::from(slot.to_be_bytes());
        client.add_account(
            ACCOUNT_CONFIG_ADDRESS,
            ExtendedAccount::new(0, U256::ZERO)
                .extend_storage([(key, pack_owner_config(verifier, scope))]),
        );
    }

    /// Builds a P256-raw-style explicit-from sender_auth with `verifier_data`
    /// short enough to fail native verify (so the resolver lands in
    /// `AuthState::Deferred` — exercising the custom-verifier admission
    /// path without needing real P256 keypair plumbing in the txpool tests).
    /// The 20-byte prefix is the *custom* verifier address; choose any
    /// address distinct from K1 / Delegate / native sentinels.
    fn explicit_custom_auth(custom_verifier: Address) -> Bytes {
        let mut buf = Vec::with_capacity(120);
        buf.extend_from_slice(custom_verifier.as_slice());
        // 100 bytes of opaque verifier-specific data (custom verifiers are
        // dispatched via STATICCALL; we just need a non-empty payload).
        buf.extend_from_slice(&[0x77u8; 100]);
        Bytes::from(buf)
    }

    /// TODO: custom-verifier (`AuthState::Deferred`) admission is temporarily
    /// rejected at pool ingress until the validator can run the verifier's
    /// STATICCALL. When that lands, flip this back to expect admission (the
    /// old behavior was admit + defer to executor STATICCALL at inclusion).
    #[test]
    fn rejects_deferred_custom_verifier() {
        const CHAIN_ID: u64 = 10;
        let sender = Address::repeat_byte(0xAA);
        let custom = Address::repeat_byte(0xBE);
        let client = fresh_client(sender);

        let mut tx = make_tx(CHAIN_ID, sender, 0);
        tx.sender_auth = explicit_custom_auth(custom);

        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("custom verifier admission temporarily rejected");
        assert!(matches!(err, Eip8130ValidationError::InvalidSenderAuth(_)));
        let _ = sender;
    }

    /// Sender_auth in pool admission must not be empty — `Empty` is the
    /// `eth_estimateGas` escape, not a valid pool-ingress shape. Maps to
    /// the new `SenderAuthRequired` variant.
    #[test]
    fn rejects_empty_sender_auth() {
        const CHAIN_ID: u64 = 10;
        let sender = Address::repeat_byte(0x11);
        let client = fresh_client(sender);

        let tx = make_tx(CHAIN_ID, sender, 0); // unsigned; sender_auth = empty
        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("empty sender_auth must be rejected at pool ingress");
        assert!(matches!(err, Eip8130ValidationError::SenderAuthRequired));
    }

    /// `sender_auth` shorter than 20 bytes can't carry a verifier prefix.
    /// Resolver returns `Invalid("too short ...")`; admission maps to
    /// `InvalidSenderAuth`.
    #[test]
    fn rejects_invalid_auth_blob_too_short() {
        const CHAIN_ID: u64 = 10;
        let sender = Address::repeat_byte(0x11);
        let client = fresh_client(sender);

        let mut tx = make_tx(CHAIN_ID, sender, 0);
        tx.sender_auth = Bytes::from(vec![0u8; 19]);
        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("malformed sender_auth must be rejected at pool ingress");
        match err {
            Eip8130ValidationError::InvalidSenderAuth(reason) => {
                assert!(reason.contains("too short"), "unexpected reason: {reason}");
            }
            other => panic!("expected InvalidSenderAuth, got {other:?}"),
        }
    }

    /// xlayer K1 strict-self-owner: explicit-from K1 sig must recover to
    /// `tx.from`. A sig from a different signer is `Invalid` →
    /// `InvalidSenderAuth`.
    #[test]
    fn rejects_strict_self_owner_mismatch() {
        const CHAIN_ID: u64 = 10;
        let victim = signer_for(0x44);
        let attacker = signer_for(0x55);
        let client = fresh_client(victim.address());

        let mut tx = make_tx(CHAIN_ID, victim.address(), 0);
        let hash = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(&attacker, hash);

        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("attacker-signed K1 must fail strict-self-owner");
        match err {
            Eip8130ValidationError::InvalidSenderAuth(reason) => {
                assert!(reason.contains("strict-self-owner"), "unexpected reason: {reason}");
            }
            other => panic!("expected InvalidSenderAuth, got {other:?}"),
        }
    }

    /// On-chain `owner_config[sender][owner_id] = (REVOKED_VERIFIER, _)` →
    /// `OwnerRevoked`. State-disagreement; not a peer-violation.
    #[test]
    fn rejects_revoked_verifier_at_admission() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x11);
        let sender = signer.address();
        let client = fresh_client(sender);
        // Tombstone the implicit-EOA owner row.
        seed_owner_config(&client, sender, implicit_owner_id(sender), REVOKED_VERIFIER, 0);

        let tx = make_signed_tx(CHAIN_ID, &signer, 0);
        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("revoked owner must be rejected at admission");
        assert!(matches!(err, Eip8130ValidationError::OwnerRevoked { .. }));
        assert!(!err.is_bad_transaction(), "OwnerRevoked is state-disagreement");
    }

    /// On-chain row binds a different verifier than the auth's resolved
    /// verifier (e.g. registry was repointed). Maps to `OwnerConfigMismatch`.
    #[test]
    fn rejects_owner_config_verifier_mismatch() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x11);
        let sender = signer.address();
        let client = fresh_client(sender);
        // Register the implicit-EOA owner with a wrong verifier (some
        // address that is neither K1 nor REVOKED_VERIFIER).
        let wrong_verifier = Address::repeat_byte(0xEE);
        seed_owner_config(
            &client,
            sender,
            implicit_owner_id(sender),
            wrong_verifier,
            OWNER_SCOPE_SENDER,
        );

        let tx = make_signed_tx(CHAIN_ID, &signer, 0);
        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("owner_config verifier mismatch must reject");
        match err {
            Eip8130ValidationError::OwnerConfigMismatch { expected, on_chain, .. } => {
                assert_eq!(expected, K1_VERIFIER_ADDRESS);
                assert_eq!(on_chain, wrong_verifier);
            }
            other => panic!("expected OwnerConfigMismatch, got {other:?}"),
        }
    }

    /// On-chain row scope is non-zero but lacks the SENDER bit — owner is
    /// registered for payer side only. Maps to `OwnerScopeMissing`.
    #[test]
    fn rejects_owner_config_scope_missing_sender_bit() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x11);
        let sender = signer.address();
        let client = fresh_client(sender);
        // Register K1 verifier with PAYER-only scope.
        seed_owner_config(
            &client,
            sender,
            implicit_owner_id(sender),
            K1_VERIFIER_ADDRESS,
            OWNER_SCOPE_PAYER,
        );

        let tx = make_signed_tx(CHAIN_ID, &signer, 0);
        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("scope without SENDER bit must reject");
        match err {
            Eip8130ValidationError::OwnerScopeMissing { required_bit, on_chain_scope } => {
                assert_eq!(required_bit, OWNER_SCOPE_SENDER);
                assert_eq!(on_chain_scope, OWNER_SCOPE_PAYER);
            }
            other => panic!("expected OwnerScopeMissing, got {other:?}"),
        }
    }

    /// EOA-recovery mode (`tx.from = None`, bare 65-byte K1 sig). Pre-P1
    /// this was rejected as `EoaRecoveryNotSupported`. Post-P1 the
    /// resolver yields `Native { owner_id = bytes20(recovered) }` and the
    /// implicit-EOA fallback admits.
    #[test]
    fn accepts_eoa_recovery_mode() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x33);
        let sender = signer.address();
        let client = fresh_client(sender);

        let mut tx = TxEip8130 {
            chain_id: CHAIN_ID,
            from: None, // EOA mode
            nonce_key: U256::ZERO,
            nonce_sequence: 0,
            expiry: 0,
            max_priority_fee_per_gas: 0,
            max_fee_per_gas: 1,
            gas_limit: 100_000,
            calls: vec![vec![Eip8130CallEntry {
                to: Address::repeat_byte(0x22),
                data: Default::default(),
            }]],
            ..Default::default()
        };
        let hash = sender_signature_hash(&tx);
        // EOA mode: bare 65-byte K1 sig (no verifier prefix).
        tx.sender_auth = Bytes::from(k1_sig_blob(&signer, hash));

        validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect("EOA-recovery mode must be admitted post-P1");
        let _ = sender;
    }

    /// Native `owner_config` row with a non-K1 verifier (P256-style) and
    /// SENDER scope — admitted. The auth blob is constructed via the
    /// custom-verifier path to land in Deferred for the resolver, but
    /// for the Native-arm coverage we instead pin the owner_config check
    /// directly: register the row, build a Deferred-auth tx (which
    /// bypasses the Native arm), and assert it admits. The companion
    /// owner_config tests above (`rejects_revoked_verifier_at_admission`
    /// etc.) already exercise the Native arm with K1 + the implicit-EOA
    /// fallback. P256-raw native plumbing in the txpool layer would
    /// require a real P256 keypair, which is out of scope for P1 unit
    /// coverage; the Native-arm logic is covered by the same handler
    /// tests in op-revm.
    #[test]
    fn accepts_native_with_explicit_owner_config_registration() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x12);
        let sender = signer.address();
        let client = fresh_client(sender);
        // Explicit (non-implicit) registration: K1 verifier, SENDER scope.
        seed_owner_config(
            &client,
            sender,
            implicit_owner_id(sender),
            K1_VERIFIER_ADDRESS,
            OWNER_SCOPE_SENDER,
        );

        let tx = make_signed_tx(CHAIN_ID, &signer, 0);
        validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect("explicit K1+SENDER registration must admit");
    }

    // ------------------------------------------------------------------
    // sequence_updates admission + rule-D plumbing.
    //
    // Each test builds a `ConfigChange` entry with empty `owner_changes`
    // (the sequence-only carve-out). Any `tx.account_changes` mutation
    // invalidates the sender_auth signature, so re-sign after edits via
    // `re_sign_sender_auth`.
    // ------------------------------------------------------------------

    /// Re-signs `tx.sender_auth` after mutating any signed field.
    /// `account_changes` participates in `sender_signature_hash`, so any
    /// P3 test that adds/removes a `ConfigChange` entry must re-sign.
    fn re_sign_sender_auth(tx: &mut TxEip8130, signer: &PrivateKeySigner) {
        let hash = sender_signature_hash(tx);
        tx.sender_auth = k1_explicit_auth(signer, hash);
    }

    /// Encodes a packed `account_state` word in the layout
    /// [`op_revm::handler::read_packed_sequence`] decodes. Mirrors the
    /// fixture in `eip8130_pool.rs::tests::make_account_state_word`.
    fn pack_account_state(unlocks_at: u64, multichain_seq: u64, local_seq: u64) -> U256 {
        let mut bytes = [0u8; 32];
        bytes[24..32].copy_from_slice(&multichain_seq.to_be_bytes());
        bytes[16..24].copy_from_slice(&local_seq.to_be_bytes());
        let ua = unlocks_at.to_be_bytes();
        bytes[11..16].copy_from_slice(&ua[3..8]);
        U256::from_be_bytes(bytes)
    }

    /// Seeds `_accountState[sender]` so admission's Step 12 reads back
    /// the given pre-values. Re-seeds NONCE_MANAGER and sender balance
    /// so the rest of the validation pipeline still has the state the
    /// other steps need (`add_account` overwrites prior storage).
    fn seed_account_state(
        client: &MockEthProvider<OpPrimitives>,
        sender: Address,
        unlocks_at: u64,
        multichain_seq: u64,
        local_seq: u64,
    ) {
        let slot = op_revm::handler::aa_lock_slot(sender);
        let key = B256::from(slot.to_be_bytes());
        let word = pack_account_state(unlocks_at, multichain_seq, local_seq);
        client.add_account(
            ACCOUNT_CONFIG_ADDRESS,
            ExtendedAccount::new(0, U256::ZERO).extend_storage([(key, word)]),
        );
    }

    /// Admission carve-out: a ConfigChange with empty `owner_changes`
    /// AND empty `authorizer_auth` is the pure sequence-bump shape and
    /// admits as long as `cc.sequence` matches on-chain pre-value.
    #[test]
    fn accepts_tx_with_only_sequence_updates_no_account_changes() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x21);
        let sender = signer.address();
        let client = fresh_client(sender);
        seed_account_state(&client, sender, 0, /* mc */ 7, /* local */ 0);

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        // chain_id == 0 → multichain half; pre-value 7 matches on-chain.
        tx.account_changes = vec![AccountChangeEntry::ConfigChange(ConfigChangeEntry {
            chain_id: 0,
            sequence: 7,
            owner_changes: vec![],
            authorizer_auth: Default::default(),
        })];
        re_sign_sender_auth(&mut tx, &signer);

        validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect("sequence-only ConfigChange with matching pre-value must admit");
    }

    /// `cc.sequence == u64::MAX` overflows the executor's `new_value =
    /// sequence + 1` arithmetic — peer-violation. Mirrors op-revm
    /// `handler.rs:501-504`'s `checked_sub(1)` rejection.
    #[test]
    fn rejects_sequence_update_underflow() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x23);
        let sender = signer.address();
        let client = fresh_client(sender);
        seed_account_state(&client, sender, 0, u64::MAX, 0);

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.account_changes = vec![AccountChangeEntry::ConfigChange(ConfigChangeEntry {
            chain_id: 0,
            sequence: u64::MAX,
            owner_changes: vec![],
            authorizer_auth: Default::default(),
        })];
        re_sign_sender_auth(&mut tx, &signer);

        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("u64::MAX sequence must be rejected as overflow");
        assert!(matches!(err, Eip8130ValidationError::SequenceUpdateUnderflow));
        assert!(err.is_bad_transaction(), "SequenceUpdateUnderflow is peer-violation");
    }

    /// On-chain multichain pre-value advanced past `cc.sequence`:
    /// state-disagreement, not a peer-violation. Mirrors op-revm
    /// `handler.rs:506-509`.
    #[test]
    fn rejects_sequence_mismatch_multichain_too_far() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x24);
        let sender = signer.address();
        let client = fresh_client(sender);
        // On-chain mc_pre = 10; tx pins sequence = 7.
        seed_account_state(&client, sender, 0, /* mc */ 10, 0);

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.account_changes = vec![AccountChangeEntry::ConfigChange(ConfigChangeEntry {
            chain_id: 0,
            sequence: 7,
            owner_changes: vec![],
            authorizer_auth: Default::default(),
        })];
        re_sign_sender_auth(&mut tx, &signer);

        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("stale multichain sequence must be rejected");
        match err {
            Eip8130ValidationError::SequenceMismatch { is_multichain, expected_pre, got_pre } => {
                assert!(is_multichain);
                assert_eq!(expected_pre, 10);
                assert_eq!(got_pre, 7);
            }
            other => panic!("expected SequenceMismatch, got {other:?}"),
        }
        assert!(!err.is_bad_transaction(), "SequenceMismatch is state-disagreement");
    }

    /// Dual-half admission: one entry pins multichain, another pins
    /// local. Both must match on-chain pre-values. The rule-D plumbing
    /// side (that `aa_invalidation_rules` populates BOTH halves of
    /// `SeqExpect`) is exercised by
    /// `eip8130_pool::tests::aa_invalidation_rules_dual_half_sequence_update`.
    #[test]
    fn accepts_dual_half_sequence_update() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x25);
        let sender = signer.address();
        let client = fresh_client(sender);
        seed_account_state(&client, sender, 0, /* mc */ 4, /* local */ 9);

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.account_changes = vec![
            AccountChangeEntry::ConfigChange(ConfigChangeEntry {
                chain_id: 0,
                sequence: 4,
                owner_changes: vec![],
                authorizer_auth: Default::default(),
            }),
            AccountChangeEntry::ConfigChange(ConfigChangeEntry {
                chain_id: CHAIN_ID,
                sequence: 9,
                owner_changes: vec![],
                authorizer_auth: Default::default(),
            }),
        ];
        re_sign_sender_auth(&mut tx, &signer);

        validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect("dual-half sequence-only ConfigChange must admit");
    }

    // ------------------------------------------------------------------
    // Sponsored payer admission.
    //
    // Each test builds a sponsored shape (`tx.payer = Some(payer_addr)`)
    // with a real K1 `payer_auth` over `payer_signature_hash(tx)`; payer
    // owner_config seeding mirrors the sender-side `seed_owner_config`
    // helper. Re-signs are required after any field mutation that
    // participates in the payer signing hash.
    // ------------------------------------------------------------------

    /// Explicit-payer K1 `payer_auth`: `[K1_VERIFIER_ADDRESS(20) || sig(65)]`
    /// over `payer_signature_hash(tx)`. Mirrors `k1_explicit_auth` but signed
    /// against the payer-domain digest (op-alloy `xlayer_sig.rs:115`).
    fn k1_explicit_payer_auth(signer: &PrivateKeySigner, hash: B256) -> Bytes {
        // Reuse the sender helper — the byte layout is identical, only the
        // hash domain differs (caller passes `payer_signature_hash`).
        k1_explicit_auth(signer, hash)
    }

    /// Builds a sponsored AA tx: explicit-from sender (signed via `make_signed_tx`'s
    /// shape) and a payer distinct from the sender. Both `sender_auth` and
    /// `payer_auth` are real K1 sigs, so the resolver yields Native on both
    /// sides.
    fn make_sponsored_tx(
        chain_id: u64,
        sender_signer: &PrivateKeySigner,
        payer_signer: &PrivateKeySigner,
        nonce_sequence: u64,
    ) -> TxEip8130 {
        use op_alloy_consensus::payer_signature_hash;
        let mut tx = TxEip8130 {
            chain_id,
            from: Some(sender_signer.address()),
            payer: Some(payer_signer.address()),
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
        };
        // sender_auth signs `sender_signature_hash`, payer_auth signs
        // `payer_signature_hash`. Both digests cover all signed fields including
        // `payer`, so build the tx skeleton first, then sign both.
        let s_hash = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(sender_signer, s_hash);
        let p_hash = payer_signature_hash(&tx);
        tx.payer_auth = k1_explicit_payer_auth(payer_signer, p_hash);
        tx
    }

    /// Seeds NONCE_MANAGER + sender + payer with bare balances. The payer
    /// gets a generous balance so the balance check passes by default; tests
    /// that target the balance branch override post-construction.
    fn fresh_sponsored_client(sender: Address, payer: Address) -> MockEthProvider<OpPrimitives> {
        let client = MockEthProvider::<OpPrimitives>::new();
        client.add_account(NONCE_MANAGER_ADDRESS, ExtendedAccount::new(0, U256::ZERO));
        // Sender balance is unused by the payer-balance check but kept
        // non-empty so an accidental sender-side read (regression bug)
        // would still produce a meaningful number rather than zero.
        client.add_account(sender, ExtendedAccount::new(0, U256::from(1_u64)));
        client.add_account(payer, ExtendedAccount::new(0, U256::from(1_000_000_000_u64)));
        client
    }

    /// Spec: sponsored K1 admission with sufficient payer balance and an
    /// implicit-EOA payer owner_config row admits. Pins that the
    /// blanket SponsoredPayerNotSupported rejection no longer fires.
    #[test]
    fn accepts_sponsored_payer_k1_happy_path() {
        const CHAIN_ID: u64 = 10;
        let sender_signer = signer_for(0x31);
        let payer_signer = signer_for(0x32);
        let sender = sender_signer.address();
        let payer = payer_signer.address();
        let client = fresh_sponsored_client(sender, payer);

        let tx = make_sponsored_tx(CHAIN_ID, &sender_signer, &payer_signer, 0);
        validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect("sponsored K1 admission must pass");
        // Sender / payer no longer carried on the outcome; they round-trip
        // from tx.from / tx.payer.
        let _ = (sender, payer);
    }

    /// `tx.payer == Some(sender)` is *not* `is_self_pay()` per the type
    /// (`op_alloy_consensus::TxEip8130::is_self_pay` returns `payer.is_none()`),
    /// so the resolver runs the sponsored path. We must still admit (the
    /// payer-side owner_config check skips because `effective_payer == sender`,
    /// rule A already covers it).
    #[test]
    fn accepts_sponsored_payer_with_payer_eq_sender_collapses_to_self_pay() {
        use op_alloy_consensus::payer_signature_hash;
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x33);
        let sender = signer.address();
        let client = fresh_client(sender);

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.payer = Some(sender);
        // Re-sign sender_auth (payer participates in sender_signature_hash)
        // and produce a fresh payer_auth from the same signer (K1 strict-self-
        // owner enforces recovered == claimed `tx.payer == sender`).
        let s_hash = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(&signer, s_hash);
        let p_hash = payer_signature_hash(&tx);
        tx.payer_auth = k1_explicit_payer_auth(&signer, p_hash);

        validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect("payer == sender sponsored shape must admit");
        // Payer / sender no longer surfaced; tx.payer.unwrap_or(sender) is
        // derived downstream where needed.
        let _ = sender;
    }

    /// Sponsored admission with a malformed `payer_auth` (shorter than 20
    /// bytes — can't carry a verifier prefix). Resolver yields
    /// `Invalid(...)` → `InvalidPayerAuth`. Peer-violation.
    #[test]
    fn rejects_sponsored_payer_invalid_auth_blob() {
        const CHAIN_ID: u64 = 10;
        let sender_signer = signer_for(0x34);
        let payer_signer = signer_for(0x35);
        let sender = sender_signer.address();
        let payer = payer_signer.address();
        let client = fresh_sponsored_client(sender, payer);

        let mut tx = make_sponsored_tx(CHAIN_ID, &sender_signer, &payer_signer, 0);
        // Truncate payer_auth below 20 bytes; sender_auth stays valid so
        // the rejection lands specifically on the payer dispatch.
        tx.payer_auth = Bytes::from(vec![0u8; 19]);
        // payer_auth doesn't participate in sender_signature_hash, so the
        // sender_auth signed by `make_sponsored_tx` is still valid.
        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("malformed payer_auth must be rejected");
        match err {
            Eip8130ValidationError::InvalidPayerAuth(reason) => {
                assert!(reason.contains("too short"), "unexpected reason: {reason}");
            }
            other => panic!("expected InvalidPayerAuth, got {other:?}"),
        }
        let err =
            validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO).unwrap_err();
        assert!(err.is_bad_transaction(), "InvalidPayerAuth is peer-violation");
    }

    /// Spec: sponsored payer balance < `gas_limit * max_fee_per_gas`
    /// rejects; sender balance is irrelevant on the sponsored path.
    #[test]
    fn rejects_sponsored_payer_insufficient_balance() {
        use op_alloy_consensus::payer_signature_hash;
        const CHAIN_ID: u64 = 10;
        let sender_signer = signer_for(0x36);
        let payer_signer = signer_for(0x37);
        let sender = sender_signer.address();
        let payer = payer_signer.address();

        let client = MockEthProvider::<OpPrimitives>::new();
        client.add_account(NONCE_MANAGER_ADDRESS, ExtendedAccount::new(0, U256::ZERO));
        // Sender has plenty of headroom; payer is starved. Pre-P2 the
        // balance read used `sender` and this would have admitted spuriously.
        client.add_account(sender, ExtendedAccount::new(0, U256::from(u128::MAX)));
        client.add_account(payer, ExtendedAccount::new(0, U256::from(1_u64)));

        let mut tx = make_sponsored_tx(CHAIN_ID, &sender_signer, &payer_signer, 0);
        tx.gas_limit = 100_000;
        tx.max_fee_per_gas = 1_000_000_000; // 1 gwei
        // Re-sign both sides — both fields participate in their respective hashes.
        let s_hash = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(&sender_signer, s_hash);
        let p_hash = payer_signature_hash(&tx);
        tx.payer_auth = k1_explicit_payer_auth(&payer_signer, p_hash);

        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("under-funded payer (sender funded) must reject");
        assert!(matches!(err, Eip8130ValidationError::InsufficientBalance { .. }));
    }

    /// On-chain `owner_config[payer][owner_id] = (REVOKED_VERIFIER, _)`
    /// rejects with `OwnerRevoked` carrying the payer's address. Pins
    /// that the variant is polymorphic across sender/payer (no bespoke
    /// payer-side variant required).
    #[test]
    fn rejects_sponsored_payer_owner_revoked() {
        const CHAIN_ID: u64 = 10;
        let sender_signer = signer_for(0x38);
        let payer_signer = signer_for(0x39);
        let sender = sender_signer.address();
        let payer = payer_signer.address();
        let client = fresh_sponsored_client(sender, payer);
        // Tombstone the payer's implicit-EOA owner row.
        seed_owner_config(&client, payer, implicit_owner_id(payer), REVOKED_VERIFIER, 0);

        let tx = make_sponsored_tx(CHAIN_ID, &sender_signer, &payer_signer, 0);
        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("revoked payer owner must reject");
        match err {
            Eip8130ValidationError::OwnerRevoked { account, .. } => {
                assert_eq!(account, payer, "OwnerRevoked must name the payer side");
            }
            other => panic!("expected OwnerRevoked, got {other:?}"),
        }
    }

    /// Payer's on-chain owner_config row is non-zero scope but lacks the
    /// PAYER bit — owner is registered for sender side only. Maps to
    /// `OwnerScopeMissing { required_bit: OWNER_SCOPE_PAYER, .. }`.
    #[test]
    fn rejects_sponsored_payer_scope_missing_payer_bit() {
        const CHAIN_ID: u64 = 10;
        let sender_signer = signer_for(0x3A);
        let payer_signer = signer_for(0x3B);
        let sender = sender_signer.address();
        let payer = payer_signer.address();
        let client = fresh_sponsored_client(sender, payer);
        // Register K1 verifier on the payer's row with SENDER-only scope.
        seed_owner_config(
            &client,
            payer,
            implicit_owner_id(payer),
            K1_VERIFIER_ADDRESS,
            OWNER_SCOPE_SENDER,
        );

        let tx = make_sponsored_tx(CHAIN_ID, &sender_signer, &payer_signer, 0);
        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("payer scope without PAYER bit must reject");
        match err {
            Eip8130ValidationError::OwnerScopeMissing { required_bit, on_chain_scope } => {
                assert_eq!(required_bit, OWNER_SCOPE_PAYER);
                assert_eq!(on_chain_scope, OWNER_SCOPE_SENDER);
            }
            other => panic!("expected OwnerScopeMissing, got {other:?}"),
        }
    }

    /// Payer's on-chain row binds a different verifier than the auth's
    /// resolved verifier. Maps to `OwnerConfigMismatch { account: payer, .. }`.
    #[test]
    fn rejects_sponsored_payer_verifier_mismatch() {
        const CHAIN_ID: u64 = 10;
        let sender_signer = signer_for(0x3C);
        let payer_signer = signer_for(0x3D);
        let sender = sender_signer.address();
        let payer = payer_signer.address();
        let client = fresh_sponsored_client(sender, payer);
        let wrong_verifier = Address::repeat_byte(0xEE);
        seed_owner_config(
            &client,
            payer,
            implicit_owner_id(payer),
            wrong_verifier,
            OWNER_SCOPE_PAYER,
        );

        let tx = make_sponsored_tx(CHAIN_ID, &sender_signer, &payer_signer, 0);
        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("payer owner_config verifier mismatch must reject");
        match err {
            Eip8130ValidationError::OwnerConfigMismatch { account, expected, on_chain, .. } => {
                assert_eq!(account, payer);
                assert_eq!(expected, K1_VERIFIER_ADDRESS);
                assert_eq!(on_chain, wrong_verifier);
            }
            other => panic!("expected OwnerConfigMismatch, got {other:?}"),
        }
    }

    /// TODO: sponsored custom-verifier (Deferred) payer_auth is temporarily
    /// rejected at pool ingress, mirroring the sender-side restriction. Flip
    /// back to expect admission once the validator can run the verifier's
    /// STATICCALL.
    #[test]
    fn rejects_sponsored_payer_deferred() {
        const CHAIN_ID: u64 = 10;
        let sender_signer = signer_for(0x3E);
        let payer = Address::repeat_byte(0xBE);
        let custom = Address::repeat_byte(0xBF);
        let sender = sender_signer.address();
        let client = fresh_sponsored_client(sender, payer);

        let mut tx = make_signed_tx(CHAIN_ID, &sender_signer, 0);
        tx.payer = Some(payer);
        // payer_auth: 20-byte custom verifier prefix + opaque data → resolver
        // returns `Deferred` (no owner_config slot read at admission).
        let mut payer_blob = Vec::with_capacity(120);
        payer_blob.extend_from_slice(custom.as_slice());
        payer_blob.extend_from_slice(&[0x77u8; 100]);
        tx.payer_auth = Bytes::from(payer_blob);
        // Re-sign sender_auth (payer participates in sender_signature_hash).
        let s_hash = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(&sender_signer, s_hash);

        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("custom verifier payer admission temporarily rejected");
        assert!(matches!(err, Eip8130ValidationError::InvalidPayerAuth(_)));
        let _ = (sender, payer);
    }

    /// Sponsored shape with empty `payer_auth` rejects as `PayerAuthRequired`
    /// — the estimateGas escape mirrors `SenderAuthRequired`. Peer-violation.
    #[test]
    fn rejects_sponsored_payer_empty_auth() {
        const CHAIN_ID: u64 = 10;
        let sender_signer = signer_for(0x40);
        let payer = Address::repeat_byte(0xC1);
        let sender = sender_signer.address();
        let client = fresh_sponsored_client(sender, payer);

        let mut tx = make_signed_tx(CHAIN_ID, &sender_signer, 0);
        tx.payer = Some(payer);
        tx.payer_auth = Bytes::new();
        let s_hash = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(&sender_signer, s_hash);

        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("empty payer_auth in sponsored mode must reject");
        assert!(matches!(err, Eip8130ValidationError::PayerAuthRequired));
        assert!(err.is_bad_transaction(), "PayerAuthRequired is peer-violation");
    }

    // ------------------------------------------------------------------
    // account_changes admission (Create / ConfigChange config writes
    // / Delegation). State-touching tests build the matching account
    // configuration via the same `seed_*` helpers used by P1-P3 and
    // re-sign sender_auth after any account_changes mutation
    // (`account_changes` participates in `sender_signature_hash`).
    // ------------------------------------------------------------------

    use op_alloy_consensus::{CreateEntry, DelegationEntry, Owner, OwnerChange};
    use op_revm::constants::{OP_AUTHORIZE_OWNER, OP_REVOKE_OWNER, OWNER_SCOPE_CONFIG};

    /// Builds a Create entry whose deployed address derives via
    /// `derive_account_address(ACCOUNT_CONFIG_ADDRESS, salt, bytecode, owners)`.
    fn make_create_entry(salt: u8, bytecode: &[u8], owners: Vec<Owner>) -> CreateEntry {
        CreateEntry {
            user_salt: B256::repeat_byte(salt),
            bytecode: Bytes::copy_from_slice(bytecode),
            initial_owners: owners,
        }
    }

    /// Marks the ACCOUNT_CONFIG_ADDRESS account as "deployed" in the mock.
    /// `MockEthProvider`'s default `add_account` leaves `bytecode_hash =
    /// None`, which mempool validation reads as "not deployed". Tests
    /// that admit a config-write tx must call this so the
    /// AccountConfigNotDeployed gate doesn't fire.
    fn mark_account_config_deployed(client: &MockEthProvider<OpPrimitives>) {
        // `with_bytecode` sets `bytecode_hash = keccak256(bytes)`; any
        // non-empty bytes will satisfy the `code_hash != keccak256([])`
        // check.
        client.add_account(
            ACCOUNT_CONFIG_ADDRESS,
            ExtendedAccount::new(0, U256::ZERO).with_bytecode(Bytes::from_static(&[0xfe])),
        );
    }

    /// Same as `mark_account_config_deployed` but preserves any
    /// previously seeded storage at `ACCOUNT_CONFIG_ADDRESS`.
    /// `MockEthProvider::add_account` overwrites prior storage, so when
    /// a test seeds owner_config / account_state then needs the
    /// contract marked deployed, it must re-seed afterwards. This
    /// helper bundles the deploy + re-seed.
    fn seed_account_config_deployed_with_storage(
        client: &MockEthProvider<OpPrimitives>,
        storage: impl IntoIterator<Item = (B256, U256)>,
    ) {
        let acc = ExtendedAccount::new(0, U256::ZERO)
            .with_bytecode(Bytes::from_static(&[0xfe]))
            .extend_storage(storage);
        client.add_account(ACCOUNT_CONFIG_ADDRESS, acc);
    }

    /// K1 `authorizer_auth` blob: `[K1_VERIFIER_ADDRESS(20) || sig(65)]`
    /// over `config_change_digest(sender, change)`. Mirrors
    /// `k1_explicit_auth` for the authorizer-chain digest domain.
    fn k1_authorizer_auth(
        signer: &PrivateKeySigner,
        sender: Address,
        change: &ConfigChangeEntry,
    ) -> Bytes {
        let digest = op_alloy_consensus::config_change_digest(sender, change);
        let mut buf = Vec::with_capacity(85);
        buf.extend_from_slice(K1_VERIFIER_ADDRESS.as_slice());
        buf.extend_from_slice(&k1_sig_blob(signer, digest));
        Bytes::from(buf)
    }

    // ---- structural rejections ----

    /// Spec: more than one `Create` entry in `account_changes`.
    /// Mirrors op-revm `handler.rs:1034-1038`. Peer-violation.
    #[test]
    fn rejects_multiple_create_entries() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x41);
        let sender = signer.address();
        let client = fresh_client(sender);

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        let entry = AccountChangeEntry::Create(make_create_entry(0x01, &[0x60, 0x00], vec![]));
        tx.account_changes = vec![entry.clone(), entry];
        re_sign_sender_auth(&mut tx, &signer);

        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("multiple Create entries must reject");
        assert!(matches!(err, Eip8130ValidationError::MultipleCreateEntries));
        assert!(err.is_bad_transaction());
    }

    /// Spec: a `Create` entry following a non-Create entry violates
    /// "Create MUST be first". Mirrors op-revm `handler.rs:1068`.
    #[test]
    fn rejects_create_not_first() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x42);
        let sender = signer.address();
        let client = fresh_client(sender);
        seed_account_state(&client, sender, 0, 0, 0);

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        // A pure-sequence ConfigChange (legal under P3 carve-out) is
        // followed by a Create — out of order.
        tx.account_changes = vec![
            AccountChangeEntry::ConfigChange(ConfigChangeEntry {
                chain_id: 0,
                sequence: 0,
                owner_changes: vec![],
                authorizer_auth: Default::default(),
            }),
            AccountChangeEntry::Create(make_create_entry(0x02, &[0x60, 0x00], vec![])),
        ];
        re_sign_sender_auth(&mut tx, &signer);

        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("Create after a non-Create must reject");
        assert!(matches!(err, Eip8130ValidationError::CreateNotFirstEntry));
        assert!(err.is_bad_transaction());
    }

    /// Spec: a `Create` entry whose runtime `bytecode.len() >
    /// MAX_CODE_SIZE` (EIP-170 = 24576) violates the deployed-code
    /// cap. Mirrors op-revm `handler.rs:1045-1051`.
    #[test]
    fn rejects_create_bytecode_too_large() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x43);
        let sender = signer.address();
        let client = fresh_client(sender);

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        let oversized = vec![0u8; op_revm::revm::primitives::eip170::MAX_CODE_SIZE + 1];
        tx.account_changes =
            vec![AccountChangeEntry::Create(make_create_entry(0x03, &oversized, vec![]))];
        re_sign_sender_auth(&mut tx, &signer);

        let err = validate_eip8130_transaction(
            &tx,
            // Encoded length cap: 256 KiB; `oversized` is 24577 bytes,
            // well within. Pass a stub encoded_len that doesn't trigger
            // the size cap.
            128 * 1024,
            0,
            CHAIN_ID,
            &client,
            U256::ZERO,
        )
        .expect_err("oversized create bytecode must reject");
        assert!(matches!(err, Eip8130ValidationError::CreateBytecodeTooLarge { .. }));
        assert!(err.is_bad_transaction());
    }

    /// Spec: more than one `Delegation` entry per tx. Mirrors op-revm
    /// `handler.rs:1077`.
    #[test]
    fn rejects_multiple_delegation_entries() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x44);
        let sender = signer.address();
        let client = fresh_client(sender);

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        let d = AccountChangeEntry::Delegation(DelegationEntry { target: Address::repeat_byte(1) });
        tx.account_changes = vec![d.clone(), d];
        re_sign_sender_auth(&mut tx, &signer);

        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("two Delegation entries must reject");
        assert!(matches!(err, Eip8130ValidationError::MultipleDelegationEntries));
        assert!(err.is_bad_transaction());
    }

    /// Spec: a matching `ConfigChange` whose `owner_changes` contains
    /// only unknown change_type codes is rejected as
    /// `EmptyConfigChange`. Mirrors op-revm `handler.rs:1058-1062`.
    #[test]
    fn rejects_empty_config_change() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x45);
        let sender = signer.address();
        let client = fresh_client(sender);
        seed_account_state(&client, sender, 0, 0, 0);

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        // Unknown change_type = 0xFF — parser drops it, no
        // `_ownerConfig` write happens, but the entry would still
        // bump sequence and run authorizer validation.
        tx.account_changes = vec![AccountChangeEntry::ConfigChange(ConfigChangeEntry {
            chain_id: 0,
            sequence: 0,
            owner_changes: vec![OwnerChange {
                change_type: 0xFF,
                owner_id: B256::repeat_byte(0xCC),
                verifier: K1_VERIFIER_ADDRESS,
                scope: OWNER_SCOPE_CONFIG,
            }],
            authorizer_auth: Default::default(),
        })];
        re_sign_sender_auth(&mut tx, &signer);

        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("ConfigChange with no known op codes must reject");
        assert!(matches!(err, Eip8130ValidationError::EmptyConfigChange));
        assert!(err.is_bad_transaction());
    }

    // ---- Create entry state checks ----

    /// Admission: a Create entry whose derived address is empty on
    /// chain (`code_hash == keccak256([])`, `nonce == 0`, no occupied
    /// owner_config slots) is admitted.
    #[test]
    fn accepts_create_at_clean_address() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x46);
        let sender = signer.address();
        let client = fresh_client(sender);

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        // Create with no initial owners — `pre_writes` is empty so
        // there are no owner-config slot reads. The Create is
        // standalone (not preceded by ConfigChange) so the lock
        // window's read still happens; we leave `_accountState`
        // unset (default zero word) so unlocks_at = 0 → not locked.
        tx.account_changes =
            vec![AccountChangeEntry::Create(make_create_entry(0x04, &[0x60, 0x00], vec![]))];
        // Cover Create entry's intrinsic gas (bytecode_cost + per-unit).
        tx.gas_limit = 500_000;
        re_sign_sender_auth(&mut tx, &signer);

        validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect("clean Create target must admit");
    }

    /// Spec: a Create entry whose derived address has existing code
    /// rejects as `CreateTargetCollision`. Mirrors op-revm
    /// `handler.rs:1520-1530`.
    #[test]
    fn rejects_create_target_collision_with_existing_code() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x47);
        let sender = signer.address();
        let client = fresh_client(sender);

        // Pre-derive the target address so we can pre-populate it.
        let create = make_create_entry(0x05, &[0x60, 0x01, 0x60, 0x00], vec![]);
        let derived = derive_account_address(
            ACCOUNT_CONFIG_ADDRESS,
            create.user_salt,
            &create.bytecode,
            &create.initial_owners,
        );
        // Mark the derived address as having non-empty code.
        client.add_account(
            derived,
            ExtendedAccount::new(0, U256::ZERO).with_bytecode(Bytes::from_static(&[0x12, 0x34])),
        );

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.account_changes = vec![AccountChangeEntry::Create(create)];
        tx.gas_limit = 500_000; // cover Create intrinsic gas
        re_sign_sender_auth(&mut tx, &signer);

        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("Create over existing code must reject");
        match err {
            Eip8130ValidationError::CreateTargetCollision { address } => {
                assert_eq!(address, derived);
            }
            other => panic!("expected CreateTargetCollision, got {other:?}"),
        }
        let err =
            validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO).unwrap_err();
        assert!(!err.is_bad_transaction(), "CreateTargetCollision is state-disagreement");
    }

    /// Spec: a Create entry whose target has nonce > 0 but no code
    /// also collides. Mirrors op-revm `handler.rs:1523`.
    #[test]
    fn rejects_create_target_collision_with_nonzero_nonce() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x48);
        let sender = signer.address();
        let client = fresh_client(sender);

        let create = make_create_entry(0x06, &[0x60, 0x02], vec![]);
        let derived = derive_account_address(
            ACCOUNT_CONFIG_ADDRESS,
            create.user_salt,
            &create.bytecode,
            &create.initial_owners,
        );
        // Nonce = 5, no code: still a collision.
        client.add_account(derived, ExtendedAccount::new(5, U256::ZERO));

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.account_changes = vec![AccountChangeEntry::Create(create)];
        tx.gas_limit = 500_000; // cover Create intrinsic gas
        re_sign_sender_auth(&mut tx, &signer);

        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("Create over nonzero-nonce account must reject");
        assert!(matches!(err, Eip8130ValidationError::CreateTargetCollision { .. }));
    }

    /// Spec: a Create entry whose `pre_writes` slot is occupied
    /// (some owner_config row already non-zero) rejects. Mirrors
    /// op-revm `handler.rs:1539-1552`.
    #[test]
    fn rejects_create_pre_write_slot_occupied() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x49);
        let sender = signer.address();
        let client = fresh_client(sender);

        let owner = Owner {
            verifier: K1_VERIFIER_ADDRESS,
            owner_id: B256::repeat_byte(0xAA),
            scope: OWNER_SCOPE_SENDER,
        };
        let create = make_create_entry(0x07, &[0x60, 0x03], vec![owner.clone()]);
        let derived = derive_account_address(
            ACCOUNT_CONFIG_ADDRESS,
            create.user_salt,
            &create.bytecode,
            &create.initial_owners,
        );

        // Pre-occupy the owner_config slot at the derived address +
        // owner_id with some non-zero value.
        let occupied_slot = aa_owner_config_slot(derived, U256::from_be_bytes(owner.owner_id.0));
        client.add_account(
            ACCOUNT_CONFIG_ADDRESS,
            ExtendedAccount::new(0, U256::ZERO).extend_storage([(
                B256::from(occupied_slot.to_be_bytes::<32>()),
                U256::from(0x42),
            )]),
        );

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.account_changes = vec![AccountChangeEntry::Create(create)];
        tx.gas_limit = 500_000; // cover Create + per-owner intrinsic gas
        re_sign_sender_auth(&mut tx, &signer);

        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("Create with occupied owner slot must reject");
        match err {
            Eip8130ValidationError::CreateOwnerSlotOccupied { address, slot } => {
                assert_eq!(address, derived);
                assert_eq!(slot, occupied_slot);
            }
            other => panic!("expected CreateOwnerSlotOccupied, got {other:?}"),
        }
    }

    // ---- Lock window ----

    /// Admission: a lock-sensitive tx (Create) is admitted when
    /// `block_timestamp >= unlocks_at`. P4 invariant.
    #[test]
    fn accepts_lock_sensitive_when_unlocked() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x4A);
        let sender = signer.address();
        let client = fresh_client(sender);
        // unlocks_at = 5; we admit at block_ts = 10 (unlocked).
        seed_account_state(&client, sender, 5, 0, 0);

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.account_changes =
            vec![AccountChangeEntry::Create(make_create_entry(0x08, &[0x60, 0x04], vec![]))];
        tx.gas_limit = 500_000; // cover Create intrinsic gas
        re_sign_sender_auth(&mut tx, &signer);

        validate_eip8130_transaction(&tx, 1024, 10, CHAIN_ID, &client, U256::ZERO)
            .expect("lock-sensitive tx with elapsed lock window must admit");
    }

    /// Spec: a lock-sensitive tx (Create) whose sender is still locked
    /// rejects with `AccountLocked`. State-disagreement.
    #[test]
    fn rejects_lock_sensitive_when_locked() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x4B);
        let sender = signer.address();
        let client = fresh_client(sender);
        seed_account_state(&client, sender, /* unlocks_at */ 100, 0, 0);

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.account_changes =
            vec![AccountChangeEntry::Create(make_create_entry(0x09, &[0x60, 0x05], vec![]))];
        tx.gas_limit = 500_000; // cover Create intrinsic gas
        re_sign_sender_auth(&mut tx, &signer);

        let err = validate_eip8130_transaction(
            &tx,
            1024,
            /* block_ts */ 50,
            CHAIN_ID,
            &client,
            U256::ZERO,
        )
        .expect_err("locked sender must reject lock-sensitive tx");
        match err {
            Eip8130ValidationError::AccountLocked { unlocks_at } => assert_eq!(unlocks_at, 100),
            other => panic!("expected AccountLocked, got {other:?}"),
        }
        let err =
            validate_eip8130_transaction(&tx, 1024, 50, CHAIN_ID, &client, U256::ZERO).unwrap_err();
        assert!(!err.is_bad_transaction(), "AccountLocked is state-disagreement");
    }

    /// Admission: pure-sequence ConfigChange entries are lock-sensitive.
    /// `process_config_change_entry` emits a `sequence_updates` slot for
    /// any matching ConfigChange, and the executor's `check_account_lock`
    /// (`op-revm/src/handler.rs:343-370`) fires on non-empty
    /// `sequence_updates`. The earlier P3 carve-out (admit on lock) was
    /// based on a misreading of the executor flow — spec line 511 and the
    /// handler agree that pure-sequence rejects on a locked account.
    #[test]
    fn rejects_pure_sequence_config_change_when_locked() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x4C);
        let sender = signer.address();
        let client = fresh_client(sender);
        // Lock window in the future (`unlocks_at = 100`); on-chain mc
        // pre-value matches the tx's pinned sequence.
        seed_account_state(&client, sender, /* unlocks_at */ 100, /* mc */ 7, 0);

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.account_changes = vec![AccountChangeEntry::ConfigChange(ConfigChangeEntry {
            chain_id: 0,
            sequence: 7,
            owner_changes: vec![],
            authorizer_auth: Default::default(),
        })];
        re_sign_sender_auth(&mut tx, &signer);

        let err = validate_eip8130_transaction(
            &tx,
            1024,
            /* block_ts */ 50,
            CHAIN_ID,
            &client,
            U256::ZERO,
        )
        .expect_err("pure-sequence ConfigChange must reject when sender locked");
        match err {
            Eip8130ValidationError::AccountLocked { unlocks_at } => assert_eq!(unlocks_at, 100),
            other => panic!("expected AccountLocked, got {other:?}"),
        }
    }

    // ---- Authorizer chain (Native K1) ----

    /// Admission: a ConfigChange with K1 authorizer_auth that recovers
    /// to the implicit-EOA owner of `sender` admits. The authorizer's
    /// owner_id is `bytes32(bytes20(sender))`, which the implicit-EOA
    /// fallback accepts even with no on-chain owner_config row.
    #[test]
    fn accepts_native_authorizer_chain_owner_writes() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x4D);
        let sender = signer.address();
        let client = fresh_client(sender);
        // Mark ACCOUNT_CONFIG deployed (the config-write path requires
        // it). Re-seed account_state so the lock check reads zero
        // (unlocks_at = 0) and the sequence check passes.
        let lock_slot = aa_lock_slot(sender);
        let lock_word = pack_account_state(0, 0, 0);
        seed_account_config_deployed_with_storage(
            &client,
            [(B256::from(lock_slot.to_be_bytes::<32>()), lock_word)],
        );

        // Build the ConfigChange first so we can sign its digest.
        let mut cc = ConfigChangeEntry {
            chain_id: 0,
            sequence: 0,
            owner_changes: vec![OwnerChange {
                change_type: OP_AUTHORIZE_OWNER,
                owner_id: B256::repeat_byte(0xCC),
                verifier: K1_VERIFIER_ADDRESS,
                scope: OWNER_SCOPE_SENDER,
            }],
            authorizer_auth: Default::default(),
        };
        cc.authorizer_auth = k1_authorizer_auth(&signer, sender, &cc);

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.account_changes = vec![AccountChangeEntry::ConfigChange(cc)];
        re_sign_sender_auth(&mut tx, &signer);

        validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect("K1 authorizer recovering to implicit-EOA owner must admit");
    }

    /// Spec: a Native authorizer whose K1 sig recovers to an owner
    /// whose on-chain row is bound to a different verifier rejects
    /// with `AuthorizerOwnerMismatch`. State-disagreement. The
    /// authorizer's signer is distinct from the tx sender so the
    /// sender-side `owner_config` check (Step 9) doesn't intercept.
    #[test]
    fn rejects_authorizer_owner_mismatch() {
        const CHAIN_ID: u64 = 10;
        let sender_signer = signer_for(0x4E);
        let auth_signer = signer_for(0x6E);
        let sender = sender_signer.address();
        let auth_owner_id = implicit_owner_id(auth_signer.address());
        let client = fresh_client(sender);

        // Seed: ACCOUNT_CONFIG deployed, account unlocked (unlocks_at=0),
        // sender's implicit-EOA owner row left empty (Step 9 admits
        // via implicit-EOA fallback), authorizer's K1 owner row bound
        // to a non-K1 verifier (Step 12d catches the mismatch).
        let lock_slot = aa_lock_slot(sender);
        let auth_owner_slot = aa_owner_config_slot(sender, U256::from_be_bytes(auth_owner_id.0));
        let wrong_verifier = Address::repeat_byte(0xEE);
        seed_account_config_deployed_with_storage(
            &client,
            [
                (B256::from(lock_slot.to_be_bytes::<32>()), pack_account_state(0, 0, 0)),
                (
                    B256::from(auth_owner_slot.to_be_bytes::<32>()),
                    pack_owner_config(wrong_verifier, OWNER_SCOPE_CONFIG),
                ),
            ],
        );

        let mut cc = ConfigChangeEntry {
            chain_id: 0,
            sequence: 0,
            owner_changes: vec![OwnerChange {
                change_type: OP_AUTHORIZE_OWNER,
                owner_id: B256::repeat_byte(0xDD),
                verifier: K1_VERIFIER_ADDRESS,
                scope: OWNER_SCOPE_SENDER,
            }],
            authorizer_auth: Default::default(),
        };
        cc.authorizer_auth = k1_authorizer_auth(&auth_signer, sender, &cc);

        let mut tx = make_signed_tx(CHAIN_ID, &sender_signer, 0);
        tx.account_changes = vec![AccountChangeEntry::ConfigChange(cc)];
        re_sign_sender_auth(&mut tx, &sender_signer);

        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("authorizer owner_config verifier mismatch must reject");
        match err {
            Eip8130ValidationError::AuthorizerOwnerMismatch { on_chain_verifier, .. } => {
                assert_eq!(on_chain_verifier, wrong_verifier);
            }
            other => panic!("expected AuthorizerOwnerMismatch, got {other:?}"),
        }
    }

    /// Spec: a Native authorizer whose owner has a non-zero scope that
    /// omits the CONFIG bit rejects. State-disagreement. Authorizer's
    /// signer is distinct from sender so the sender-side check
    /// (Step 9) doesn't intercept.
    #[test]
    fn rejects_authorizer_owner_lacks_config_scope() {
        const CHAIN_ID: u64 = 10;
        let sender_signer = signer_for(0x4F);
        let auth_signer = signer_for(0x6F);
        let sender = sender_signer.address();
        let auth_owner_id = implicit_owner_id(auth_signer.address());
        let client = fresh_client(sender);

        // Seed: K1 verifier on the authorizer's row with SENDER-only
        // scope (CONFIG bit missing). Sender's row left empty so
        // Step 9 admits via implicit-EOA fallback.
        let lock_slot = aa_lock_slot(sender);
        let auth_owner_slot = aa_owner_config_slot(sender, U256::from_be_bytes(auth_owner_id.0));
        seed_account_config_deployed_with_storage(
            &client,
            [
                (B256::from(lock_slot.to_be_bytes::<32>()), pack_account_state(0, 0, 0)),
                (
                    B256::from(auth_owner_slot.to_be_bytes::<32>()),
                    pack_owner_config(K1_VERIFIER_ADDRESS, OWNER_SCOPE_SENDER),
                ),
            ],
        );

        let mut cc = ConfigChangeEntry {
            chain_id: 0,
            sequence: 0,
            owner_changes: vec![OwnerChange {
                change_type: OP_AUTHORIZE_OWNER,
                owner_id: B256::repeat_byte(0xEE),
                verifier: K1_VERIFIER_ADDRESS,
                scope: OWNER_SCOPE_SENDER,
            }],
            authorizer_auth: Default::default(),
        };
        cc.authorizer_auth = k1_authorizer_auth(&auth_signer, sender, &cc);

        let mut tx = make_signed_tx(CHAIN_ID, &sender_signer, 0);
        tx.account_changes = vec![AccountChangeEntry::ConfigChange(cc)];
        re_sign_sender_auth(&mut tx, &sender_signer);

        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("authorizer owner without CONFIG scope must reject");
        assert!(matches!(err, Eip8130ValidationError::AuthorizerOwnerLacksConfigScope { .. }));
    }

    /// Admission: a custom-verifier (Deferred) authorizer admits at
    /// the mempool — the executor runs the STATICCALL at inclusion.
    #[test]
    fn admits_deferred_custom_authorizer() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x50);
        let sender = signer.address();
        let client = fresh_client(sender);

        // Deployed, unlocked, on-chain unset (Deferred admits without
        // reading owner_config).
        let lock_slot = aa_lock_slot(sender);
        seed_account_config_deployed_with_storage(
            &client,
            [(B256::from(lock_slot.to_be_bytes::<32>()), pack_account_state(0, 0, 0))],
        );

        // Custom verifier blob: 20-byte address that is neither K1 nor
        // Delegate, plus opaque payload.
        let custom = Address::repeat_byte(0x77);
        let mut blob = Vec::with_capacity(120);
        blob.extend_from_slice(custom.as_slice());
        blob.extend_from_slice(&[0x42u8; 100]);

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.account_changes = vec![AccountChangeEntry::ConfigChange(ConfigChangeEntry {
            chain_id: 0,
            sequence: 0,
            owner_changes: vec![OwnerChange {
                change_type: OP_AUTHORIZE_OWNER,
                owner_id: B256::repeat_byte(0xAB),
                verifier: K1_VERIFIER_ADDRESS,
                scope: OWNER_SCOPE_SENDER,
            }],
            authorizer_auth: Bytes::from(blob),
        })];
        re_sign_sender_auth(&mut tx, &signer);

        validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect("deferred custom authorizer must admit (executor catches at inclusion)");
    }

    /// Admission: a tx with two ConfigChange entries where the second
    /// authorizer's owner_id was authorized by the first entry's
    /// `owner_changes` validates against the pending overlay (no
    /// on-chain owner_config row yet, but the overlay carries it).
    /// Walks the ordered chain rather than relying on alphabetical
    /// ordering.
    #[test]
    fn accepts_authorizer_chain_with_pending_overlay() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x51);
        let sender = signer.address();
        // Second authorizer signs as a different K1 owner — that
        // owner gets registered by the first ConfigChange.
        let second_signer = signer_for(0x52);
        let second_owner_id = implicit_owner_id(second_signer.address());

        let client = fresh_client(sender);
        let lock_slot = aa_lock_slot(sender);
        seed_account_config_deployed_with_storage(
            &client,
            [(B256::from(lock_slot.to_be_bytes::<32>()), pack_account_state(0, 0, 0))],
        );

        // Entry 1: authorize `second_owner_id` (K1, CONFIG scope).
        // Authorizer = signer (the implicit EOA self-owner).
        let mut entry1 = ConfigChangeEntry {
            chain_id: 0,
            sequence: 0,
            owner_changes: vec![OwnerChange {
                change_type: OP_AUTHORIZE_OWNER,
                owner_id: second_owner_id,
                verifier: K1_VERIFIER_ADDRESS,
                scope: OWNER_SCOPE_CONFIG,
            }],
            authorizer_auth: Default::default(),
        };
        entry1.authorizer_auth = k1_authorizer_auth(&signer, sender, &entry1);

        // Entry 2: authorize a third owner. Authorizer = `second_signer`,
        // whose owner_id was just authorized by entry 1 (overlay).
        let mut entry2 = ConfigChangeEntry {
            chain_id: 0,
            sequence: 1, // sequence advances by 1 after entry 1
            owner_changes: vec![OwnerChange {
                change_type: OP_AUTHORIZE_OWNER,
                owner_id: B256::repeat_byte(0xFE),
                verifier: K1_VERIFIER_ADDRESS,
                scope: OWNER_SCOPE_SENDER,
            }],
            authorizer_auth: Default::default(),
        };
        entry2.authorizer_auth = k1_authorizer_auth(&second_signer, sender, &entry2);

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.account_changes = vec![
            AccountChangeEntry::ConfigChange(entry1),
            AccountChangeEntry::ConfigChange(entry2),
        ];
        // Cover per-op + authorizer intrinsic gas (two K1 authorizers).
        tx.gas_limit = 500_000;
        re_sign_sender_auth(&mut tx, &signer);

        validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect("authorizer chain validating against pending overlay must admit");
    }

    // ---- Delegation gating ----

    /// Admission: a Delegation entry from a sender authenticated as
    /// their own K1 EOA self-owner with implicit-EOA scope admits.
    /// (`owner_config[sender][bytes20(sender)]` is empty → fallback
    /// rule grants CONFIG implicitly.)
    #[test]
    fn accepts_delegation_with_eoa_self_owner_and_config_scope() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x53);
        let sender = signer.address();
        let client = fresh_client(sender);

        // The Delegation entry triggers the lock check — seed an
        // unlocked account_state. ACCOUNT_CONFIG_DEPLOYED check is
        // NOT triggered by Delegation alone (no config writes), so
        // the contract code-hash isn't required.
        seed_account_state(&client, sender, 0, 0, 0);

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.account_changes = vec![AccountChangeEntry::Delegation(DelegationEntry {
            target: Address::repeat_byte(0xDE),
        })];
        re_sign_sender_auth(&mut tx, &signer);

        validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect("delegation with implicit-EOA self-owner must admit");
    }

    /// Spec: a Delegation entry from a sender NOT authenticated as
    /// their own EOA self-owner (e.g. authenticated via custom
    /// verifier) rejects. Peer-violation.
    ///
    /// TODO: while custom-verifier admission is temporarily rejected
    /// at the sender_auth check, this scenario is caught earlier as
    /// `InvalidSenderAuth` rather than `DelegationRequiresEoaSelfOwner`.
    /// Flip back to the delegation-specific error once Deferred
    /// admission lands.
    #[test]
    fn rejects_delegation_when_sender_auth_not_native_eoa() {
        const CHAIN_ID: u64 = 10;
        let sender = Address::repeat_byte(0xAB);
        let client = fresh_client(sender);

        let mut tx = make_tx(CHAIN_ID, sender, 0);
        let custom = Address::repeat_byte(0xBE);
        tx.sender_auth = explicit_custom_auth(custom);
        tx.account_changes = vec![AccountChangeEntry::Delegation(DelegationEntry {
            target: Address::repeat_byte(0xDE),
        })];

        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("delegation from custom-verifier sender must reject");
        assert!(matches!(err, Eip8130ValidationError::InvalidSenderAuth(_)));
        assert!(err.is_bad_transaction());
    }

    /// Spec: a Delegation entry where the sender's K1 EOA self-owner
    /// row is registered with a non-zero scope that omits the CONFIG
    /// bit rejects. State-disagreement.
    #[test]
    fn rejects_delegation_owner_lacks_config_scope() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x54);
        let sender = signer.address();
        let client = fresh_client(sender);

        // Seed: K1 self-owner registered explicitly with SENDER-only
        // scope (no CONFIG bit). The implicit-EOA fallback only fires
        // when verifier == ZERO; a non-empty K1-but-no-CONFIG row
        // takes the explicit path which fails the scope check.
        let owner_slot =
            aa_owner_config_slot(sender, U256::from_be_bytes(implicit_owner_id(sender).0));
        let lock_slot = aa_lock_slot(sender);
        client.add_account(
            ACCOUNT_CONFIG_ADDRESS,
            ExtendedAccount::new(0, U256::ZERO).extend_storage([
                (B256::from(lock_slot.to_be_bytes::<32>()), pack_account_state(0, 0, 0)),
                (
                    B256::from(owner_slot.to_be_bytes::<32>()),
                    pack_owner_config(K1_VERIFIER_ADDRESS, OWNER_SCOPE_SENDER),
                ),
            ]),
        );

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.account_changes = vec![AccountChangeEntry::Delegation(DelegationEntry {
            target: Address::repeat_byte(0xDE),
        })];
        re_sign_sender_auth(&mut tx, &signer);

        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("delegation with K1 owner missing CONFIG scope must reject");
        assert!(matches!(err, Eip8130ValidationError::DelegationOwnerLacksConfigScope { .. }));
    }

    // ---- ACCOUNT_CONFIG deployment ----

    /// Spec: a tx with config writes when ACCOUNT_CONFIG is not
    /// deployed rejects with `AccountConfigNotDeployed`.
    /// State-disagreement.
    #[test]
    fn rejects_when_account_config_not_deployed() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x55);
        let sender = signer.address();
        let client = fresh_client(sender);
        // Default `add_account` for ACCOUNT_CONFIG sets bytecode_hash =
        // None → contract not deployed.
        seed_account_state(&client, sender, 0, 0, 0);

        let mut cc = ConfigChangeEntry {
            chain_id: 0,
            sequence: 0,
            owner_changes: vec![OwnerChange {
                change_type: OP_AUTHORIZE_OWNER,
                owner_id: B256::repeat_byte(0x99),
                verifier: K1_VERIFIER_ADDRESS,
                scope: OWNER_SCOPE_SENDER,
            }],
            authorizer_auth: Default::default(),
        };
        cc.authorizer_auth = k1_authorizer_auth(&signer, sender, &cc);

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.account_changes = vec![AccountChangeEntry::ConfigChange(cc)];
        re_sign_sender_auth(&mut tx, &signer);

        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("config writes without ACCOUNT_CONFIG deployed must reject");
        assert!(matches!(err, Eip8130ValidationError::AccountConfigNotDeployed));
        assert!(!err.is_bad_transaction(), "AccountConfigNotDeployed is state-disagreement");
        // Silence dead_code on helpers used only in this module path.
        let _ = mark_account_config_deployed;
        let _ = OP_REVOKE_OWNER;
    }

    // -----------------------------------------------------------------
    // Delegate→Native inner-binding (correction pass).
    //
    // The executor's `dispatch_auth_state` (op-revm `handler.rs:676-710`)
    // runs TWO owner_config reads for a Delegate→Native auth:
    //   - outer: `owner_config[sender][bytes32(bytes20(delegate))]` binds
    //     DELEGATE_VERIFIER_ADDRESS;
    //   - inner: `owner_config[delegate_address][delegate_inner.owner_id]` binds the inner verifier
    //     (K1 / P256-raw / WebAuthn).
    // Admission must mirror both checks; otherwise a Delegate→Native tx
    // whose nested K1 sig is mathematically valid but whose recovered
    // inner owner is not registered on the delegate account would slip
    // through (the source of base's `verify_delegate` bug).
    // -----------------------------------------------------------------

    /// Builds a Delegate→K1 sender_auth blob:
    /// `[DELEGATE(20) || delegate_addr(20) || K1(20) || k1_sig(65)]`.
    /// Mirrors `delegate_native_inner_returns_native` in
    /// alloy-op-evm `auth_state.rs`.
    fn delegate_to_k1_sender_auth(
        inner_signer: &PrivateKeySigner,
        delegate: Address,
        hash: B256,
    ) -> Bytes {
        use op_revm::constants::DELEGATE_VERIFIER_ADDRESS;
        let mut buf = Vec::with_capacity(20 + 20 + 20 + 65);
        buf.extend_from_slice(DELEGATE_VERIFIER_ADDRESS.as_slice());
        buf.extend_from_slice(delegate.as_slice());
        buf.extend_from_slice(K1_VERIFIER_ADDRESS.as_slice());
        buf.extend_from_slice(&k1_sig_blob(inner_signer, hash));
        Bytes::from(buf)
    }

    /// Re-signs a Delegate→K1 sender_auth blob after mutating the tx.
    /// Mirrors `re_sign_sender_auth` for the Delegate path: the inner K1
    /// sig is over `sender_signature_hash(tx)` (same domain as direct K1).
    fn re_sign_delegate_to_k1_sender_auth(
        tx: &mut TxEip8130,
        inner_signer: &PrivateKeySigner,
        delegate: Address,
    ) {
        let hash = sender_signature_hash(tx);
        tx.sender_auth = delegate_to_k1_sender_auth(inner_signer, delegate, hash);
    }

    /// Delegate→K1 happy path: outer slot binds `(DELEGATE, scope)` for
    /// `owner_id = bytes32(bytes20(delegate))`; inner slot binds
    /// `(K1, scope)` for `owner_id = bytes32(bytes20(inner_signer))` on
    /// the delegate account. Both reads validate → admit.
    #[test]
    fn accepts_delegate_to_k1_inner_owner_config_validates() {
        use op_revm::constants::DELEGATE_VERIFIER_ADDRESS;
        const CHAIN_ID: u64 = 10;
        let inner_signer = signer_for(0x71);
        let sender = Address::repeat_byte(0xA1);
        let delegate = Address::repeat_byte(0xCD);
        let client = fresh_client(sender);

        // Outer slot: owner_config[sender][bytes20(delegate)] = (DELEGATE, SENDER).
        let outer_owner_id = {
            let mut buf = [0u8; 32];
            buf[..20].copy_from_slice(delegate.as_slice());
            B256::from(buf)
        };
        let outer_slot = aa_owner_config_slot(sender, U256::from_be_bytes(outer_owner_id.0));
        // Inner slot: owner_config[delegate][bytes20(inner_signer)] = (K1, SENDER).
        let inner_owner_id = implicit_owner_id(inner_signer.address());
        let inner_slot = aa_owner_config_slot(delegate, U256::from_be_bytes(inner_owner_id.0));

        client.add_account(
            ACCOUNT_CONFIG_ADDRESS,
            ExtendedAccount::new(0, U256::ZERO).extend_storage([
                (
                    B256::from(outer_slot.to_be_bytes::<32>()),
                    pack_owner_config(DELEGATE_VERIFIER_ADDRESS, OWNER_SCOPE_SENDER),
                ),
                (
                    B256::from(inner_slot.to_be_bytes::<32>()),
                    pack_owner_config(K1_VERIFIER_ADDRESS, OWNER_SCOPE_SENDER),
                ),
            ]),
        );

        let mut tx = make_tx(CHAIN_ID, sender, 0);
        re_sign_delegate_to_k1_sender_auth(&mut tx, &inner_signer, delegate);

        validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect("Delegate→K1 with both slots populated must admit");
    }

    /// Outer slot fine; inner slot is REVOKED → executor rejects with
    /// the inner binding's account in `OwnerRevoked`. Mirrors the
    /// executor's behavior at `handler.rs:697-708`.
    #[test]
    fn rejects_delegate_to_k1_inner_owner_revoked() {
        use op_revm::constants::DELEGATE_VERIFIER_ADDRESS;
        const CHAIN_ID: u64 = 10;
        let inner_signer = signer_for(0x72);
        let sender = Address::repeat_byte(0xA2);
        let delegate = Address::repeat_byte(0xCE);
        let client = fresh_client(sender);

        let outer_owner_id = {
            let mut buf = [0u8; 32];
            buf[..20].copy_from_slice(delegate.as_slice());
            B256::from(buf)
        };
        let outer_slot = aa_owner_config_slot(sender, U256::from_be_bytes(outer_owner_id.0));
        let inner_owner_id = implicit_owner_id(inner_signer.address());
        let inner_slot = aa_owner_config_slot(delegate, U256::from_be_bytes(inner_owner_id.0));

        client.add_account(
            ACCOUNT_CONFIG_ADDRESS,
            ExtendedAccount::new(0, U256::ZERO).extend_storage([
                (
                    B256::from(outer_slot.to_be_bytes::<32>()),
                    pack_owner_config(DELEGATE_VERIFIER_ADDRESS, OWNER_SCOPE_SENDER),
                ),
                (
                    B256::from(inner_slot.to_be_bytes::<32>()),
                    pack_owner_config(REVOKED_VERIFIER, 0),
                ),
            ]),
        );

        let mut tx = make_tx(CHAIN_ID, sender, 0);
        re_sign_delegate_to_k1_sender_auth(&mut tx, &inner_signer, delegate);

        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("Delegate→K1 with revoked inner must reject");
        match err {
            Eip8130ValidationError::OwnerRevoked { account, owner_id } => {
                assert_eq!(account, delegate, "OwnerRevoked must name the delegate account");
                assert_eq!(owner_id, inner_owner_id);
            }
            other => panic!("expected OwnerRevoked on inner slot, got {other:?}"),
        }
    }

    /// Outer slot fine; inner slot binds the wrong verifier (e.g. an
    /// off-chain registry repointing) → `OwnerConfigMismatch` keyed on
    /// the delegate account.
    #[test]
    fn rejects_delegate_to_k1_inner_verifier_mismatch() {
        use op_revm::constants::DELEGATE_VERIFIER_ADDRESS;
        const CHAIN_ID: u64 = 10;
        let inner_signer = signer_for(0x73);
        let sender = Address::repeat_byte(0xA3);
        let delegate = Address::repeat_byte(0xCF);
        let wrong_verifier = Address::repeat_byte(0xEE);
        let client = fresh_client(sender);

        let outer_owner_id = {
            let mut buf = [0u8; 32];
            buf[..20].copy_from_slice(delegate.as_slice());
            B256::from(buf)
        };
        let outer_slot = aa_owner_config_slot(sender, U256::from_be_bytes(outer_owner_id.0));
        let inner_owner_id = implicit_owner_id(inner_signer.address());
        let inner_slot = aa_owner_config_slot(delegate, U256::from_be_bytes(inner_owner_id.0));

        client.add_account(
            ACCOUNT_CONFIG_ADDRESS,
            ExtendedAccount::new(0, U256::ZERO).extend_storage([
                (
                    B256::from(outer_slot.to_be_bytes::<32>()),
                    pack_owner_config(DELEGATE_VERIFIER_ADDRESS, OWNER_SCOPE_SENDER),
                ),
                (
                    B256::from(inner_slot.to_be_bytes::<32>()),
                    pack_owner_config(wrong_verifier, OWNER_SCOPE_SENDER),
                ),
            ]),
        );

        let mut tx = make_tx(CHAIN_ID, sender, 0);
        re_sign_delegate_to_k1_sender_auth(&mut tx, &inner_signer, delegate);

        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("Delegate→K1 with mismatched inner verifier must reject");
        match err {
            Eip8130ValidationError::OwnerConfigMismatch { account, expected, on_chain, .. } => {
                assert_eq!(account, delegate);
                assert_eq!(expected, K1_VERIFIER_ADDRESS);
                assert_eq!(on_chain, wrong_verifier);
            }
            other => panic!("expected OwnerConfigMismatch on inner slot, got {other:?}"),
        }
    }

    /// Outer slot fine; inner slot is K1 but scope is non-zero and lacks
    /// the SENDER bit (e.g. CONFIG-only) → `OwnerScopeMissing`.
    #[test]
    fn rejects_delegate_to_k1_inner_scope_missing() {
        use op_revm::constants::DELEGATE_VERIFIER_ADDRESS;
        const CHAIN_ID: u64 = 10;
        let inner_signer = signer_for(0x74);
        let sender = Address::repeat_byte(0xA4);
        let delegate = Address::repeat_byte(0xC0);
        let client = fresh_client(sender);

        let outer_owner_id = {
            let mut buf = [0u8; 32];
            buf[..20].copy_from_slice(delegate.as_slice());
            B256::from(buf)
        };
        let outer_slot = aa_owner_config_slot(sender, U256::from_be_bytes(outer_owner_id.0));
        let inner_owner_id = implicit_owner_id(inner_signer.address());
        let inner_slot = aa_owner_config_slot(delegate, U256::from_be_bytes(inner_owner_id.0));

        // Inner: K1, but CONFIG-scope only (no SENDER bit).
        client.add_account(
            ACCOUNT_CONFIG_ADDRESS,
            ExtendedAccount::new(0, U256::ZERO).extend_storage([
                (
                    B256::from(outer_slot.to_be_bytes::<32>()),
                    pack_owner_config(DELEGATE_VERIFIER_ADDRESS, OWNER_SCOPE_SENDER),
                ),
                (
                    B256::from(inner_slot.to_be_bytes::<32>()),
                    pack_owner_config(K1_VERIFIER_ADDRESS, OWNER_SCOPE_CONFIG),
                ),
            ]),
        );

        let mut tx = make_tx(CHAIN_ID, sender, 0);
        re_sign_delegate_to_k1_sender_auth(&mut tx, &inner_signer, delegate);

        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("Delegate→K1 with inner missing SENDER scope must reject");
        match err {
            Eip8130ValidationError::OwnerScopeMissing { required_bit, on_chain_scope } => {
                assert_eq!(required_bit, OWNER_SCOPE_SENDER);
                assert_eq!(on_chain_scope, OWNER_SCOPE_CONFIG);
            }
            other => panic!("expected OwnerScopeMissing on inner slot, got {other:?}"),
        }
    }

    /// TODO: Delegate→Custom resolves to `AuthState::Deferred` (inner is
    /// custom), so admission is temporarily rejected for the same reason as
    /// pure custom verifier. Flip back to expect admission once the validator
    /// can run the STATICCALL.
    #[test]
    fn rejects_delegate_to_custom() {
        use op_revm::constants::DELEGATE_VERIFIER_ADDRESS;
        const CHAIN_ID: u64 = 10;
        let sender = Address::repeat_byte(0xA5);
        let delegate = Address::repeat_byte(0xC1);
        let inner_custom = Address::repeat_byte(0xEF);
        let client = fresh_client(sender);

        let mut blob = Vec::with_capacity(20 + 20 + 20 + 30);
        blob.extend_from_slice(DELEGATE_VERIFIER_ADDRESS.as_slice());
        blob.extend_from_slice(delegate.as_slice());
        blob.extend_from_slice(inner_custom.as_slice());
        blob.extend_from_slice(&[0xAB; 30]);

        let mut tx = make_tx(CHAIN_ID, sender, 0);
        tx.sender_auth = Bytes::from(blob);

        let err = validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect_err("Delegate→Custom admission temporarily rejected");
        assert!(matches!(err, Eip8130ValidationError::InvalidSenderAuth(_)));
    }

    // -----------------------------------------------------------------
    // Authorizer-chain dispatch correction.
    //
    // The executor's `validate_authorizer_chain` (op-revm
    // `handler.rs:765-844`) gates Native vs Custom on
    // `verify_call.is_some()` (alloy-op-evm `parts.rs:457-481` populates
    // `verify_call = None` for native verifiers including P256). The
    // admission path used to bucket all non-K1 verifiers into
    // `DeferredCustom`; that wrongly admitted P256-raw / WebAuthn
    // authorizer auths without re-validating their on-chain
    // owner_config row. The fix routes through `try_native_verify` (the
    // same dispatch the parts builder uses).
    // -----------------------------------------------------------------

    /// Custom-verifier authorizer (Deferred) still admits — verify the
    /// dispatch correction didn't break the existing custom path.
    #[test]
    fn admits_custom_staticcall_authorizer_after_dispatch_fix() {
        const CHAIN_ID: u64 = 10;
        let signer = signer_for(0x75);
        let sender = signer.address();
        let client = fresh_client(sender);

        let lock_slot = aa_lock_slot(sender);
        seed_account_config_deployed_with_storage(
            &client,
            [(B256::from(lock_slot.to_be_bytes::<32>()), pack_account_state(0, 0, 0))],
        );

        // Custom verifier address that is neither K1, Delegate, nor a
        // native sentinel → `try_native_verify` returns Unsupported →
        // DeferredCustom (admit, defer to executor STATICCALL).
        let custom = Address::repeat_byte(0x66);
        let mut blob = Vec::with_capacity(120);
        blob.extend_from_slice(custom.as_slice());
        blob.extend_from_slice(&[0x42u8; 100]);

        let mut tx = make_signed_tx(CHAIN_ID, &signer, 0);
        tx.account_changes = vec![AccountChangeEntry::ConfigChange(ConfigChangeEntry {
            chain_id: 0,
            sequence: 0,
            owner_changes: vec![OwnerChange {
                change_type: OP_AUTHORIZE_OWNER,
                owner_id: B256::repeat_byte(0xAB),
                verifier: K1_VERIFIER_ADDRESS,
                scope: OWNER_SCOPE_SENDER,
            }],
            authorizer_auth: Bytes::from(blob),
        })];
        re_sign_sender_auth(&mut tx, &signer);

        validate_eip8130_transaction(&tx, 1024, 0, CHAIN_ID, &client, U256::ZERO)
            .expect("custom-verifier authorizer must still admit after dispatch correction");
    }
}
