//! EIP-8130 helper functions for the OpHandler.
//!
//! Ported 1:1 from base's crates/execution/revm/src/handler.rs (lines 42-619)
//! to keep semantics byte-compatible with base. Constants are re-declared here
//! (rather than imported from op-alloy-consensus) to mirror base's intra-crate
//! dep-cycle avoidance pattern.
use std::{boxed::Box, collections::HashMap};

use revm::{
    context::{
        journaled_state::account::JournaledAccountTr, result::InvalidTransaction, LocalContextTr,
    },
    context_interface::{Block, Cfg, ContextTr, JournalTr},
    handler::{
        EvmTr, FrameResult, Handler, MainnetHandler, evm::FrameTr, handler::EvmTrError,
    },
    interpreter::{
        SharedMemory,
        interpreter_action::{CallInput, CallInputs, CallScheme, CallValue, FrameInit, FrameInput},
    },
    primitives::{Address, B256, Bytes, U256, keccak256},
};

use crate::{
    api::exec::OpContextTr,
    eip8130_policy::{
        PendingOwnerState, PendingOwnerValidationError, pending_owner_state_for_change,
        validate_pending_owner_state,
    },
    transaction::{OpTransactionError, eip8130::Eip8130Parts},
};

/// EIP-8130 AA transaction type byte.
///
/// Mirrors base/handler.rs:43. `precompiles.rs` keeps a private copy of the
/// same byte (mirroring base/precompiles.rs:208) to avoid cross-imports
/// between handler and precompile modules.
pub(crate) const EIP8130_TX_TYPE: u8 = 0x7B;

/// Estimated calldata gas for a K1 auth blob missing during gas estimation.
///
/// K1 auth = 66 bytes (type + 64-byte signature + recovery byte).
/// In RLP, that adds ~67 bytes vs the 1-byte encoding of an empty bytes field.
/// 67 bytes × 16 gas/byte (non-zero) ≈ 1,072, rounded up for safety.
const ESTIMATION_AUTH_CALLDATA_GAS: u64 = 1_100;

/// Gas delta between cold and warm nonce key SSTORE costs.
///
/// `aa_intrinsic_gas` always uses the cold worst-case (22,100). When the nonce
/// channel has been used before the SSTORE cost is only 5,000, so the handler
/// gives back this delta to the call phases at execution time.
pub(crate) const NONCE_COLD_WARM_DELTA: u64 = 17_100;

/// AccountConfiguration deployed contract address.
/// Must match the CREATE2 address from `Deploy.s.sol` (salt = 0).
pub(crate) const ACCOUNT_CONFIG_ADDRESS: Address = Address::new([
    0x4F, 0x20, 0x61, 0x8C, 0xf5, 0xc1, 0x60, 0xe7, 0xAA, 0x38, 0x52, 0x68, 0x72, 0x1d, 0xA9, 0x68,
    0xF8, 0x6F, 0x0e, 0x61,
]);

/// Explicit native K1/ecrecover verifier sentinel (`address(1)`).
///
/// Mirrors `base_alloy_consensus::K1_VERIFIER_ADDRESS` to avoid a cyclic
/// dependency.
const K1_VERIFIER_ADDRESS: Address = Address::new([
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x01,
]);

/// Sentinel verifier written when the implicit EOA owner is explicitly revoked.
///
/// Mirrors `base_alloy_consensus::REVOKED_VERIFIER`.
const REVOKED_VERIFIER: Address = Address::new([0xff; 20]);

/// Monotonic cache: once the AccountConfiguration contract is detected, we
/// skip the code-existence check on all subsequent calls. Relaxed ordering
/// is safe because the flag only transitions `false → true`.
static ACCOUNT_CONFIG_DEPLOYED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Base storage slot for the nonce mapping in NonceManager (slot index 1).
///
/// Mirrors base/handler.rs:87. `precompiles.rs` keeps a public copy of the
/// same value (`NONCE_BASE_SLOT`) so external callers like alloy-op-evm can
/// reuse it without importing handler-internal symbols.
const NONCE_BASE_SLOT: U256 = U256::from_limbs([1, 0, 0, 0]);
/// Base storage slot for the packed `_accountState` mapping in AccountConfig (slot index 1).
const LOCK_BASE_SLOT: U256 = U256::from_limbs([1, 0, 0, 0]);

/// Sentinel nonce key that activates nonce-free mode.
///
/// Mirrors [`op_alloy_consensus::NONCE_KEY_MAX`] to avoid a cyclic dependency.
pub(crate) const NONCE_KEY_MAX: U256 = U256::MAX;

/// Mirrors [`op_alloy_consensus::EXPIRING_SEEN_BASE_SLOT`].
const EXPIRING_SEEN_BASE_SLOT: U256 = U256::from_limbs([2, 0, 0, 0]);
/// Mirrors [`op_alloy_consensus::EXPIRING_RING_BASE_SLOT`].
const EXPIRING_RING_BASE_SLOT: U256 = U256::from_limbs([3, 0, 0, 0]);
/// Mirrors [`op_alloy_consensus::EXPIRING_RING_PTR_SLOT`].
pub(crate) const EXPIRING_RING_PTR_SLOT: U256 = U256::from_limbs([4, 0, 0, 0]);
/// Mirrors [`op_alloy_consensus::EXPIRING_NONCE_SET_CAPACITY`].
pub(crate) const EXPIRING_NONCE_SET_CAPACITY: u32 = 300_000;
/// Mirrors [`op_alloy_consensus::NONCE_FREE_MAX_EXPIRY_WINDOW`].
pub(crate) const NONCE_FREE_MAX_EXPIRY_WINDOW: u64 = 30;

/// Computes the NonceManager storage slot for `nonce[account][nonce_key]`.
///
/// `keccak256(nonce_key . keccak256(account . NONCE_BASE_SLOT))`
///
/// Mirrors [`base_alloy_consensus::nonce_slot`] / base/handler.rs:112.
/// `precompiles.rs` keeps a public copy of the same function for external
/// callers so handler and precompile modules need not cross-import.
pub(crate) fn aa_nonce_slot(account: Address, nonce_key: U256) -> U256 {
    let inner = {
        let mut buf = [0u8; 64];
        buf[12..32].copy_from_slice(account.as_slice());
        let base_bytes = NONCE_BASE_SLOT.to_be_bytes::<32>();
        buf[32..64].copy_from_slice(&base_bytes);
        keccak256(buf)
    };
    let mut buf = [0u8; 64];
    buf[..32].copy_from_slice(&nonce_key.to_be_bytes::<32>());
    buf[32..64].copy_from_slice(inner.as_slice());
    U256::from_be_bytes(keccak256(buf).0)
}

/// Computes the storage slot for `expiringNonceSeen[txHash]`.
///
/// Mirrors [`op_alloy_consensus::expiring_seen_slot`] to avoid a cyclic dependency.
pub(crate) fn aa_expiring_seen_slot(tx_hash: B256) -> U256 {
    let mut buf = [0u8; 64];
    buf[..32].copy_from_slice(tx_hash.as_slice());
    let base = EXPIRING_SEEN_BASE_SLOT.to_be_bytes::<32>();
    buf[32..64].copy_from_slice(&base);
    U256::from_be_bytes(keccak256(buf).0)
}

/// Computes the storage slot for `expiringNonceRing[index]`.
///
/// Mirrors [`op_alloy_consensus::expiring_ring_slot`] to avoid a cyclic dependency.
pub(crate) fn aa_expiring_ring_slot(index: u32) -> U256 {
    let mut buf = [0u8; 64];
    buf[28..32].copy_from_slice(&index.to_be_bytes());
    let base = EXPIRING_RING_BASE_SLOT.to_be_bytes::<32>();
    buf[32..64].copy_from_slice(&base);
    U256::from_be_bytes(keccak256(buf).0)
}

/// Computes the AccountConfig storage slot for `lock_state(account)`.
///
/// Mirrors [`base_alloy_consensus::lock_slot`] to avoid a cyclic dependency.
fn aa_lock_slot(account: Address) -> U256 {
    let mut buf = [0u8; 64];
    buf[12..32].copy_from_slice(account.as_slice());
    let base_bytes = LOCK_BASE_SLOT.to_be_bytes::<32>();
    buf[32..64].copy_from_slice(&base_bytes);
    U256::from_be_bytes(keccak256(buf).0)
}

/// Owner config base storage slot in AccountConfig (slot index 0).
///
/// Solidity declaration:
/// `mapping(bytes32 ownerId => mapping(address account => OwnerConfig)) _ownerConfig`
///
/// `keccak256(account . keccak256(owner_id . 0))`
const OWNER_CONFIG_BASE_SLOT: U256 = U256::ZERO;

/// Computes the AccountConfig storage slot for `owner_config(account, owner_id)`.
///
/// Mirrors [`base_alloy_consensus::owner_config_slot`] to avoid a cyclic dependency.
fn aa_owner_config_slot(account: Address, owner_id: U256) -> U256 {
    let inner = {
        let mut buf = [0u8; 64];
        buf[..32].copy_from_slice(&owner_id.to_be_bytes::<32>());
        let base_bytes = OWNER_CONFIG_BASE_SLOT.to_be_bytes::<32>();
        buf[32..64].copy_from_slice(&base_bytes);
        keccak256(buf)
    };
    let mut buf = [0u8; 64];
    buf[12..32].copy_from_slice(account.as_slice());
    buf[32..64].copy_from_slice(inner.as_slice());
    U256::from_be_bytes(keccak256(buf).0)
}

/// Parses a packed owner_config word into `(verifier_address, scope)`.
///
/// Layout: `[zeros(11) | scope(1) | verifier(20)]` (big-endian 32 bytes).
fn parse_owner_config_word(word: U256) -> (Address, u8) {
    let bytes = word.to_be_bytes::<32>();
    let scope = bytes[11];
    let verifier = Address::from_slice(&bytes[12..32]);
    (verifier, scope)
}

/// Validates owner authorization against effective state:
/// pending in-tx owner changes first, then on-chain storage.
fn validate_owner_against_effective_config<EVM, ERROR>(
    evm: &mut EVM,
    account: Address,
    owner_id: U256,
    expected_verifier: Address,
    required_scope: u8,
    allow_implicit_eoa: bool,
    pending_owner_overrides: Option<&HashMap<U256, PendingOwnerState>>,
) -> Result<(), ERROR>
where
    EVM: EvmTr<Context: OpContextTr>,
    ERROR: EvmTrError<EVM> + From<OpTransactionError>,
{
    if let Some(state) = pending_owner_overrides.and_then(|m| m.get(&owner_id)) {
        validate_pending_owner_state(state, expected_verifier, required_scope).map_err(|err| {
            let msg: std::borrow::Cow<'static, str> = match err {
                PendingOwnerValidationError::Revoked => {
                    "owner explicitly revoked in pending config changes".into()
                }
                PendingOwnerValidationError::VerifierMismatch { expected, actual } => {
                    format!("verifier mismatch: expected {expected}, got {actual}").into()
                }
                PendingOwnerValidationError::MissingScope { required_scope } => {
                    format!("owner lacks required scope bit 0x{required_scope:02x}").into()
                }
            };
            eip8130_invalid_tx::<ERROR>(msg)
        })?;
        return Ok(());
    }

    evm.ctx().journal_mut().load_account(ACCOUNT_CONFIG_ADDRESS)?;
    let slot = aa_owner_config_slot(account, owner_id);
    let config_word = evm.ctx().journal_mut().sload(ACCOUNT_CONFIG_ADDRESS, slot)?.data;
    let (on_chain_verifier, scope) = parse_owner_config_word(config_word);

    if on_chain_verifier == REVOKED_VERIFIER {
        return Err(eip8130_invalid_tx::<ERROR>(
            "native verifier owner explicitly revoked (REVOKED_VERIFIER sentinel)",
        ));
    }

    if on_chain_verifier == Address::ZERO {
        if allow_implicit_eoa {
            let implicit_owner_id = {
                let mut buf = [0u8; 32];
                buf[..20].copy_from_slice(account.as_slice());
                U256::from_be_bytes(buf)
            };
            if owner_id == implicit_owner_id && expected_verifier == K1_VERIFIER_ADDRESS {
                return Ok(());
            }
        }
        return Err(eip8130_invalid_tx::<ERROR>(
            "owner_config not found and implicit EOA rule does not apply",
        ));
    }

    if on_chain_verifier != expected_verifier {
        return Err(eip8130_invalid_tx::<ERROR>(format!(
            "verifier mismatch: expected {expected_verifier}, got {on_chain_verifier}",
        )));
    }
    if scope != 0 && (scope & required_scope) == 0 {
        return Err(eip8130_invalid_tx::<ERROR>(format!(
            "owner lacks required scope bit 0x{required_scope:02x}",
        )));
    }
    Ok(())
}

/// Reads one sequence value from packed `AccountState { multichain, local, unlocksAt, unlockDelay }`.
fn read_packed_sequence(slot_value: U256, is_multichain: bool) -> u64 {
    if is_multichain { slot_value.as_limbs()[0] } else { (slot_value >> 64_u8).as_limbs()[0] }
}

/// Extra gas to reserve during `eth_estimateGas` for auth blob calldata that
/// will be present in the real transaction but is absent in the estimation
/// request (which uses empty `senderAuth` / `payerAuth`).
pub(crate) fn estimation_calldata_overhead(parts: &Eip8130Parts) -> u64 {
    let mut overhead = 0;
    if parts.sender_auth_empty {
        overhead += ESTIMATION_AUTH_CALLDATA_GAS;
    }
    if parts.payer_auth_empty {
        overhead += ESTIMATION_AUTH_CALLDATA_GAS;
    }
    overhead
}

/// Creates an `InvalidTransaction` error from a message string.
///
/// Produces `EVMError::Transaction(OpTransactionError::Base(InvalidTransaction::Str(...)))`,
/// which the block builder catches and skips (rather than aborting the flashblock).
fn eip8130_invalid_tx<ERROR: From<OpTransactionError>>(
    msg: impl Into<std::borrow::Cow<'static, str>>,
) -> ERROR {
    OpTransactionError::Base(InvalidTransaction::Str(msg.into())).into()
}

/// Validates that `owner_id` is registered in AccountConfig with the expected
/// verifier address and required scope. Returns `Err` on mismatch.
pub(crate) fn validate_owner_config<EVM, ERROR>(
    evm: &mut EVM,
    account: Address,
    owner_id: U256,
    expected_verifier: Address,
    required_scope: u8,
    pending_owner_overrides: Option<&HashMap<U256, PendingOwnerState>>,
) -> Result<(), ERROR>
where
    EVM: EvmTr<Context: OpContextTr>,
    ERROR: EvmTrError<EVM> + From<OpTransactionError>,
{
    validate_owner_against_effective_config::<EVM, ERROR>(
        evm,
        account,
        owner_id,
        expected_verifier,
        required_scope,
        false,
        pending_owner_overrides,
    )
}

/// Re-validates a native verifier's owner_config at inclusion time.
///
/// For `DELEGATE` verifiers this requires two SLOADs: one to resolve the
/// delegation target and another to check the inner verifier's config.
pub(crate) fn validate_native_verifier_owner<EVM, ERROR>(
    evm: &mut EVM,
    account: Address,
    verifier: Address,
    owner_id: B256,
    required_scope: u8,
    pending_owner_overrides: Option<&HashMap<U256, PendingOwnerState>>,
) -> Result<(), ERROR>
where
    EVM: EvmTr<Context: OpContextTr>,
    ERROR: EvmTrError<EVM> + From<OpTransactionError>,
{
    let owner_id_uint = U256::from_be_bytes(owner_id.0);
    let has_pending_override =
        pending_owner_overrides.and_then(|m| m.get(&owner_id_uint)).is_some();

    validate_owner_against_effective_config::<EVM, ERROR>(
        evm,
        account,
        owner_id_uint,
        verifier,
        required_scope,
        true,
        pending_owner_overrides,
    )?;

    if verifier == crate::constants::DELEGATE_VERIFIER_ADDRESS && !has_pending_override {
        // DELEGATE: the on-chain verifier is the delegation target. Read
        // the inner owner's config for the SAME owner_id under the
        // delegation target's verifier address.
        let inner_slot = aa_owner_config_slot(account, owner_id_uint);
        let inner_word = evm.ctx().journal_mut().sload(ACCOUNT_CONFIG_ADDRESS, inner_slot)?.data;
        let (inner_verifier, inner_scope) = parse_owner_config_word(inner_word);

        if inner_verifier == Address::ZERO {
            return Err(eip8130_invalid_tx::<ERROR>("delegate inner verifier owner revoked"));
        }
        if inner_scope != 0 && (inner_scope & required_scope) == 0 {
            return Err(eip8130_invalid_tx::<ERROR>(format!(
                "delegate inner owner lacks required scope 0x{required_scope:02x}"
            )));
        }
    }

    Ok(())
}

/// Re-validates config-change preconditions at inclusion time.
///
/// This ensures config updates are still valid even when state changed after
/// mempool admission:
/// - account is not locked
/// - each config-change sequence matches expected on-chain value, with
///   in-tx chaining across multiple entries.
pub(crate) fn validate_config_change_preconditions<EVM, ERROR>(
    evm: &mut EVM,
    sender: Address,
    eip8130: &Eip8130Parts,
) -> Result<(), ERROR>
where
    EVM: EvmTr<Context: OpContextTr>,
    ERROR: EvmTrError<EVM> + From<OpTransactionError>,
{
    if eip8130.sequence_updates.is_empty() && eip8130.config_writes.is_empty() {
        return Ok(());
    }

    if !ACCOUNT_CONFIG_DEPLOYED.load(std::sync::atomic::Ordering::Relaxed) {
        let acct = evm.ctx().journal_mut().load_account_with_code_mut(ACCOUNT_CONFIG_ADDRESS)?.data;
        let has_code = acct.account().info.code_hash != keccak256([]);
        drop(acct);
        if has_code {
            ACCOUNT_CONFIG_DEPLOYED.store(true, std::sync::atomic::Ordering::Relaxed);
        } else {
            return Err(eip8130_invalid_tx::<ERROR>(
                "config changes require AccountConfiguration to be deployed",
            ));
        }
    }

    evm.ctx().journal_mut().load_account(ACCOUNT_CONFIG_ADDRESS)?;

    // Lock-state check: locked accounts cannot process config changes.
    // AccountState packs `unlocksAt` (uint40) at bytes [11..16] of the slot.
    // An account is locked when `block.timestamp < unlocksAt`.
    let lock_slot = aa_lock_slot(sender);
    let lock_word = evm.ctx().journal_mut().sload(ACCOUNT_CONFIG_ADDRESS, lock_slot)?.data;
    let lock_bytes = lock_word.to_be_bytes::<32>();
    let mut ua = [0u8; 8];
    ua[3..8].copy_from_slice(&lock_bytes[11..16]);
    let unlocks_at = u64::from_be_bytes(ua);
    let now: u64 = evm.ctx().block().timestamp().to();
    if now < unlocks_at {
        return Err(eip8130_invalid_tx::<ERROR>("config changes not allowed: account is locked"));
    }

    if eip8130.sequence_updates.is_empty() {
        return Ok(());
    }

    // Sequence check with in-tx chaining.
    let seq_slot = eip8130.sequence_updates[0].slot;
    let packed = evm.ctx().journal_mut().sload(ACCOUNT_CONFIG_ADDRESS, seq_slot)?.data;
    let mut expected_multichain = read_packed_sequence(packed, true);
    let mut expected_local = read_packed_sequence(packed, false);

    for upd in &eip8130.sequence_updates {
        let tx_sequence = upd.new_value.checked_sub(1).ok_or_else(|| {
            eip8130_invalid_tx::<ERROR>("invalid config change sequence (underflow)")
        })?;

        if upd.is_multichain {
            if tx_sequence != expected_multichain {
                return Err(eip8130_invalid_tx::<ERROR>(format!(
                    "config change sequence mismatch: expected {expected_multichain}, got {tx_sequence}"
                )));
            }
            expected_multichain = upd.new_value;
        } else {
            if tx_sequence != expected_local {
                return Err(eip8130_invalid_tx::<ERROR>(format!(
                    "config change sequence mismatch: expected {expected_local}, got {tx_sequence}"
                )));
            }
            expected_local = upd.new_value;
        }
    }

    Ok(())
}

/// Runs a custom verifier STATICCALL and decodes the returned owner_id.
///
/// Charges gas against the transaction's custom-verifier budget via
/// `verification_gas_used`.
pub(crate) fn run_custom_verifier_staticcall<EVM, ERROR, FRAME>(
    mainnet: &mut MainnetHandler<EVM, ERROR, FRAME>,
    evm: &mut EVM,
    verifier: Address,
    calldata: &Bytes,
    caller: Address,
    verification_gas_cap: u64,
    verification_gas_used: &mut u64,
    call_failed_msg: &'static str,
    invalid_owner_id_msg: &'static str,
) -> Result<U256, ERROR>
where
    EVM: EvmTr<Context: OpContextTr, Frame = FRAME>,
    ERROR: EvmTrError<EVM> + From<OpTransactionError>,
    FRAME: FrameTr<FrameResult = FrameResult, FrameInit = FrameInit>,
{
    // Load verifier account with code so we can pass its bytecode to revm 38's
    // `CallInputs::known_bytecode` (revm 38 made this non-optional and expects
    // the caller to materialize the bytecode tuple).
    let verifier_known_bytecode = {
        let info = &evm.ctx().journal_mut().load_account_with_code(verifier)?.data.info;
        (info.code_hash(), info.code.clone().unwrap_or_default())
    };

    let call_gas = verification_gas_cap.saturating_sub(*verification_gas_used);
    let call_inputs = CallInputs {
        input: CallInput::Bytes(calldata.clone()),
        return_memory_offset: 0..0,
        gas_limit: call_gas,
        // EIP-8037 reservoir: 0 for verifier STATICCALLs (not propagating any
        // pre-charged state gas budget into the verifier sub-frame).
        reservoir: 0,
        bytecode_address: verifier,
        known_bytecode: verifier_known_bytecode,
        target_address: verifier,
        caller,
        value: CallValue::Transfer(U256::ZERO),
        scheme: CallScheme::StaticCall,
        is_static: true,
    };

    let frame_init = FrameInit {
        depth: 0,
        memory: {
            let ctx = evm.ctx();
            let mut mem = SharedMemory::new_with_buffer(ctx.local().shared_memory_buffer().clone());
            mem.set_memory_limit(ctx.cfg().memory_limit());
            mem
        },
        frame_input: FrameInput::Call(Box::new(call_inputs)),
    };

    let result = mainnet.run_exec_loop(evm, frame_init)?;
    let used = call_gas.saturating_sub(result.gas().remaining());
    *verification_gas_used = verification_gas_used.saturating_add(used);

    if !result.interpreter_result().result.is_ok() {
        return Err(eip8130_invalid_tx::<ERROR>(call_failed_msg));
    }

    let output = result.interpreter_result().output.as_ref();
    if output.len() < 32 {
        return Err(eip8130_invalid_tx::<ERROR>(invalid_owner_id_msg));
    }

    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&output[..32]);
    Ok(U256::from_be_bytes(bytes))
}

/// Validates the config change authorizer chain.
///
/// Iterates `authorizer_validations` in order. For each entry:
/// - Custom verifiers (0x00): STATICCALL the verifier to get `owner_id`, then
///   check `owner_config(sender, owner_id)` for CONFIG scope.
/// - Native verifiers: check the pre-authenticated `owner_id` against
///   `owner_config(sender, owner_id)` for CONFIG scope.
///
/// **Chaining:** AUTHORIZE operations from earlier entries are tracked in an
/// in-memory map. When a later entry's authorizer is a newly-authorized owner,
/// the scope check uses the pending entry rather than an SLOAD. This allows a
/// single tx to chain: entry 1 authorized by existing owner adds new owner X,
/// entry 2 authorized by X does further changes.
///
/// Uses the transaction's `custom_verifier_gas_cap` budget for custom
/// authorizer STATICCALLs. `verification_gas_used` is updated to reflect gas
/// consumed.
pub(crate) fn validate_authorizer_chain<EVM, ERROR, FRAME>(
    mainnet: &mut MainnetHandler<EVM, ERROR, FRAME>,
    evm: &mut EVM,
    sender: Address,
    eip8130: &Eip8130Parts,
    verification_gas_used: &mut u64,
) -> Result<HashMap<U256, PendingOwnerState>, ERROR>
where
    EVM: EvmTr<Context: OpContextTr, Frame = FRAME>,
    ERROR: EvmTrError<EVM> + From<OpTransactionError>,
    FRAME: FrameTr<FrameResult = FrameResult, FrameInit = FrameInit>,
{
    if eip8130.authorizer_validations.is_empty() {
        return Ok(HashMap::new());
    }

    // Pending additions from earlier entries in the chain.
    // Maps owner_id -> effective pending owner state.
    let mut pending_owners: HashMap<U256, PendingOwnerState> = HashMap::new();

    for validation in &eip8130.authorizer_validations {
        // Placeholder entries (no auth payload) are used for empty/malformed
        // config-change auth blobs and should be ignored here. Native-authorized
        // entries also have `verify_call == None`, so we only skip the true
        // placeholder shape.
        if validation.verify_call.is_none()
            && validation.owner_id == B256::ZERO
            && validation.owner_changes.is_empty()
        {
            continue;
        }

        let owner_id = if let Some(verify_call) = &validation.verify_call {
            // Custom verifier: STATICCALL to get owner_id.
            run_custom_verifier_staticcall::<EVM, ERROR, FRAME>(
                mainnet,
                evm,
                verify_call.verifier,
                &verify_call.calldata,
                sender,
                eip8130.custom_verifier_gas_cap,
                verification_gas_used,
                "config change authorizer STATICCALL failed",
                "config change authorizer returned invalid owner_id",
            )?
        } else {
            // Native verifier: owner_id was pre-authenticated at conversion time.
            U256::from_be_bytes(validation.owner_id.0)
        };

        if owner_id.is_zero() {
            return Err(eip8130_invalid_tx::<ERROR>(
                "config change authorizer returned zero owner_id",
            ));
        }

        // Check CONFIG scope with pending overrides first, then on-chain.
        validate_owner_against_effective_config::<EVM, ERROR>(
            evm,
            sender,
            owner_id,
            validation.verifier,
            crate::constants::OWNER_SCOPE_CONFIG,
            true,
            Some(&pending_owners),
        )?;

        // Record pending additions from this entry for chaining.
        for op in &validation.owner_changes {
            if let Some(state) =
                pending_owner_state_for_change(op.change_type, op.verifier, op.scope)
            {
                pending_owners.insert(U256::from_be_bytes(op.owner_id.0), state);
            }
        }
    }

    Ok(pending_owners)
}
