//! Proper mempool validation for EIP-8130 (AA) transactions.
//!
//! Validates nonce, expiry, sender/payer authorization (with native Rust
//! crypto for K1 and P256 verifiers), and payer balance before accepting
//! an AA transaction into the pending pool.
//!
//! Ported from base/crates/txpool/src/eip8130_validate.rs (1-508 + 668-1411).
//!
//! NOT YET PORTED:
//! - `verify_custom_via_evm` (base lines 509-666): runs custom verifier
//!   STATICCALL inside a sub-EVM during pool validation. Requires deeper
//!   wiring to spin up an OpEvm against a state provider, which isn't yet
//!   plumbed in op-reth's txpool. Custom-verifier paths return
//!   `VerifierNotAllowed` for now (effectively requires native verifiers
//!   for mempool admission). Tracked as a follow-up phase 6c-ii.
//! - `compute_account_tier` (base lines 1413+): depends on the throughput-
//!   tier types from the dual-pool architecture (Eip8130Pool) we haven't
//!   ported. Sender throughput tiering is deferred.

#![allow(dead_code, unused_imports)]

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use alloy_consensus::Transaction;
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use op_revm::{
    OpSpecId, OpTransaction,
    constants::DEFAULT_CUSTOM_VERIFIER_GAS_CAP,
    eip8130_policy::{
        PendingOwnerState, PendingOwnerValidationError, pending_owner_state_for_change,
        validate_pending_owner_state,
    },
};
use parking_lot::RwLock;
use reth_storage_api::StateProviderFactory;

use op_alloy_consensus::transaction::eip8130::{
    AA_TX_TYPE_ID, ACCOUNT_CONFIG_ADDRESS, AccountChangeEntry, DELEGATE_VERIFIER_ADDRESS,
    K1_VERIFIER_ADDRESS, MAX_CONFIG_OPS_PER_TX, NONCE_FREE_MAX_EXPIRY_WINDOW, NONCE_KEY_MAX,
    NONCE_MANAGER_ADDRESS, NativeVerifier, NativeVerifyResult, OwnerScope, ParsedSenderAuth,
    PurityScanner, PurityVerdict, REVOKED_VERIFIER, TxEip8130, ValidationError, VerifierGasCosts,
    config_change_digest, encode_verify_call, expiring_seen_slot, implicit_eoa_owner_id,
    intrinsic_gas, is_native_verifier, lock_slot, nonce_slot, owner_config_slot,
    parse_account_state, parse_owner_config, parse_sender_auth, payer_signature_hash,
    read_sequence, sender_signature_hash, sequence_base_slot, try_native_verify, validate_expiry,
    validate_structure,
};

use crate::{InvalidationKey, OpPooledTx, compute_invalidation_keys};

/// Controls which verifier contracts the mempool will accept in AA transactions.
///
/// Native verifier addresses (K1, P256, WebAuthn, Delegate) are always included
/// automatically, so callers only need to provide custom verifier addresses.
#[derive(Debug, Clone)]
pub struct VerifierAllowlist {
    allowed: HashSet<Address>,
}

impl VerifierAllowlist {
    /// Creates an allowlist from custom verifier addresses.
    ///
    /// The four native verifier addresses are added automatically.
    pub fn new(custom_addresses: impl IntoIterator<Item = Address>) -> Self {
        let mut allowed: HashSet<Address> = custom_addresses.into_iter().collect();
        allowed.extend(NativeVerifier::ALL.into_iter().map(NativeVerifier::address));
        Self { allowed }
    }

    /// Returns `true` if the given verifier address is allowed.
    pub fn is_allowed(&self, address: &Address) -> bool {
        self.allowed.contains(address)
    }
}

impl Default for VerifierAllowlist {
    fn default() -> Self {
        Self::new(std::iter::empty())
    }
}

/// Txpool admission mode for non-native EIP-8130 verifiers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CustomVerifierPolicy {
    /// Accept any custom verifier that passes runtime validation.
    Open,
    /// Accept only allowlisted custom verifier addresses.
    Allowlist,
    /// Accept only custom verifiers whose runtime bytecode is pure.
    Pure,
    /// Accept allowlisted custom verifiers, plus any verifier whose runtime
    /// bytecode is pure.
    #[default]
    AllowlistOrPure,
}

impl CustomVerifierPolicy {
    /// Returns `true` if allowlisted custom verifiers are accepted.
    pub const fn allows_allowlist(self) -> bool {
        matches!(self, Self::Allowlist | Self::AllowlistOrPure)
    }

    /// Returns `true` if pure custom verifiers are accepted.
    pub const fn allows_pure(self) -> bool {
        matches!(self, Self::Pure | Self::AllowlistOrPure)
    }

    /// Returns `true` if the txpool is fully permissive for custom verifiers.
    pub const fn is_open(self) -> bool {
        matches!(self, Self::Open)
    }
}

/// Mempool admission policy for EIP-8130 verifiers.
///
/// Native verifiers are always admitted. This policy only controls whether
/// custom verifiers are accepted.
#[derive(Debug, Clone, Default)]
pub struct VerifierAdmissionPolicy {
    mode: CustomVerifierPolicy,
    allowlist: VerifierAllowlist,
}

impl VerifierAdmissionPolicy {
    /// Creates a new verifier admission policy.
    pub fn new(mode: CustomVerifierPolicy, allowlist: VerifierAllowlist) -> Self {
        Self { mode, allowlist }
    }

    /// Creates a fully permissive policy for custom verifiers.
    pub fn open() -> Self {
        Self::new(CustomVerifierPolicy::Open, VerifierAllowlist::default())
    }

    /// Creates a policy that accepts only allowlisted custom verifiers.
    pub fn allowlist_only(allowlist: VerifierAllowlist) -> Self {
        Self::new(CustomVerifierPolicy::Allowlist, allowlist)
    }

    /// Creates a policy that accepts only pure custom verifiers.
    pub fn pure_only() -> Self {
        Self::new(CustomVerifierPolicy::Pure, VerifierAllowlist::default())
    }

    /// Creates a policy that accepts allowlisted or pure custom verifiers.
    pub fn allowlist_or_pure(allowlist: VerifierAllowlist) -> Self {
        Self::new(CustomVerifierPolicy::AllowlistOrPure, allowlist)
    }

    /// Returns the configured custom verifier mode.
    pub const fn mode(&self) -> CustomVerifierPolicy {
        self.mode
    }

    /// Returns the configured verifier allowlist.
    pub fn allowlist(&self) -> &VerifierAllowlist {
        &self.allowlist
    }
}

/// Cache of purity verdicts keyed by verifier runtime bytecode hash.
#[derive(Debug, Default)]
pub struct VerifierPurityCache {
    by_code_hash: RwLock<HashMap<B256, PurityVerdict>>,
}

impl VerifierPurityCache {
    fn verdict_for(&self, runtime_code_hash: B256, runtime_code: &[u8]) -> PurityVerdict {
        if let Some(verdict) = self.by_code_hash.read().get(&runtime_code_hash).cloned() {
            return verdict;
        }

        let verdict = PurityScanner::analyze(runtime_code);
        self.by_code_hash
            .write()
            .entry(runtime_code_hash)
            .or_insert_with(|| verdict.clone())
            .clone()
    }
}

/// Successful AA validation outcome, providing the data the txpool needs for
/// ordering and balance tracking.
#[derive(Debug)]
pub struct Eip8130ValidationOutcome {
    /// Payer's balance (used for txpool cost checks).
    pub balance: U256,
    /// The sender's current nonce_sequence (used for txpool nonce ordering).
    pub state_nonce: u64,
    /// The nonce key from the transaction. Used by the 2D nonce pool to
    /// route transactions with `nonce_key != 0` to the dedicated pool.
    pub nonce_key: U256,
    /// The resolved sender owner ID (from native signature verification).
    pub sender_owner_id: B256,
    /// Storage slot dependencies for invalidation tracking.
    pub invalidation_keys: HashSet<InvalidationKey>,
    /// The resolved payer address. `None` for self-pay transactions.
    /// Used for payer pending count tracking.
    pub sponsored_payer: Option<Address>,
    /// Unix timestamp after which this transaction is invalid. `0` = no expiry.
    pub expiry: u64,
}

/// Errors from AA transaction validation.
#[derive(Debug)]
pub enum Eip8130ValidationError {
    /// Failed to decode the `TxEip8130` from 2718-encoded bytes.
    DecodeFailed(String),
    /// AA transaction exceeds the txpool ingress encoded size cap.
    TxTooLarge {
        /// Encoded transaction size in bytes.
        size: usize,
        /// Maximum allowed encoded size in bytes.
        limit: usize,
    },
    /// Structural validation failed (sizes, nonce_key range, account_changes).
    Structural(ValidationError),
    /// Transaction chain_id does not match the network.
    ChainIdMismatch {
        /// Network chain_id.
        expected: u64,
        /// Transaction's chain_id.
        got: u64,
    },
    /// Transaction has expired.
    Expired {
        /// Transaction's expiry timestamp.
        expiry: u64,
        /// Current block timestamp.
        current: u64,
    },
    /// Nonce does not match the on-chain value.
    NonceMismatch {
        /// On-chain nonce.
        expected: u64,
        /// Transaction's nonce_sequence.
        got: u64,
    },
    /// `sender_auth` is malformed or signature verification failed.
    SenderAuthInvalid(String),
    /// Sender's owner is not authorized in AccountConfig.
    SenderNotAuthorized(String),
    /// `payer_auth` is malformed or signature verification failed.
    PayerAuthInvalid(String),
    /// Payer's owner is not authorized in AccountConfig.
    PayerNotAuthorized(String),
    /// Verifier address is not admitted by the mempool policy.
    VerifierNotAllowed(Address),
    /// Custom verifier STATICCALL failed in the txpool EVM.
    CustomVerifierCallFailed(String),
    /// Custom verifier has EIP-7702 delegation bytecode prefix.
    VerifierEip7702Delegated(Address),
    /// Config change authorizer auth is invalid.
    AuthorizerAuthInvalid(String),
    /// Config change authorizer lacks CONFIG scope or is not a recognized owner.
    AuthorizerNotAuthorized(String),
    /// Too many config operations in a single transaction.
    TooManyConfigOperations {
        /// Number of operations in the transaction.
        count: usize,
        /// Maximum allowed.
        limit: usize,
    },
    /// Account is locked; config changes are rejected.
    AccountLocked,
    /// AccountConfiguration contract has not been deployed yet.
    AccountConfigNotDeployed,
    /// Config change sequence does not match on-chain value.
    SequenceMismatch {
        /// On-chain sequence.
        expected: u64,
        /// Sequence in the transaction.
        got: u64,
    },
    /// Gas limit is below the intrinsic gas cost.
    IntrinsicGasTooLow {
        /// Minimum required gas.
        intrinsic: u64,
        /// Gas limit in the transaction.
        gas_limit: u64,
    },
    /// Payer has insufficient balance to cover `gas_limit * max_fee_per_gas`.
    InsufficientBalance {
        /// Required balance.
        required: U256,
        /// Available balance.
        available: U256,
    },
    /// Nonce-free transaction's expiry is too far in the future.
    NonceFreeExpiryTooFar {
        /// Transaction's expiry timestamp.
        expiry: u64,
        /// Maximum allowed expiry.
        max_allowed: u64,
    },
    /// Nonce-free transaction hash already recorded in the on-chain seen set.
    NonceFreeReplay,
    /// Error reading on-chain state.
    StateError(String),
}

impl std::fmt::Display for Eip8130ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DecodeFailed(e) => write!(f, "decode failed: {e}"),
            Self::TxTooLarge { size, limit } => {
                write!(f, "AA tx too large (size={size}, limit={limit})")
            }
            Self::Structural(e) => write!(f, "structural: {e}"),
            Self::ChainIdMismatch { expected, got } => {
                write!(f, "chain_id mismatch (expected={expected}, got={got})")
            }
            Self::Expired { expiry, current } => {
                write!(f, "expired (expiry={expiry}, current={current})")
            }
            Self::NonceMismatch { expected, got } => {
                write!(f, "nonce mismatch (expected={expected}, got={got})")
            }
            Self::VerifierNotAllowed(addr) => {
                write!(f, "verifier {addr} rejected by mempool policy")
            }
            Self::CustomVerifierCallFailed(e) => {
                write!(f, "custom verifier STATICCALL failed: {e}")
            }
            Self::VerifierEip7702Delegated(addr) => {
                write!(f, "verifier {addr} has EIP-7702 delegation prefix")
            }
            Self::SenderAuthInvalid(e) => write!(f, "sender auth invalid: {e}"),
            Self::SenderNotAuthorized(e) => write!(f, "sender not authorized: {e}"),
            Self::PayerAuthInvalid(e) => write!(f, "payer auth invalid: {e}"),
            Self::PayerNotAuthorized(e) => write!(f, "payer not authorized: {e}"),
            Self::AuthorizerAuthInvalid(e) => write!(f, "authorizer auth invalid: {e}"),
            Self::AuthorizerNotAuthorized(e) => write!(f, "authorizer not authorized: {e}"),
            Self::TooManyConfigOperations { count, limit } => {
                write!(f, "too many config operations ({count}/{limit})")
            }
            Self::AccountLocked => write!(f, "account is locked"),
            Self::AccountConfigNotDeployed => {
                write!(f, "AccountConfiguration contract not deployed")
            }
            Self::SequenceMismatch { expected, got } => {
                write!(f, "config change sequence mismatch (expected={expected}, got={got})")
            }
            Self::IntrinsicGasTooLow { intrinsic, gas_limit } => {
                write!(f, "gas limit below intrinsic (intrinsic={intrinsic}, limit={gas_limit})")
            }
            Self::InsufficientBalance { required, available } => {
                write!(f, "payer insufficient balance (required={required}, available={available})")
            }
            Self::NonceFreeExpiryTooFar { expiry, max_allowed } => {
                write!(f, "nonce-free expiry too far: expiry={expiry}, max_allowed={max_allowed}")
            }
            Self::NonceFreeReplay => {
                write!(f, "nonce-free transaction replay: hash already seen")
            }
            Self::StateError(e) => write!(f, "state access error: {e}"),
        }
    }
}

impl std::error::Error for Eip8130ValidationError {}

impl reth_transaction_pool::error::PoolTransactionError for Eip8130ValidationError {
    fn is_bad_transaction(&self) -> bool {
        matches!(
            self,
            Self::Structural(_)
                | Self::DecodeFailed(_)
                | Self::TxTooLarge { .. }
                | Self::ChainIdMismatch { .. }
                | Self::CustomVerifierCallFailed(_)
                | Self::VerifierEip7702Delegated(_)
                | Self::SenderAuthInvalid(_)
                | Self::PayerAuthInvalid(_)
                | Self::AuthorizerAuthInvalid(_)
                | Self::TooManyConfigOperations { .. }
        )
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Extracts `TxEip8130` from a pool transaction, avoiding re-encode/re-decode.
fn decode_tx_eip8130<Tx: OpPooledTx>(
    transaction: &Tx,
) -> Result<TxEip8130, Eip8130ValidationError> {
    transaction
        .as_eip8130()
        .cloned()
        .ok_or_else(|| Eip8130ValidationError::DecodeFailed("not an AA transaction".into()))
}

/// Reads a storage slot from a state provider, returning U256::ZERO if absent.
fn read_storage(
    state: &dyn reth_storage_api::StateProvider,
    address: Address,
    slot: B256,
) -> Result<U256, Eip8130ValidationError> {
    state
        .storage(address, slot.into())
        .map(|v| v.unwrap_or_default())
        .map_err(|e| Eip8130ValidationError::StateError(e.to_string()))
}

/// Reads `owner_config(account, owner_id)` from AccountConfig storage.
///
/// Returns `(verifier_address, scope)`.
fn read_owner_config_from_state(
    state: &dyn reth_storage_api::StateProvider,
    account: Address,
    owner_id: B256,
) -> Result<(Address, u8), Eip8130ValidationError> {
    let slot = owner_config_slot(account, owner_id);
    let value = read_storage(state, ACCOUNT_CONFIG_ADDRESS, slot)?;
    Ok(parse_owner_config(B256::from(value.to_be_bytes::<32>())))
}

/// Resolves the sender address for an AA transaction.
///
/// For EOA mode (`from == Address::ZERO`): ecrecovers the sender from
/// `sender_auth` and returns the recovered address plus the owner_id.
///
/// For configured mode: returns `tx.from` as the sender. The owner_id
/// is not yet validated (done in `validate_sender_authorization`).
fn resolve_sender_address(tx: &TxEip8130) -> Result<(Address, B256), Eip8130ValidationError> {
    let parsed =
        parse_sender_auth(tx).map_err(|e| Eip8130ValidationError::SenderAuthInvalid(e.into()))?;

    match parsed {
        ParsedSenderAuth::Eoa { signature } => {
            let sig_hash = sender_signature_hash(tx);
            let sig_bytes = Bytes::copy_from_slice(&signature);
            let result = try_native_verify(K1_VERIFIER_ADDRESS, &sig_bytes, sig_hash);
            match result {
                NativeVerifyResult::Verified(owner_id) => {
                    let recovered = Address::from_slice(&owner_id.as_slice()[..20]);
                    Ok((recovered, owner_id))
                }
                NativeVerifyResult::Invalid(e) => {
                    Err(Eip8130ValidationError::SenderAuthInvalid(e.to_string()))
                }
                NativeVerifyResult::Unsupported => Err(Eip8130ValidationError::SenderAuthInvalid(
                    "K1 should be natively supported".into(),
                )),
            }
        }
        ParsedSenderAuth::Configured { .. } => {
            tx.from.map(|from| (from, B256::ZERO)).ok_or(Eip8130ValidationError::SenderAuthInvalid(
                "configured sender must set from field".to_string(),
            ))
        }
    }
}

/// Default gas limit for custom verifier STATICCALLs in the txpool.
///
/// Matches [`DEFAULT_CUSTOM_VERIFIER_GAS_CAP`] from the execution layer.
/// Override via [`OpTransactionValidator::with_custom_verifier_gas_limit`].
pub const DEFAULT_CUSTOM_VERIFIER_GAS_LIMIT: u64 = DEFAULT_CUSTOM_VERIFIER_GAS_CAP;

/// Maximum EIP-2718 encoded size accepted for a single AA transaction at txpool ingress.
///
/// This is a pragmatic first guard for parse/allocation amplification: oversized
/// AA envelopes are rejected before running deeper stateful validation.
pub const MAX_AA_TX_ENCODED_BYTES: usize = 128 * 1024;

fn ensure_custom_verifier_admitted(
    state: &dyn reth_storage_api::StateProvider,
    verifier: Address,
    verifier_policy: &VerifierAdmissionPolicy,
    purity_cache: &VerifierPurityCache,
) -> Result<(), Eip8130ValidationError> {
    debug_assert!(!is_native_verifier(verifier), "native verifiers bypass custom policy");

    let code = state
        .account_code(&verifier)
        .map_err(|e| Eip8130ValidationError::StateError(e.to_string()))?;

    if code.as_ref().is_some_and(|code| code.original_bytes().starts_with(&[0xef, 0x01, 0x00])) {
        return Err(Eip8130ValidationError::VerifierEip7702Delegated(verifier));
    }

    if verifier_policy.mode().is_open() {
        return Ok(());
    }

    if verifier_policy.mode().allows_allowlist() && verifier_policy.allowlist().is_allowed(&verifier)
    {
        return Ok(());
    }

    if !verifier_policy.mode().allows_pure() {
        return Err(Eip8130ValidationError::VerifierNotAllowed(verifier));
    }

    let Some(code) = code else {
        return Err(Eip8130ValidationError::VerifierNotAllowed(verifier));
    };

    let verdict =
        purity_cache.verdict_for(keccak256(code.original_bytes()), &code.original_bytes());
    if verdict.is_pure() {
        Ok(())
    } else {
        Err(Eip8130ValidationError::VerifierNotAllowed(verifier))
    }
}

/// Executes a custom verifier's `IVerifier.verify(hash, data)` via a
/// lightweight EVM STATICCALL and validates the returned owner_id against
/// the on-chain owner_config.
///
/// Returns the authenticated `owner_id` on success.
fn verify_auth_with_scope(
    state: &dyn reth_storage_api::StateProvider,
    tx: &TxEip8130,
    sender: Address,
    verifier: Address,
    data: &Bytes,
    sig_hash: B256,
    caller: Address,
    account: Address,
    required_scope: u8,
    role: OwnerRole,
    verifier_policy: &VerifierAdmissionPolicy,
    purity_cache: &VerifierPurityCache,
    remaining_custom_verifier_gas: &mut u64,
    pending_owners: Option<&HashMap<B256, PendingOwnerState>>,
) -> Result<B256, Eip8130ValidationError> {
    if verifier == DELEGATE_VERIFIER_ADDRESS {
        return verify_delegate_auth_with_scope(
            state,
            tx,
            sender,
            data,
            sig_hash,
            caller,
            account,
            required_scope,
            role,
            verifier_policy,
            purity_cache,
            remaining_custom_verifier_gas,
            pending_owners,
        );
    }

    match try_native_verify(verifier, data, sig_hash) {
        NativeVerifyResult::Verified(owner_id) => {
            if let Some(pending) = pending_owners {
                check_owner_authorized_with_pending(
                    state,
                    account,
                    owner_id,
                    verifier,
                    required_scope,
                    pending,
                    role,
                )?;
            } else {
                check_owner_authorized(state, account, owner_id, verifier, required_scope, role)?;
            }
            Ok(owner_id)
        }
        NativeVerifyResult::Invalid(e) => Err(role.auth_invalid(e.to_string())),
        NativeVerifyResult::Unsupported => {
            // TODO(eip-8130 phase 6c-ii): port verify_custom_via_evm (sub-EVM
            // STATICCALL into custom verifier). For first cut, the txpool only
            // admits transactions whose verifiers are natively supported (K1 /
            // P256-raw / P256-WebAuthn / Delegate). Custom verifier paths are
            // gated here.
            ensure_custom_verifier_admitted(state, verifier, verifier_policy, purity_cache)?;
            let _ = (tx, caller, sig_hash, data, remaining_custom_verifier_gas, pending_owners);
            Err(role.auth_invalid(format!(
                "custom verifier {verifier} not yet supported in txpool (phase 6c-ii)"
            )))
        }
    }
}

/// Verifies canonical delegate auth:
/// `delegate_account(20) || nested_auth(verifier(20) || data...)`.
///
/// For nested native verifiers, both delegate resolution and nested signature
/// verification stay native (no EVM call). For nested custom verifiers, only
/// the nested verifier executes via STATICCALL.
fn verify_delegate_auth_with_scope(
    state: &dyn reth_storage_api::StateProvider,
    tx: &TxEip8130,
    sender: Address,
    delegate_data: &Bytes,
    sig_hash: B256,
    caller: Address,
    account: Address,
    required_scope: u8,
    role: OwnerRole,
    verifier_policy: &VerifierAdmissionPolicy,
    purity_cache: &VerifierPurityCache,
    remaining_custom_verifier_gas: &mut u64,
    pending_owners: Option<&HashMap<B256, PendingOwnerState>>,
) -> Result<B256, Eip8130ValidationError> {
    if delegate_data.len() < 40 {
        return Err(role.auth_invalid(format!(
            "delegate auth too short: expected >= 40 bytes, got {}",
            delegate_data.len()
        )));
    }

    let delegate_account = Address::from_slice(&delegate_data[..20]);
    let nested_verifier = Address::from_slice(&delegate_data[20..40]);
    let nested_data = Bytes::copy_from_slice(&delegate_data[40..]);

    if nested_verifier == DELEGATE_VERIFIER_ADDRESS {
        return Err(role.auth_invalid("nested delegation is not allowed".into()));
    }

    // Outer delegate owner check on the source account.
    let delegate_owner_id = implicit_eoa_owner_id(delegate_account);
    if let Some(pending) = pending_owners {
        check_owner_authorized_with_pending(
            state,
            account,
            delegate_owner_id,
            DELEGATE_VERIFIER_ADDRESS,
            required_scope,
            pending,
            role,
        )?;
    } else {
        check_owner_authorized(
            state,
            account,
            delegate_owner_id,
            DELEGATE_VERIFIER_ADDRESS,
            required_scope,
            role,
        )?;
    }

    // Nested verifier check on the delegate account.
    if nested_verifier == Address::ZERO {
        // Implicit EOA nested path: recover signer and enforce it is the delegate.
        let recovered_owner_id =
            match try_native_verify(K1_VERIFIER_ADDRESS, &nested_data, sig_hash) {
                NativeVerifyResult::Verified(owner_id) => owner_id,
                NativeVerifyResult::Invalid(e) => return Err(role.auth_invalid(e.to_string())),
                NativeVerifyResult::Unsupported => {
                    return Err(role.auth_invalid("K1 nested verifier unsupported".into()));
                }
            };
        if recovered_owner_id != delegate_owner_id {
            return Err(role
                .not_authorized("delegate nested signature recovered non-delegate signer".into()));
        }
        check_owner_authorized(
            state,
            delegate_account,
            recovered_owner_id,
            K1_VERIFIER_ADDRESS,
            required_scope,
            role,
        )?;
        return Ok(delegate_owner_id);
    }

    if is_native_verifier(nested_verifier) {
        let nested_owner_id = match try_native_verify(nested_verifier, &nested_data, sig_hash) {
            NativeVerifyResult::Verified(owner_id) => owner_id,
            NativeVerifyResult::Invalid(e) => return Err(role.auth_invalid(e.to_string())),
            NativeVerifyResult::Unsupported => {
                return Err(
                    role.auth_invalid("nested native verifier unexpectedly unsupported".into())
                );
            }
        };
        check_owner_authorized(
            state,
            delegate_account,
            nested_owner_id,
            nested_verifier,
            required_scope,
            role,
        )?;
        return Ok(delegate_owner_id);
    }

    ensure_custom_verifier_admitted(state, nested_verifier, verifier_policy, purity_cache)?;

    // TODO(eip-8130 phase 6c-ii): nested custom verifier under DELEGATE auth
    // requires verify_custom_via_evm. Reject for now so the txpool only admits
    // delegate auth where the inner verifier is also native.
    let _ = (tx, sender, sig_hash, delegate_data, caller, account, required_scope, role,
        remaining_custom_verifier_gas, pending_owners, &nested_data, delegate_account);
    Err(role.auth_invalid(format!(
        "delegate auth with nested custom verifier {nested_verifier} not yet supported (phase 6c-ii)"
    )))
}

/// Validates `sender_auth` authorization against on-chain owner_config.
///
/// For EOA mode: checks the already-recovered `owner_id` against AccountConfig.
/// For configured mode: parses the verifier, attempts native verification,
/// and checks the owner_config for SENDER scope.
fn validate_sender_authorization(
    tx: &TxEip8130,
    sender: Address,
    eoa_owner_id: B256,
    state: &dyn reth_storage_api::StateProvider,
    verifier_policy: &VerifierAdmissionPolicy,
    purity_cache: &VerifierPurityCache,
    remaining_custom_verifier_gas: &mut u64,
    pending_owners: Option<&HashMap<B256, PendingOwnerState>>,
) -> Result<B256, Eip8130ValidationError> {
    if tx.is_eoa() {
        if let Some(pending) = pending_owners {
            check_owner_authorized_with_pending(
                state,
                sender,
                eoa_owner_id,
                K1_VERIFIER_ADDRESS,
                OwnerScope::SENDER,
                pending,
                OwnerRole::Sender,
            )?;
        } else {
            check_owner_authorized(
                state,
                sender,
                eoa_owner_id,
                K1_VERIFIER_ADDRESS,
                OwnerScope::SENDER,
                OwnerRole::Sender,
            )?;
        }
        return Ok(eoa_owner_id);
    }

    let parsed =
        parse_sender_auth(tx).map_err(|e| Eip8130ValidationError::SenderAuthInvalid(e.into()))?;
    let sig_hash = sender_signature_hash(tx);

    match parsed {
        ParsedSenderAuth::Eoa { .. } => unreachable!("handled above"),
        ParsedSenderAuth::Configured { verifier, data } => verify_auth_with_scope(
            state,
            tx,
            sender,
            verifier,
            &data,
            sig_hash,
            sender,
            sender,
            OwnerScope::SENDER,
            OwnerRole::Sender,
            verifier_policy,
            purity_cache,
            remaining_custom_verifier_gas,
            pending_owners,
        ),
    }
}

/// Validates `payer_auth` for a sponsored AA transaction.
///
/// Returns the authenticated payer `owner_id` on success.
fn validate_payer(
    tx: &TxEip8130,
    sender: Address,
    payer: Address,
    state: &dyn reth_storage_api::StateProvider,
    verifier_policy: &VerifierAdmissionPolicy,
    purity_cache: &VerifierPurityCache,
    remaining_custom_verifier_gas: &mut u64,
    pending_owners: Option<&HashMap<B256, PendingOwnerState>>,
) -> Result<B256, Eip8130ValidationError> {
    if tx.payer_auth.len() < 20 {
        return Err(Eip8130ValidationError::PayerAuthInvalid(
            "payer_auth too short for verifier address".into(),
        ));
    }

    let sig_hash = payer_signature_hash(tx);

    let verifier = Address::from_slice(&tx.payer_auth[..20]);
    let data = Bytes::copy_from_slice(&tx.payer_auth[20..]);

    verify_auth_with_scope(
        state,
        tx,
        sender,
        verifier,
        &data,
        sig_hash,
        sender,
        payer,
        OwnerScope::PAYER,
        OwnerRole::Payer,
        verifier_policy,
        purity_cache,
        remaining_custom_verifier_gas,
        pending_owners,
    )
}

/// Checks that the owner_config for `(account, owner_id)` authorizes the given
/// verifier and has the required scope bit.
///
/// Implements the implicit EOA rule: if the slot is empty and
/// `owner_id == bytes32(bytes20(account))`, the K1 verifier is authorized.
fn check_owner_authorized(
    state: &dyn reth_storage_api::StateProvider,
    account: Address,
    owner_id: B256,
    expected_verifier: Address,
    required_scope: u8,
    role: OwnerRole,
) -> Result<(), Eip8130ValidationError> {
    let (verifier, scope) = read_owner_config_from_state(state, account, owner_id)?;

    if verifier == REVOKED_VERIFIER {
        return Err(role.not_authorized("owner explicitly revoked".into()));
    }

    if verifier != Address::ZERO {
        if verifier != expected_verifier {
            return Err(role.not_authorized(format!(
                "owner_config verifier mismatch: expected {expected_verifier}, got {verifier}"
            )));
        }
        if scope != 0 && (scope & required_scope) == 0 {
            return Err(role
                .not_authorized(format!("owner lacks required scope bit 0x{required_scope:02x}")));
        }
        return Ok(());
    }

    // verifier == address(0): empty slot, implicit EOA rule.
    let implicit_id = implicit_eoa_owner_id(account);
    if owner_id == implicit_id && expected_verifier == K1_VERIFIER_ADDRESS {
        return Ok(());
    }

    Err(role.not_authorized("no owner_config and implicit EOA rule doesn't apply".into()))
}

/// Distinguishes between sender, payer, and authorizer roles for error reporting.
#[derive(Debug, Clone, Copy)]
enum OwnerRole {
    Sender,
    Payer,
    Authorizer,
}

impl OwnerRole {
    fn not_authorized(self, detail: String) -> Eip8130ValidationError {
        match self {
            Self::Sender => Eip8130ValidationError::SenderNotAuthorized(detail),
            Self::Payer => Eip8130ValidationError::PayerNotAuthorized(detail),
            Self::Authorizer => Eip8130ValidationError::AuthorizerNotAuthorized(detail),
        }
    }

    fn auth_invalid(self, detail: String) -> Eip8130ValidationError {
        match self {
            Self::Sender => Eip8130ValidationError::SenderAuthInvalid(detail),
            Self::Payer => Eip8130ValidationError::PayerAuthInvalid(detail),
            Self::Authorizer => Eip8130ValidationError::AuthorizerAuthInvalid(detail),
        }
    }
}

/// Validates the authorizer chain for config change entries at mempool time.
///
/// For each `ConfigChangeEntry`:
/// 1. Computes the config change digest.
/// 2. Parses `authorizer_auth` and verifies the signature (native or custom).
/// 3. Checks the authenticated owner_id has CONFIG scope in `owner_config`.
/// 4. Tracks pending additions in-memory for chained authorization.
///
/// Also validates authorizer custom verifiers against the mempool admission
/// policy and rejects verifiers with EIP-7702 delegation bytecode.
fn validate_authorizer_chain(
    tx: &TxEip8130,
    sender: Address,
    state: &dyn reth_storage_api::StateProvider,
    verifier_policy: &VerifierAdmissionPolicy,
    purity_cache: &VerifierPurityCache,
    remaining_custom_verifier_gas: &mut u64,
) -> Result<HashMap<B256, PendingOwnerState>, Eip8130ValidationError> {
    let mut pending_owners: HashMap<B256, PendingOwnerState> = HashMap::new();

    for entry in &tx.account_changes {
        let cc = match entry {
            AccountChangeEntry::ConfigChange(cc) => cc,
            _ => continue,
        };

        if cc.authorizer_auth.len() < 20 {
            return Err(Eip8130ValidationError::AuthorizerAuthInvalid(
                "authorizer_auth too short for verifier address".into(),
            ));
        }

        let digest = config_change_digest(sender, cc);
        let auth = &cc.authorizer_auth;
        let verifier = Address::from_slice(&auth[..20]);
        let data = Bytes::copy_from_slice(&auth[20..]);

        verify_auth_with_scope(
            state,
            tx,
            sender,
            verifier,
            &data,
            digest,
            sender,
            sender,
            OwnerScope::CONFIG,
            OwnerRole::Authorizer,
            verifier_policy,
            purity_cache,
            remaining_custom_verifier_gas,
            Some(&pending_owners),
        )?;

        // Track pending additions/revocations for chaining.
        for op in &cc.owner_changes {
            if let Some(state) =
                pending_owner_state_for_change(op.change_type, op.verifier, op.scope)
            {
                pending_owners.insert(op.owner_id, state);
            }
        }
    }

    Ok(pending_owners)
}

/// Like [`check_owner_authorized`] but also checks pending additions from
/// earlier config change entries in the chain. Pending owners take priority
/// over on-chain state, enabling chained authorization within a single tx.
fn check_owner_authorized_with_pending(
    state: &dyn reth_storage_api::StateProvider,
    account: Address,
    owner_id: B256,
    expected_verifier: Address,
    required_scope: u8,
    pending_owners: &HashMap<B256, PendingOwnerState>,
    role: OwnerRole,
) -> Result<(), Eip8130ValidationError> {
    if let Some(state_override) = pending_owners.get(&owner_id) {
        validate_pending_owner_state(state_override, expected_verifier, required_scope).map_err(
            |err| match err {
                PendingOwnerValidationError::Revoked => {
                    role.not_authorized("owner explicitly revoked in pending config changes".into())
                }
                PendingOwnerValidationError::VerifierMismatch { expected, actual } => role
                    .not_authorized(format!(
                        "pending owner verifier mismatch: expected {expected}, got {actual}",
                    )),
                PendingOwnerValidationError::MissingScope { required_scope } => role
                    .not_authorized(format!(
                        "pending owner lacks required scope 0x{required_scope:02x}",
                    )),
            },
        )?;
        return Ok(());
    }

    check_owner_authorized(state, account, owner_id, expected_verifier, required_scope, role)
}

/// Full AA transaction validation pipeline for the mempool.
///
/// Validates structural integrity, expiry, chain_id, nonce, sender/payer
/// authorization, and payer balance. Returns the data the txpool needs to
/// order and track the transaction.
pub fn validate_eip8130_transaction<Tx, Client>(
    transaction: &Tx,
    block_timestamp: u64,
    chain_id: u64,
    client: &Client,
    verifier_policy: &VerifierAdmissionPolicy,
    purity_cache: &VerifierPurityCache,
    custom_verifier_gas_limit: u64,
    _trusted_payer_bytecodes: &HashSet<B256>,
) -> Result<Eip8130ValidationOutcome, Eip8130ValidationError>
where
    Tx: OpPooledTx + Transaction,
    Client: StateProviderFactory,
{
    let encoded_len = transaction.encoded_2718().len();
    if encoded_len > MAX_AA_TX_ENCODED_BYTES {
        return Err(Eip8130ValidationError::TxTooLarge {
            size: encoded_len,
            limit: MAX_AA_TX_ENCODED_BYTES,
        });
    }

    let tx = decode_tx_eip8130(transaction)?;

    // 1. Structural validation (no state needed)
    validate_structure(&tx).map_err(Eip8130ValidationError::Structural)?;

    // 1b. Chain ID must match the network.
    if tx.chain_id != chain_id {
        return Err(Eip8130ValidationError::ChainIdMismatch {
            expected: chain_id,
            got: tx.chain_id,
        });
    }

    // 2. Expiry check
    validate_expiry(&tx, block_timestamp).map_err(|e| match e {
        ValidationError::Expired { expiry, current } => {
            Eip8130ValidationError::Expired { expiry, current }
        }
        other => Eip8130ValidationError::Structural(other),
    })?;

    // 3. Resolve the sender address. For EOA mode (`from == Address::ZERO`),
    //    ecrecover derives the real sender. This must happen before any state
    //    reads that key on the sender address (nonce, lock, sequence, balance).
    let (sender, eoa_owner_id) = resolve_sender_address(&tx)?;

    // 4. Open state provider for storage reads
    let state = client.latest().map_err(|e| Eip8130ValidationError::StateError(e.to_string()))?;

    // 5. Nonce validation (skipped in nonce-free mode)
    let current_nonce = if tx.nonce_key != NONCE_KEY_MAX {
        let nonce_key_slot = nonce_slot(sender, tx.nonce_key);
        let current = read_storage(&*state, NONCE_MANAGER_ADDRESS, nonce_key_slot)?.to::<u64>();
        if current != tx.nonce_sequence {
            return Err(Eip8130ValidationError::NonceMismatch {
                expected: current,
                got: tx.nonce_sequence,
            });
        }
        current
    } else {
        if tx.expiry > block_timestamp + NONCE_FREE_MAX_EXPIRY_WINDOW {
            return Err(Eip8130ValidationError::NonceFreeExpiryTooFar {
                expiry: tx.expiry,
                max_allowed: block_timestamp + NONCE_FREE_MAX_EXPIRY_WINDOW,
            });
        }
        // Pre-check the on-chain expiring-nonce seen set for replay
        let sig_hash = sender_signature_hash(&tx);
        let seen_slot = expiring_seen_slot(sig_hash);
        let seen_expiry = read_storage(&*state, NONCE_MANAGER_ADDRESS, seen_slot)?.to::<u64>();
        if seen_expiry != 0 && seen_expiry > block_timestamp {
            return Err(Eip8130ValidationError::NonceFreeReplay);
        }
        0
    };

    // 6. Lock state — reject config changes on locked accounts
    let has_config_changes =
        tx.account_changes.iter().any(|e| matches!(e, AccountChangeEntry::ConfigChange(_)));
    let has_custom_verifier = tx.has_custom_verifier();
    let mut remaining_custom_verifier_gas =
        if has_custom_verifier { custom_verifier_gas_limit } else { 0 };
    if has_config_changes {
        if !op_alloy_consensus::transaction::eip8130::is_account_config_known_deployed() {
            let deployed = state
                .account_code(&ACCOUNT_CONFIG_ADDRESS)
                .map_err(|e| Eip8130ValidationError::StateError(e.to_string()))?
                .is_some_and(|code| !code.is_empty());
            if deployed {
                op_alloy_consensus::transaction::eip8130::mark_account_config_deployed();
            } else {
                return Err(Eip8130ValidationError::AccountConfigNotDeployed);
            }
        }
        let lock_slot_key = lock_slot(sender);
        let lock_value = read_storage(&*state, ACCOUNT_CONFIG_ADDRESS, lock_slot_key)?;
        let unlocks_at = parse_account_state(lock_value).unlocks_at;
        if block_timestamp < unlocks_at {
            return Err(Eip8130ValidationError::AccountLocked);
        }
    }

    // 7. Config change validation: operation count limit, sequence check,
    //    and authorizer chain verification.
    let total_config_ops: usize = tx
        .account_changes
        .iter()
        .filter_map(|e| match e {
            AccountChangeEntry::ConfigChange(cc) => Some(cc.owner_changes.len()),
            _ => None,
        })
        .sum();
    if total_config_ops > MAX_CONFIG_OPS_PER_TX {
        return Err(Eip8130ValidationError::TooManyConfigOperations {
            count: total_config_ops,
            limit: MAX_CONFIG_OPS_PER_TX,
        });
    }

    if has_config_changes {
        let seq_slot = sequence_base_slot(sender);
        let packed = read_storage(&*state, ACCOUNT_CONFIG_ADDRESS, seq_slot)?;
        let mut expected_multichain = read_sequence(packed, true);
        let mut expected_local = read_sequence(packed, false);

        for entry in &tx.account_changes {
            if let AccountChangeEntry::ConfigChange(change) = entry {
                if change.chain_id == 0 {
                    if change.sequence != expected_multichain {
                        return Err(Eip8130ValidationError::SequenceMismatch {
                            expected: expected_multichain,
                            got: change.sequence,
                        });
                    }
                    expected_multichain = expected_multichain.saturating_add(1);
                } else {
                    if change.sequence != expected_local {
                        return Err(Eip8130ValidationError::SequenceMismatch {
                            expected: expected_local,
                            got: change.sequence,
                        });
                    }
                    expected_local = expected_local.saturating_add(1);
                }
            }
        }
    }

    // 7b. Authorizer chain validation for config changes.
    let pending_owner_overrides = if has_config_changes {
        validate_authorizer_chain(
            &tx,
            sender,
            &*state,
            verifier_policy,
            purity_cache,
            &mut remaining_custom_verifier_gas,
        )?
    } else {
        HashMap::new()
    };

    // 8. Compute intrinsic gas for the balance check. `gas_limit` is the
    //    sender's execution-only budget, so we don't compare it against
    //    intrinsic gas. Nonce key is "warm" if the current sequence > 0.
    let nonce_key_is_warm = current_nonce > 0;
    let aa_intrinsic_gas = intrinsic_gas(&tx, nonce_key_is_warm, tx.chain_id);

    // 9. Sender authorization (checks owner_config for the resolved sender)
    let sender_owner_id = validate_sender_authorization(
        &tx,
        sender,
        eoa_owner_id,
        &*state,
        verifier_policy,
        purity_cache,
        &mut remaining_custom_verifier_gas,
        Some(&pending_owner_overrides),
    )?;

    // 10. Payer resolution and authorization
    let payer = tx.payer.unwrap_or(sender);
    let payer_owner_id = if payer != sender {
        Some(validate_payer(
            &tx,
            sender,
            payer,
            &*state,
            verifier_policy,
            purity_cache,
            &mut remaining_custom_verifier_gas,
            None,
        )?)
    } else {
        None
    };

    // 11. Balance check — payer must cover max gas cost.
    //     Total = (intrinsic + custom_verifier_cap + execution_gas_limit) * max_fee_per_gas
    let verifier_gas_cap = if has_custom_verifier { custom_verifier_gas_limit } else { 0u64 };
    let total_gas =
        U256::from(aa_intrinsic_gas) + U256::from(verifier_gas_cap) + U256::from(tx.gas_limit);
    let max_gas_cost = total_gas.saturating_mul(U256::from(tx.max_fee_per_gas));
    let balance = state
        .account_balance(&payer)
        .map_err(|e| Eip8130ValidationError::StateError(e.to_string()))?
        .unwrap_or_default();
    if balance < max_gas_cost {
        return Err(Eip8130ValidationError::InsufficientBalance {
            required: max_gas_cost,
            available: balance,
        });
    }

    // 12. Compute invalidation keys for the state-diff based eviction index
    let invalidation_keys = compute_invalidation_keys(
        &tx,
        sender,
        Some(sender_owner_id).filter(|id| *id != B256::ZERO),
        payer_owner_id.filter(|id| *id != B256::ZERO),
    );

    let sponsored_payer = if payer != sender { Some(payer) } else { None };

    Ok(Eip8130ValidationOutcome {
        balance,
        state_nonce: tx.nonce_sequence,
        nonce_key: tx.nonce_key,
        sender_owner_id,
        invalidation_keys,
        sponsored_payer,
        expiry: tx.expiry,
    })
}
