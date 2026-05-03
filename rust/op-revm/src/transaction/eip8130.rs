//! EIP-8130 AA transaction data carried through the execution pipeline.
//!
//! These types mirror the subset of `TxEip8130` fields the handler needs for
//! phased call execution, auto-delegation, and pre-execution storage writes.
//! They use only primitive types to avoid pulling consensus-layer deps into
//! the EVM crate.

use revm::primitives::{Address, B256, Bytes, Log, LogData, U256, keccak256};
use std::vec::Vec;

/// A single call within an AA transaction phase.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Eip8130Call {
    /// Target address.
    pub to: Address,
    /// Calldata.
    pub data: Bytes,
    /// Value to transfer.
    pub value: U256,
}

/// A pre-execution storage write (nonce increment, owner registration, etc.).
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Eip8130StorageWrite {
    /// Contract address holding the storage.
    pub address: Address,
    /// Storage slot key.
    pub slot: U256,
    /// New value to write.
    pub value: U256,
}

/// Code placement for auto-delegation (EIP-7702 style delegation designator).
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Eip8130CodePlacement {
    /// Address to place code at.
    pub address: Address,
    /// Bytecode to set.
    pub code: Bytes,
}

/// Per-phase execution result.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Eip8130PhaseResult {
    /// Whether the phase succeeded.
    pub success: bool,
    /// Gas consumed by the phase.
    pub gas_used: u64,
}

/// A config-change sequence update applied as a read-modify-write on the
/// packed `ChangeSequences { uint64 multichain; uint64 local }` slot.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Eip8130SequenceUpdate {
    /// Pre-computed storage slot for `_changeSequences[account]`.
    pub slot: U256,
    /// `true` = update the multichain (chain_id 0) field, `false` = local.
    pub is_multichain: bool,
    /// The new sequence value to write (old + 1).
    pub new_value: u64,
}

impl Eip8130SequenceUpdate {
    /// Applies this update to the current packed slot value.
    pub fn apply(&self, current: U256) -> U256 {
        let shift = if self.is_multichain { 0 } else { 64 };
        let field_mask: U256 = U256::from(u64::MAX) << shift;
        if self.is_multichain {
            (current & !field_mask) | U256::from(self.new_value)
        } else {
            (current & !field_mask) | (U256::from(self.new_value) << shift)
        }
    }
}

/// Aggregated AA execution data populated during transaction conversion.
///
/// Built from a `TxEip8130` upstream and consumed by the handler during
/// execution. Non-AA transactions carry a default (empty) instance.
///
/// **Gas-input fields:** [`sender_payload_calldata_cost`][Self::sender_payload_calldata_cost],
/// [`sender_auth`][Self::sender_auth], [`payer_auth`][Self::payer_auth], and
/// [`is_eoa`][Self::is_eoa] are precomputed at conversion time so
/// [`crate::eip8130_gas`] can derive intrinsic gas from `&Eip8130Parts`
/// alone. Gas figures themselves stay fork-aware: the handler reads the
/// active fork's [`revm::context_interface::cfg::GasParams`] from
/// `cfg.gas_params()` and combines it with these inputs on demand. The cap
/// on aggregate custom-verifier gas is bound to
/// [`crate::constants::XLAYER_AA_CUSTOM_VERIFIER_GAS_CAP`] and gated on the
/// presence of an [`AuthState::Deferred`] state at the call site.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Eip8130Parts {
    /// Transaction expiry timestamp (`0` means no expiry).
    pub expiry: u64,
    /// The effective sender address.
    pub sender: Address,
    /// The effective payer address (same as sender if self-pay).
    pub payer: Address,
    /// Resolved sender auth state.
    ///
    /// Built at conversion time from `tx.sender_auth` by
    /// `alloy_op_evm::eip8130::auth_state::build_sender_auth_state`. The
    /// handler dispatches on this with a single `match`.
    pub sender_authstate: AuthState,
    /// Resolved payer auth state.
    ///
    /// `AuthState::Empty` for self-pay txs (no separate payer auth check).
    /// Built at conversion time from `tx.payer_auth` by
    /// `alloy_op_evm::eip8130::auth_state::build_payer_auth_state`.
    pub payer_authstate: AuthState,
    /// Nonce key for 2D nonce slot calculation.
    pub nonce_key: U256,
    /// For nonce-free transactions (`nonce_key == NONCE_KEY_MAX`), the hash
    /// used for on-chain replay protection via the expiring-nonce circular
    /// buffer. `None` for standard (sequenced) transactions; if `None` while
    /// `nonce_key == NONCE_KEY_MAX`, the tx is rejected at validation time.
    pub nonce_free_hash: Option<B256>,
    /// Whether the tx includes a create entry (determines auto-delegation skip).
    pub has_create_entry: bool,
    /// Explicit delegation target from an `AccountChangeEntry::Delegation`.
    ///
    /// `Some(target)` means the tx requests EIP-7702-style code delegation to
    /// `target`. `Some(Address::ZERO)` clears an existing delegation.
    /// `None` means no delegation entry is present.
    pub delegation_target: Option<Address>,
    /// Total account-change units in the transaction.
    pub account_change_units: usize,
    /// EIP-2028 calldata cost (4 per zero, 16 per non-zero byte) of the
    /// sender-signing preimage `tx.encoded_for_sender_signing()`.
    ///
    /// Precomputed at conversion time so
    /// [`crate::eip8130_gas::aa_intrinsic_gas`] is fork-aware-on-rates but
    /// pure data: no need to RLP-re-encode the tx every time intrinsic gas
    /// is queried (validate, deduct, max-cost, gas-limit precompiles).
    /// Fork-independent because EIP-8130's signing encoding is locked and
    /// EIP-2028's per-byte rates are part of the consensus baseline.
    pub sender_payload_calldata_cost: u64,
    /// Whether the tx is in EOA mode (`tx.from.is_none()` upstream).
    ///
    /// Cached so the gas path doesn't need the underlying `TxEip8130`. EOA
    /// mode forces K1 verification cost (the bare 65-byte sig path).
    pub is_eoa: bool,
    /// Raw `tx.sender_auth` blob.
    ///
    /// Cached as `Bytes` (Arc-backed, cheap clone). The gas path inspects
    /// `len()` and the 20-byte verifier prefix to derive the per-side
    /// verification cost; the auth-state-builder side handles the
    /// cryptographic verdict. Empty when sender-auth was omitted (the
    /// `eth_estimateGas` escape — handler rejects outside estimation).
    pub sender_auth: Bytes,
    /// Raw `tx.payer_auth` blob. Empty for self-pay txs and for sponsored
    /// estimateGas. See [`Self::sender_auth`] for the role this plays in
    /// gas computation.
    pub payer_auth: Bytes,
    /// Auto-delegation code (`0xef0100 || DEFAULT_ACCOUNT_ADDRESS`) if applicable.
    pub auto_delegation_code: Bytes,
    /// Pre-execution storage writes for account creation (owner registrations).
    pub pre_writes: Vec<Eip8130StorageWrite>,
    /// Storage writes for config changes (authorize/revoke owners).
    pub config_writes: Vec<Eip8130StorageWrite>,
    /// Sequence updates requiring read-modify-write on packed storage slots.
    pub sequence_updates: Vec<Eip8130SequenceUpdate>,
    /// Code placements for account creation. EIP-8130 allows at most one
    /// create entry per transaction; this `Vec` length must be `<= 1`.
    pub code_placements: Vec<Eip8130CodePlacement>,
    /// Phased call batches. Each inner `Vec` is one atomic phase.
    pub call_phases: Vec<Vec<Eip8130Call>>,
    /// Per-config-change authorizer validation data.
    pub authorizer_validations: Vec<Eip8130AuthorizerValidation>,
    /// System log events for account creation.
    pub account_creation_logs: Vec<Eip8130ConfigLog>,
    /// System log events for config changes.
    pub config_change_logs: Vec<Eip8130ConfigLog>,
}

impl Eip8130Parts {
    /// Whether the tx is self-pay (sender pays its own gas).
    ///
    /// Mirrors `TxEip8130::is_self_pay`: in `eip8130_parts` the payer is
    /// initialized to `tx.payer.unwrap_or(caller)`, so `payer == sender`
    /// iff `tx.payer.is_none()`. Used by the gas path to short-circuit
    /// payer-side costs and by the auth-state-builder to mark
    /// `payer_authstate = AuthState::SelfPay`.
    #[inline]
    pub fn is_self_pay(&self) -> bool {
        self.sender == self.payer
    }
}

/// Authorizer validation data for a single config change entry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Eip8130AuthorizerValidation {
    /// Verifier address from the authorizer's auth prefix.
    pub verifier: Address,
    /// The authenticated owner_id (from native verification at conversion time).
    pub owner_id: B256,
    /// STATICCALL data for custom verifiers. `None` for native verifiers.
    pub verify_call: Option<Eip8130VerifyCall>,
    /// The owner changes in this config change.
    pub owner_changes: Vec<Eip8130ConfigOp>,
}

/// Simplified config operation for the handler's in-memory chaining logic.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Eip8130ConfigOp {
    /// `0x01` = authorize, `0x02` = revoke.
    pub change_type: u8,
    /// Verifier contract address.
    pub verifier: Address,
    /// Owner identifier.
    pub owner_id: B256,
    /// Permission scope bitmask.
    pub scope: u8,
}

/// Inner-verifier metadata for the Delegate→Native auth case.
///
/// Carries the `(verifier, owner_id)` pair that the inner native verifier
/// (K1 / P256Raw / P256WebAuthn) recovered eagerly at conversion time, so the
/// handler can run the inner-binding check
/// `owner_config[delegate_address][owner_id] = (verifier, scope)` on the
/// *delegate account's* owner_config — independent of (and after) the outer
/// `owner_config[account][bytes20(delegate_address)] = (DELEGATE_VERIFIER_ADDRESS, scope)`
/// check.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DelegateInner {
    /// Inner native verifier address (K1, P256Raw, or P256WebAuthn).
    pub verifier: Address,
    /// Inner-recovered owner id: `bytes32(bytes20(addr))` for K1,
    /// `keccak256(pubkey)` for P256 variants.
    pub owner_id: B256,
}

/// Pre-encoded data for a STATICCALL to `IVerifier.verify(hash, data)`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Eip8130VerifyCall {
    /// The verifier contract address.
    pub verifier: Address,
    /// ABI-encoded `IVerifier.verify(hash, data)` calldata.
    pub calldata: Bytes,
    /// The account whose owner_config to check the returned owner_id against.
    pub account: Address,
    /// Required scope bit for the owner.
    pub required_scope: u8,
}

/// Resolved sender / payer auth state at conversion time.
///
/// This collapses the EIP-8130 wire format (EOA mode bare 65-byte sig, or
/// explicit-from `[verifier_addr || data]`) and the result of any eager native
/// verification into one of four discrete states for the handler to dispatch
/// on.
///
/// Replaces the previous flat-sentinel layout (`{verifier, owner_id,
/// auth_empty, auth_invalid, verify_call}`), which scattered five logical
/// states across five fields and required the handler to re-derive intent
/// from sentinel values. Routing on this enum is a single `match`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AuthState {
    /// Self-pay: `payer == sender`, no separate payer auth needed.
    ///
    /// On the *payer* side this is the canonical "no separate check"
    /// outcome. Also used as the [`AuthState::default()`] for both sides:
    /// non-AA transactions never read these fields, and AA-tx-specific
    /// tests that don't care about auth dispatch can leave the field at
    /// default to skip the validate-and-deduct flow's auth match.
    /// Real AA conversion always overwrites both sides via
    /// `build_sender_auth_state` / `build_payer_auth_state`.
    #[default]
    SelfPay,

    /// Auth field was empty — `eth_estimateGas` escape hatch.
    ///
    /// Only valid when fee checks are disabled. The handler rejects this state
    /// outside estimation. Sender side: explicit-`from` mode with empty
    /// `sender_auth`. Payer side: sponsored mode with empty `payer_auth`.
    Empty,

    /// Auth was malformed or eager verification failed.
    ///
    /// Produced for parse errors, signature recovery failures, mismatched
    /// recovered key vs. claimed `from`/`payer`, or unsupported verifier
    /// addresses with bad data layout. The handler unconditionally rejects.
    ///
    /// This is the validator-side defense against a malicious sequencer that
    /// includes a tx with a forged auth: the cryptographic check happens
    /// once at decode time and the handler refuses to execute on failure.
    ///
    /// Carries a reason describing which specific check rejected the auth,
    /// surfaced by the handler in its error message so failures can be
    /// diagnosed without re-running the verifier. `String` rather than
    /// `&'static str` so the variant survives a serde round-trip
    /// (`op-revm/serde` is enabled transitively via `kona-genesis`); the
    /// extra allocation lands on the cold rejection path where it has no
    /// measurable cost.
    Invalid(std::string::String),

    /// Native verifier succeeded eagerly at conversion time.
    ///
    /// Covers K1 / P256Raw / P256WebAuthn / Delegate-with-native-inner.
    /// The handler does not re-run cryptography; it re-validates the
    /// `(account, owner_id, verifier, scope)` binding against on-chain
    /// `owner_config` (and pending in-tx config changes).
    ///
    /// **Delegate→Native** sets `delegate_inner = Some(...)`. The outer
    /// `verifier` is then `DELEGATE_VERIFIER_ADDRESS` and `owner_id =
    /// bytes32(bytes20(delegate_address))`. The handler runs **two**
    /// owner_config checks for this case:
    ///
    /// 1. Outer: `owner_config[account][owner_id] = (DELEGATE_VERIFIER_ADDRESS, scope)`.
    /// 2. Inner: `owner_config[delegate_address][delegate_inner.owner_id] =
    ///    (delegate_inner.verifier, scope)`.
    ///
    /// Without (2) a tx whose nested K1/P256 sig is mathematically valid but
    /// whose recovered inner owner is **not registered on the delegate
    /// account** would slip through — that's the bug base's mempool
    /// `verify_delegate_auth_with_scope` catches and base's validator-side
    /// `verify_delegate` doesn't. We do (2) at validator dispatch so the
    /// mempool and validator share one code path.
    Native {
        /// Verifier address (K1 sentinel, P256Raw addr, WebAuthn addr, or
        /// `DELEGATE_VERIFIER_ADDRESS`).
        verifier: Address,
        /// Authenticated owner id. For K1 / Delegate this is
        /// `bytes32(bytes20(addr))`; for P256 variants it's `keccak256(pubkey)`.
        owner_id: B256,
        /// `Some` only for Delegate→Native; carries the inner verifier
        /// address and inner-recovered owner_id so the handler can run the
        /// inner-binding check on the *delegate account's* owner_config.
        /// `None` for K1 / P256Raw / P256WebAuthn.
        delegate_inner: Option<DelegateInner>,
    },

    /// Custom verifier — STATICCALL deferred to handler.
    ///
    /// The handler runs the STATICCALL, captures the returned `owner_id` from
    /// the verifier contract's return value, and validates the
    /// `(spec.account, returned_owner_id, spec.verifier, spec.required_scope)`
    /// binding against `owner_config`.
    ///
    /// `delegate_outer` is `Some(delegate_address)` only when the outer
    /// verifier is `DELEGATE_VERIFIER_ADDRESS` and the inner (nested) verifier
    /// is custom. In that case the handler additionally validates
    /// `owner_config[account][bytes32(delegate_address)] = (DELEGATE_VERIFIER_ADDRESS, scope)`
    /// after the inner STATICCALL succeeds.
    Deferred {
        /// Pre-encoded STATICCALL spec for the custom verifier.
        spec: Eip8130VerifyCall,
        /// `Some(delegate_address)` for Delegate→Custom. `None` otherwise.
        delegate_outer: Option<Address>,
    },
}

impl AuthState {
    /// `true` iff this state is [`AuthState::SelfPay`].
    pub fn is_self_pay(&self) -> bool {
        matches!(self, Self::SelfPay)
    }

    /// `true` iff this state is [`AuthState::Empty`].
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    /// `true` iff this state is [`AuthState::Invalid`].
    pub fn is_invalid(&self) -> bool {
        matches!(self, Self::Invalid(_))
    }

    /// Returns the deferred STATICCALL spec when this state is
    /// [`AuthState::Deferred`].
    pub fn verify_call(&self) -> Option<&Eip8130VerifyCall> {
        match self {
            Self::Deferred { spec, .. } => Some(spec),
            _ => None,
        }
    }

    /// Returns the `(verifier, owner_id)` pair for [`AuthState::Native`].
    pub fn native_pair(&self) -> Option<(Address, B256)> {
        match self {
            Self::Native { verifier, owner_id, .. } => Some((*verifier, *owner_id)),
            _ => None,
        }
    }

    /// Returns the `delegate_outer` address when present.
    pub fn delegate_outer(&self) -> Option<Address> {
        match self {
            Self::Deferred { delegate_outer, .. } => *delegate_outer,
            _ => None,
        }
    }
}

/// Encodes phase results into the output bytes of an AA transaction.
///
/// Format: one byte per phase, `0x01` = success, `0x00` = failure.
pub fn encode_phase_statuses(results: &[Eip8130PhaseResult]) -> Bytes {
    Bytes::from(results.iter().map(|r| u8::from(r.success)).collect::<Vec<_>>())
}

/// Decodes phase statuses from AA transaction output bytes.
pub fn decode_phase_statuses(output: &[u8]) -> Vec<bool> {
    output.iter().map(|&b| b != 0).collect()
}

/// System log topic for persisting per-phase execution statuses in receipts.
///
/// `keccak256("Eip8130PhaseStatuses(bytes)")`
pub fn phase_statuses_log_topic() -> B256 {
    keccak256(b"Eip8130PhaseStatuses(bytes)")
}

// ── AccountConfiguration contract event topics ───────────────────

/// `keccak256("OwnerAuthorized(address,bytes32,address,uint8)")`
pub(crate) fn owner_authorized_log_topic() -> B256 {
    keccak256(b"OwnerAuthorized(address,bytes32,address,uint8)")
}

/// `keccak256("OwnerRevoked(address,bytes32)")`
pub(crate) fn owner_revoked_log_topic() -> B256 {
    keccak256(b"OwnerRevoked(address,bytes32)")
}

/// `keccak256("AccountCreated(address,bytes32,bytes32)")`
pub(crate) fn account_created_log_topic() -> B256 {
    keccak256(b"AccountCreated(address,bytes32,bytes32)")
}

/// Pads an [`Address`] into a 32-byte indexed topic (left-padded with zeros).
fn address_to_topic(addr: Address) -> B256 {
    let mut topic = B256::ZERO;
    topic.0[12..32].copy_from_slice(addr.as_slice());
    topic
}

/// An `AccountConfiguration` contract event to be injected as a system log.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Eip8130ConfigLog {
    /// `OwnerAuthorized(address indexed account, bytes32 indexed ownerId, address verifier, uint8
    /// scope)`
    OwnerAuthorized {
        /// The account whose owner table is being modified.
        account: Address,
        /// Verifier-derived identifier for the owner.
        owner_id: B256,
        /// Verifier contract that authenticates this owner.
        verifier: Address,
        /// Permission bitmask (0x00 = unrestricted).
        scope: u8,
    },
    /// `OwnerRevoked(address indexed account, bytes32 indexed ownerId)`
    OwnerRevoked {
        /// The account whose owner table is being modified.
        account: Address,
        /// Verifier-derived identifier for the revoked owner.
        owner_id: B256,
    },
    /// `AccountCreated(address indexed account, bytes32 userSalt, bytes32 codeHash)`
    AccountCreated {
        /// The newly deployed account address.
        account: Address,
        /// User-chosen salt for CREATE2 derivation.
        user_salt: B256,
        /// `keccak256(bytecode)` of the deployed runtime code.
        code_hash: B256,
    },
}

/// Converts an [`Eip8130ConfigLog`] into a revm [`Log`] emitted from
/// `emitter` (the AccountConfiguration contract address).
pub fn config_log_to_system_log(emitter: Address, event: &Eip8130ConfigLog) -> Log {
    match event {
        Eip8130ConfigLog::OwnerAuthorized { account, owner_id, verifier, scope } => {
            let mut data = Vec::with_capacity(64);
            let mut verifier_word = [0u8; 32];
            verifier_word[12..32].copy_from_slice(verifier.as_slice());
            data.extend_from_slice(&verifier_word);
            let mut scope_word = [0u8; 32];
            scope_word[31] = *scope;
            data.extend_from_slice(&scope_word);
            Log {
                address: emitter,
                data: LogData::new_unchecked(
                    std::vec![owner_authorized_log_topic(), address_to_topic(*account), *owner_id],
                    Bytes::from(data),
                ),
            }
        }
        Eip8130ConfigLog::OwnerRevoked { account, owner_id } => Log {
            address: emitter,
            data: LogData::new_unchecked(
                std::vec![owner_revoked_log_topic(), address_to_topic(*account), *owner_id],
                Bytes::new(),
            ),
        },
        Eip8130ConfigLog::AccountCreated { account, user_salt, code_hash } => {
            let mut data = Vec::with_capacity(64);
            data.extend_from_slice(user_salt.as_slice());
            data.extend_from_slice(code_hash.as_slice());
            Log {
                address: emitter,
                data: LogData::new_unchecked(
                    std::vec![account_created_log_topic(), address_to_topic(*account)],
                    Bytes::from(data),
                ),
            }
        }
    }
}

/// Creates a system log carrying per-phase execution statuses.
pub fn phase_statuses_system_log(emitter: Address, results: &[Eip8130PhaseResult]) -> Log {
    let data = Bytes::from(results.iter().map(|r| u8::from(r.success)).collect::<Vec<_>>());
    Log {
        address: emitter,
        data: LogData::new_unchecked(std::vec![phase_statuses_log_topic()], data),
    }
}

/// Extracts per-phase statuses from a system log emitted during EIP-8130 execution.
pub fn extract_phase_statuses_from_logs<T: AsRef<Log>>(
    logs: &[T],
    emitter: Address,
) -> Option<Vec<bool>> {
    let topic = phase_statuses_log_topic();
    for log in logs {
        let log = log.as_ref();
        if log.address == emitter && log.topics().first() == Some(&topic) {
            return Some(decode_phase_statuses(&log.data.data));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;

    #[test]
    fn encode_all_success() {
        let results = vec![
            Eip8130PhaseResult { success: true, gas_used: 100 },
            Eip8130PhaseResult { success: true, gas_used: 200 },
        ];
        let encoded = encode_phase_statuses(&results);
        assert_eq!(&encoded[..], &[0x01, 0x01]);
    }

    #[test]
    fn encode_all_failure() {
        let results = vec![
            Eip8130PhaseResult { success: false, gas_used: 50 },
            Eip8130PhaseResult { success: false, gas_used: 75 },
        ];
        let encoded = encode_phase_statuses(&results);
        assert_eq!(&encoded[..], &[0x00, 0x00]);
    }

    #[test]
    fn encode_mixed() {
        let results = vec![
            Eip8130PhaseResult { success: true, gas_used: 10 },
            Eip8130PhaseResult { success: false, gas_used: 20 },
            Eip8130PhaseResult { success: true, gas_used: 30 },
        ];
        let encoded = encode_phase_statuses(&results);
        assert_eq!(&encoded[..], &[0x01, 0x00, 0x01]);
    }

    #[test]
    fn encode_empty() {
        let encoded = encode_phase_statuses(&[]);
        assert!(encoded.is_empty());
    }

    #[test]
    fn decode_roundtrip() {
        let results = vec![
            Eip8130PhaseResult { success: true, gas_used: 0 },
            Eip8130PhaseResult { success: false, gas_used: 0 },
            Eip8130PhaseResult { success: true, gas_used: 0 },
            Eip8130PhaseResult { success: false, gas_used: 0 },
        ];
        let encoded = encode_phase_statuses(&results);
        let decoded = decode_phase_statuses(&encoded);
        assert_eq!(decoded, vec![true, false, true, false]);
    }

    #[test]
    fn decode_empty() {
        let decoded = decode_phase_statuses(&[]);
        assert!(decoded.is_empty());
    }

    #[test]
    fn parts_default_is_empty() {
        let parts = Eip8130Parts::default();
        assert_eq!(parts.sender, Address::ZERO);
        assert_eq!(parts.payer, Address::ZERO);
        assert_eq!(parts.sender_authstate, AuthState::SelfPay);
        assert_eq!(parts.payer_authstate, AuthState::SelfPay);
        assert_eq!(parts.nonce_key, U256::ZERO);
        assert!(!parts.has_create_entry);
        assert_eq!(parts.account_change_units, 0);
        assert_eq!(parts.sender_payload_calldata_cost, 0);
        assert!(!parts.is_eoa);
        assert!(parts.sender_auth.is_empty());
        assert!(parts.payer_auth.is_empty());
        assert!(parts.is_self_pay());
        assert!(parts.auto_delegation_code.is_empty());
        assert!(parts.pre_writes.is_empty());
        assert!(parts.call_phases.is_empty());
        assert!(parts.account_creation_logs.is_empty());
        assert!(parts.config_change_logs.is_empty());
    }

    #[test]
    fn owner_authorized_log_encoding() {
        let emitter = Address::repeat_byte(0xAC);
        let account = Address::repeat_byte(0x01);
        let owner_id = B256::repeat_byte(0x02);
        let verifier = Address::repeat_byte(0x03);
        let scope = 0x0A;

        let event = Eip8130ConfigLog::OwnerAuthorized { account, owner_id, verifier, scope };
        let log = config_log_to_system_log(emitter, &event);

        assert_eq!(log.address, emitter);
        assert_eq!(log.topics().len(), 3);
        assert_eq!(log.topics()[0], owner_authorized_log_topic());
        assert_eq!(log.topics()[1], address_to_topic(account));
        assert_eq!(log.topics()[2], owner_id);

        assert_eq!(log.data.data.len(), 64);
        assert_eq!(&log.data.data[12..32], verifier.as_slice());
        assert_eq!(log.data.data[63], scope);
    }

    #[test]
    fn owner_revoked_log_encoding() {
        let emitter = Address::repeat_byte(0xAC);
        let account = Address::repeat_byte(0x01);
        let owner_id = B256::repeat_byte(0x02);

        let event = Eip8130ConfigLog::OwnerRevoked { account, owner_id };
        let log = config_log_to_system_log(emitter, &event);

        assert_eq!(log.address, emitter);
        assert_eq!(log.topics().len(), 3);
        assert_eq!(log.topics()[0], owner_revoked_log_topic());
        assert_eq!(log.topics()[1], address_to_topic(account));
        assert_eq!(log.topics()[2], owner_id);
        assert!(log.data.data.is_empty());
    }

    #[test]
    fn account_created_log_encoding() {
        let emitter = Address::repeat_byte(0xAC);
        let account = Address::repeat_byte(0x01);
        let user_salt = B256::repeat_byte(0xAA);
        let code_hash = B256::repeat_byte(0xBB);

        let event = Eip8130ConfigLog::AccountCreated { account, user_salt, code_hash };
        let log = config_log_to_system_log(emitter, &event);

        assert_eq!(log.address, emitter);
        assert_eq!(log.topics().len(), 2);
        assert_eq!(log.topics()[0], account_created_log_topic());
        assert_eq!(log.topics()[1], address_to_topic(account));

        assert_eq!(log.data.data.len(), 64);
        assert_eq!(&log.data.data[..32], user_salt.as_slice());
        assert_eq!(&log.data.data[32..64], code_hash.as_slice());
    }

    #[test]
    fn sequence_update_preserves_lock_bits() {
        let mut bytes = [0u8; 32];
        bytes[24..32].copy_from_slice(&1u64.to_be_bytes());
        bytes[16..24].copy_from_slice(&2u64.to_be_bytes());
        bytes[11..16].copy_from_slice(&[0x01, 0x02, 0x03, 0x04, 0x05]);
        bytes[9..11].copy_from_slice(&0x0607u16.to_be_bytes());
        let current = U256::from_be_bytes(bytes);

        let updated =
            Eip8130SequenceUpdate { slot: U256::ZERO, is_multichain: true, new_value: 99 }
                .apply(current);
        let updated_bytes = updated.to_be_bytes::<32>();

        assert_eq!(u64::from_be_bytes(updated_bytes[24..32].try_into().unwrap()), 99);
        assert_eq!(u64::from_be_bytes(updated_bytes[16..24].try_into().unwrap()), 2);
        assert_eq!(&updated_bytes[11..16], &[0x01, 0x02, 0x03, 0x04, 0x05]);
        assert_eq!(&updated_bytes[9..11], &0x0607u16.to_be_bytes());
    }
}
