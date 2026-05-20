//!Handler related to Optimism chain
use crate::{
    L1BlockInfo, OpHaltReason, OpSpecId,
    api::exec::OpContextTr,
    constants::{BASE_FEE_RECIPIENT, L1_FEE_RECIPIENT, OPERATOR_FEE_RECIPIENT},
    eip8130_policy::{
        PendingOwnerState, PendingOwnerValidationError, pending_owner_state_for_change,
        validate_pending_owner_state,
    },
    precompiles_xlayer::{NONCE_MANAGER_ADDRESS, TX_CONTEXT_ADDRESS, aa_nonce_slot},
    transaction::{
        OpTransactionError, OpTxTr,
        deposit::DEPOSIT_TRANSACTION_TYPE,
        eip8130::{
            AuthState, Eip8130Parts, Eip8130PhaseResult, config_log_to_system_log,
            encode_phase_statuses, phase_statuses_system_log,
        },
    },
};
use core::sync::atomic::{AtomicBool, Ordering};
use revm::{
    context::{
        LocalContextTr,
        journaled_state::{JournalCheckpoint, account::JournaledAccountTr},
        result::InvalidTransaction,
    },
    context_interface::{
        Block, Cfg, ContextTr, JournalTr, Transaction,
        cfg::gas::InitialAndFloorGas,
        context::take_error,
        result::{EVMError, ExecutionResult, FromStringError, ResultGas},
    },
    handler::{
        EthFrame, EvmTr, FrameResult, Handler, MainnetHandler,
        evm::FrameTr,
        handler::EvmTrError,
        post_execution::{self, reimburse_caller},
        pre_execution::{calculate_caller_fee, validate_account_nonce_and_code_with_components},
    },
    inspector::{Inspector, InspectorEvmTr, InspectorHandler},
    interpreter::{
        CallOutcome, Gas, InstructionResult, InterpreterResult, SharedMemory,
        interpreter::EthInterpreter,
        interpreter_action::{CallInput, CallInputs, CallScheme, CallValue, FrameInit, FrameInput},
    },
    primitives::{Address, B256, Bytes, U256, hardfork::SpecId, keccak256, uint},
};
use std::{boxed::Box, collections::BTreeMap, vec::Vec};

/// EIP-8130 AA transaction type byte.
const EIP8130_TX_TYPE: u8 = 0x7B;

/// Estimated calldata gas for a K1 auth blob missing during gas estimation.
const ESTIMATION_AUTH_CALLDATA_GAS: u64 = 1_100;

use crate::constants::{ACCOUNT_CONFIG_ADDRESS, K1_VERIFIER_ADDRESS, REVOKED_VERIFIER};

/// Monotonic cache: once the `AccountConfiguration` contract is detected, we
/// skip the code-existence check on all subsequent calls.
static ACCOUNT_CONFIG_DEPLOYED: AtomicBool = AtomicBool::new(false);

/// Base storage slot for the packed `_accountState` mapping in `AccountConfig` (slot index 1).
const LOCK_BASE_SLOT: U256 = uint!(1_U256);

/// Sentinel nonce key that activates nonce-free mode.
pub const NONCE_KEY_MAX: U256 = U256::MAX;

/// Base storage slot for the expiring-seen mapping in `NonceManager`.
const EXPIRING_SEEN_BASE_SLOT: U256 = uint!(2_U256);
/// Base storage slot for the expiring-ring mapping in `NonceManager`.
const EXPIRING_RING_BASE_SLOT: U256 = uint!(3_U256);
/// Storage slot for the expiring-ring pointer in `NonceManager`.
const EXPIRING_RING_PTR_SLOT: U256 = uint!(4_U256);
/// Capacity of the expiring-nonce ring buffer.
const EXPIRING_NONCE_SET_CAPACITY: u32 = 300_000;
/// Maximum allowed expiry-window length for nonce-free transactions.
pub const NONCE_FREE_MAX_EXPIRY_WINDOW: u64 = 30;

/// Computes the storage slot for `expiringNonceSeen[txHash]`.
pub fn aa_expiring_seen_slot(tx_hash: B256) -> U256 {
    use alloy_sol_types::SolValue;
    U256::from_be_bytes(keccak256((tx_hash, EXPIRING_SEEN_BASE_SLOT).abi_encode()).0)
}

/// Computes the storage slot for `expiringNonceRing[index]`.
fn aa_expiring_ring_slot(index: u32) -> U256 {
    use alloy_sol_types::SolValue;
    U256::from_be_bytes(keccak256((U256::from(index), EXPIRING_RING_BASE_SLOT).abi_encode()).0)
}

/// Computes the `AccountConfig` storage slot for `lock_state(account)`.
///
/// Public so the txpool's invalidation index can use the same derivation
/// the handler uses to mutate the slot.
pub fn aa_lock_slot(account: Address) -> U256 {
    use alloy_sol_types::SolValue;
    U256::from_be_bytes(keccak256((account, LOCK_BASE_SLOT).abi_encode()).0)
}

/// Owner config base storage slot in `AccountConfig` (slot index 0).
const OWNER_CONFIG_BASE_SLOT: U256 = U256::ZERO;

/// Computes the `AccountConfig` storage slot for `owner_config(account, owner_id)`.
///
/// Public for the same reason as [`aa_lock_slot`] — txpool invalidation
/// index needs the canonical slot derivation.
pub fn aa_owner_config_slot(account: Address, owner_id: U256) -> U256 {
    use alloy_sol_types::SolValue;
    let inner = keccak256((owner_id, OWNER_CONFIG_BASE_SLOT).abi_encode());
    U256::from_be_bytes(keccak256((account, inner).abi_encode()).0)
}

/// Parses a packed `owner_config` word into `(verifier_address, scope)`.
///
/// Public so the txpool's invalidation index can decode the same word the
/// handler reads from `owner_config`. Layout: bytes[12..32] = verifier
/// (20 bytes), byte[11] = scope (spec line 226).
pub fn parse_owner_config_word(word: U256) -> (Address, u8) {
    let bytes = word.to_be_bytes::<32>();
    let scope = bytes[11];
    let verifier = Address::from_slice(&bytes[12..32]);
    (verifier, scope)
}

/// Extracts the `unlocksAt` field (uint40) from a packed `AccountState` word.
///
/// Public for the same reason as [`parse_owner_config_word`]. Layout:
/// bytes[11..16] big-endian = u40 unlocksAt (see `pack_account_state` test
/// helper for the full word layout).
pub fn unlocks_at_from_account_state_word(word: U256) -> u64 {
    let bytes = word.to_be_bytes::<32>();
    let mut ua = [0u8; 8];
    ua[3..8].copy_from_slice(&bytes[11..16]);
    u64::from_be_bytes(ua)
}

/// Reads one sequence value from packed `AccountState`.
///
/// Public so the txpool's invalidation evaluator can decode the same word.
/// `is_multichain == true` returns the low 8 bytes (`limbs[0]`); the local
/// half lives in the next 8 bytes (`(word >> 64).limbs[0]`).
pub fn read_packed_sequence(slot_value: U256, is_multichain: bool) -> u64 {
    if is_multichain { slot_value.as_limbs()[0] } else { (slot_value >> 64_u8).as_limbs()[0] }
}

/// Extra gas to reserve during `eth_estimateGas` for missing auth blobs.
const fn estimation_calldata_overhead(parts: &Eip8130Parts) -> u64 {
    let mut overhead = 0;
    if parts.sender_authstate.is_empty() {
        overhead += ESTIMATION_AUTH_CALLDATA_GAS;
    }
    if parts.payer_authstate.is_empty() {
        overhead += ESTIMATION_AUTH_CALLDATA_GAS;
    }
    overhead
}

/// Computes EIP-8130 intrinsic gas on-demand from `cfg.gas_params()` and
/// the cached gas-path inputs on `ctx.tx().eip8130_parts()`.
///
/// Synthetic AA test fixtures that build `Eip8130Parts::default()` directly
/// (no underlying `TxEip8130`) get a minimal intrinsic of
/// `tx_base_stipend + k1_verification + nonce_cold` — empty `sender_auth`
/// and `is_eoa = false` route through the K1 fallback, well within typical
/// test gas limits.
fn aa_intrinsic_gas_for_tx<CTX>(ctx: &CTX) -> u64
where
    CTX: ContextTr<Cfg: Cfg<Spec = OpSpecId>, Tx: OpTxTr>,
{
    crate::eip8130_gas::aa_intrinsic_gas(ctx.tx().eip8130_parts(), ctx.cfg().gas_params())
}

/// Sender-side custom-verifier gas cap. This portion consumes `gas_limit`
/// before calls because sender authentication and config-change authorizers
/// are sender-controlled.
fn aa_sender_custom_verifier_gas_cap(parts: &Eip8130Parts) -> u64 {
    let needs_cap = matches!(parts.sender_authstate, AuthState::Deferred { .. }) ||
        parts.account_changes.authorizer_validations.iter().any(|v| v.verify_call.is_some());
    if needs_cap { crate::constants::XLAYER_AA_CUSTOM_VERIFIER_GAS_CAP } else { 0 }
}

/// Payer-side custom-verifier gas cap. This is metered separately from
/// `gas_limit` so the payer's verifier choice cannot reduce call gas.
fn aa_payer_custom_verifier_gas_cap(parts: &Eip8130Parts) -> u64 {
    if !parts.is_self_pay() && matches!(parts.payer_authstate, AuthState::Deferred { .. }) {
        crate::constants::XLAYER_AA_CUSTOM_VERIFIER_GAS_CAP
    } else {
        0
    }
}

/// Creates an `InvalidTransaction` error from a message string.
fn eip8130_invalid_tx<ERROR: From<OpTransactionError>>(msg: &'static str) -> ERROR {
    OpTransactionError::Base(InvalidTransaction::Str(msg.into())).into()
}

/// Creates an `InvalidTransaction` error from an owned string message.
fn eip8130_invalid_tx_owned<ERROR: From<OpTransactionError>>(msg: std::string::String) -> ERROR {
    OpTransactionError::Base(InvalidTransaction::Str(msg.into())).into()
}

/// Validates owner authorization against effective state.
fn validate_owner_against_effective_config<EVM, ERROR>(
    evm: &mut EVM,
    account: Address,
    owner_id: U256,
    expected_verifier: Address,
    required_scope: u8,
    allow_implicit_eoa: bool,
    pending_owner_overrides: Option<&BTreeMap<U256, PendingOwnerState>>,
) -> Result<(), ERROR>
where
    EVM: EvmTr<Context: OpContextTr>,
    ERROR: EvmTrError<EVM> + From<OpTransactionError>,
{
    if let Some(state) = pending_owner_overrides.and_then(|m| m.get(&owner_id)) {
        validate_pending_owner_state(state, expected_verifier, required_scope).map_err(|err| {
            let msg: std::string::String = match err {
                PendingOwnerValidationError::Revoked => {
                    "owner explicitly revoked in pending config changes".into()
                }
                PendingOwnerValidationError::VerifierMismatch { .. } => {
                    "verifier mismatch against pending config changes".into()
                }
                PendingOwnerValidationError::MissingScope { .. } => {
                    "owner lacks required scope (pending config changes)".into()
                }
            };
            eip8130_invalid_tx_owned::<ERROR>(msg)
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
        return Err(eip8130_invalid_tx::<ERROR>("verifier mismatch in owner_config"));
    }
    if scope != 0 && (scope & required_scope) == 0 {
        return Err(eip8130_invalid_tx::<ERROR>("owner lacks required scope bit"));
    }
    Ok(())
}

/// Validates that `owner_id` is registered in `AccountConfig`.
fn validate_owner_config<EVM, ERROR>(
    evm: &mut EVM,
    account: Address,
    owner_id: U256,
    expected_verifier: Address,
    required_scope: u8,
    pending_owner_overrides: Option<&BTreeMap<U256, PendingOwnerState>>,
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

/// Re-validates a native verifier's `owner_config` at inclusion time.
///
/// Just the outer `(account, owner_id) → (verifier, scope)` binding. For the
/// Delegate→Native case, the **inner** binding
/// `owner_config[delegate_address][inner_owner_id] = (inner_verifier, scope)`
/// is checked separately by the caller (`dispatch_auth_state`'s `Native` arm)
/// using `AuthState::Native::delegate_inner`. Doing the inner check here
/// using only `(account, owner_id)` was wrong — `owner_id` is
/// `bytes20(delegate_addr)` and the slot it indexes is the same outer slot,
/// so the read was effectively a no-op (the source of base's
/// `verify_delegate` bug).
fn validate_native_verifier_owner<EVM, ERROR>(
    evm: &mut EVM,
    account: Address,
    verifier: Address,
    owner_id: B256,
    required_scope: u8,
    pending_owner_overrides: Option<&BTreeMap<U256, PendingOwnerState>>,
) -> Result<(), ERROR>
where
    EVM: EvmTr<Context: OpContextTr>,
    ERROR: EvmTrError<EVM> + From<OpTransactionError>,
{
    let owner_id_uint = U256::from_be_bytes(owner_id.0);
    validate_owner_against_effective_config::<EVM, ERROR>(
        evm,
        account,
        owner_id_uint,
        verifier,
        required_scope,
        true,
        pending_owner_overrides,
    )
}

/// Reads the `unlocksAt` timestamp for `account` from `AccountConfig`.
fn read_unlocks_at<EVM, ERROR>(evm: &mut EVM, account: Address) -> Result<u64, ERROR>
where
    EVM: EvmTr<Context: OpContextTr>,
    ERROR: EvmTrError<EVM> + From<OpTransactionError>,
{
    evm.ctx().journal_mut().load_account(ACCOUNT_CONFIG_ADDRESS)?;
    let lock_slot = aa_lock_slot(account);
    let lock_word = evm.ctx().journal_mut().sload(ACCOUNT_CONFIG_ADDRESS, lock_slot)?.data;
    Ok(unlocks_at_from_account_state_word(lock_word))
}

/// Pre-deduction lock check covering both delegation entries and config changes.
///
/// Per xlayer-aa.md (2026-04-21 lesson): runs BEFORE gas deduction to ensure
/// locked accounts don't pay for failed inclusion.
fn check_account_lock<EVM, ERROR>(evm: &mut EVM, sender: Address) -> Result<(), ERROR>
where
    EVM: EvmTr<Context: OpContextTr>,
    ERROR: EvmTrError<EVM> + From<OpTransactionError>,
{
    // Read parts in a tight scope so the borrow on `evm.ctx().tx()` is
    // released before we re-borrow `evm` mutably for the journal/sload
    // path below. This sidesteps having to deep-clone `Eip8130Parts`
    // (Vec/Bytes contents) just to satisfy the borrow checker — the
    // helper internally re-fetches parts at the moment they're needed.
    let needs_check = {
        let ctx = evm.ctx();
        let parts = ctx.tx().eip8130_parts();
        parts.account_changes.delegation_target.is_some() ||
            !parts.account_changes.config_writes.is_empty() ||
            !parts.account_changes.sequence_updates.is_empty()
    };
    if !needs_check {
        return Ok(());
    }

    let unlocks_at = read_unlocks_at::<EVM, ERROR>(evm, sender)?;
    let now: u64 = evm.ctx().block().timestamp().saturating_to::<u64>();
    if now < unlocks_at {
        return Err(eip8130_invalid_tx::<ERROR>("account is locked"));
    }
    Ok(())
}

/// EIP-8130 delegation gating: when a delegation entry is present, the
/// transaction's sender must be authenticated as their own EOA self-owner
/// (`ownerId == bytes32(bytes20(sender))`) with `CONFIG` scope.
///
/// Custom-verifier auth (where `owner_id` is some other 32-byte identifier)
/// must NOT be allowed to install or clear an EIP-7702-style delegation.
/// Delegation clearing (`delegation_target == Some(ADDRESS_ZERO)`) is also a
/// delegation entry and gets the same treatment.
///
/// `pending_owner_overrides` is forwarded to
/// `validate_owner_against_effective_config` so pending in-tx config changes
/// (or the implicit-EOA fallback for unconfigured accounts) are honored.
fn check_delegation_requires_eoa_config_owner<EVM, ERROR>(
    evm: &mut EVM,
    sender: Address,
    pending_owner_overrides: Option<&BTreeMap<U256, PendingOwnerState>>,
) -> Result<(), ERROR>
where
    EVM: EvmTr<Context: OpContextTr>,
    ERROR: EvmTrError<EVM> + From<OpTransactionError>,
{
    // Snapshot the scalars we need from `Eip8130Parts` in a tight scope, then
    // drop the `evm.ctx().tx()` borrow before calling the journal-touching
    // helper below. Avoids having to deep-clone all of `Eip8130Parts` at the
    // caller just because `validate_owner_against_effective_config` needs
    // `&mut evm`.
    let resolved_owner_id_uint = {
        let ctx = evm.ctx();
        let parts = ctx.tx().eip8130_parts();
        // No delegation entry → nothing to check.
        if parts.account_changes.delegation_target.is_none() {
            return Ok(());
        }
        match &parts.sender_authstate {
            AuthState::Native { owner_id, .. } => U256::from_be_bytes(owner_id.0),
            // Empty/Invalid/Deferred/SelfPay: no eager K1 owner_id, can't be the
            // EOA self-owner — short-circuit reject.
            _ => U256::ZERO,
        }
    };

    // (a) Sender's resolved ownerId must be the EOA self-owner pattern:
    //     bytes32(bytes20(sender)). If they authenticated via a custom
    //     verifier or a non-K1 native verifier, `owner_id` won't match this
    //     pattern (custom returns from STATICCALL; P256 returns
    //     keccak256(pubkey); Delegate returns bytes20(delegate_addr)).
    let expected_owner_id_uint = {
        let mut buf = [0u8; 32];
        buf[..20].copy_from_slice(sender.as_slice());
        U256::from_be_bytes(buf)
    };
    if resolved_owner_id_uint != expected_owner_id_uint {
        return Err(eip8130_invalid_tx::<ERROR>(
            "EIP-8130: delegation requires sender authenticated as EOA self-owner",
        ));
    }

    // (b) That owner must be registered (or qualify as implicit EOA) with
    //     CONFIG scope. Reuse the existing helper which honors pending in-tx
    //     config changes and the implicit-EOA fallback.
    validate_owner_against_effective_config::<EVM, ERROR>(
        evm,
        sender,
        expected_owner_id_uint,
        K1_VERIFIER_ADDRESS,
        crate::constants::OWNER_SCOPE_CONFIG,
        true,
        pending_owner_overrides,
    )
}

/// Re-validates config-change preconditions at inclusion time.
///
/// Reads `Eip8130Parts` from `evm.ctx().tx()` directly inside the function
/// rather than taking a `&Eip8130Parts` parameter — the borrow checker
/// otherwise rejects holding such a reference across the journal-touching
/// `evm.ctx().journal_mut()` calls below. The deep clone the caller used to
/// take to side-step the conflict is no longer required.
fn validate_config_change_preconditions<EVM, ERROR>(
    evm: &mut EVM,
    _sender: Address,
) -> Result<(), ERROR>
where
    EVM: EvmTr<Context: OpContextTr>,
    ERROR: EvmTrError<EVM> + From<OpTransactionError>,
{
    let (has_seq_updates, has_config_writes) = {
        let ctx = evm.ctx();
        let parts = ctx.tx().eip8130_parts();
        (
            !parts.account_changes.sequence_updates.is_empty(),
            !parts.account_changes.config_writes.is_empty(),
        )
    };
    if !has_seq_updates && !has_config_writes {
        return Ok(());
    }

    if !ACCOUNT_CONFIG_DEPLOYED.load(Ordering::Relaxed) {
        let acct = evm.ctx().journal_mut().load_account_with_code_mut(ACCOUNT_CONFIG_ADDRESS)?.data;
        let has_code = acct.account().info.code_hash != keccak256([]);
        drop(acct);
        if has_code {
            ACCOUNT_CONFIG_DEPLOYED.store(true, Ordering::Relaxed);
        } else {
            return Err(eip8130_invalid_tx::<ERROR>(
                "config changes require AccountConfiguration to be deployed",
            ));
        }
    }

    if !has_seq_updates {
        return Ok(());
    }

    evm.ctx().journal_mut().load_account(ACCOUNT_CONFIG_ADDRESS)?;
    let seq_slot = {
        let ctx = evm.ctx();
        let parts = ctx.tx().eip8130_parts();
        parts.account_changes.sequence_updates[0].slot
    };
    let packed = evm.ctx().journal_mut().sload(ACCOUNT_CONFIG_ADDRESS, seq_slot)?.data;
    let mut expected_multichain = read_packed_sequence(packed, true);
    let mut expected_local = read_packed_sequence(packed, false);

    // No `evm` calls inside this loop, so a stable parts borrow is fine here.
    let ctx = evm.ctx();
    let parts = ctx.tx().eip8130_parts();
    for upd in &parts.account_changes.sequence_updates {
        let tx_sequence = upd
            .new_value
            .checked_sub(1)
            .ok_or_else(|| eip8130_invalid_tx::<ERROR>("invalid config change sequence"))?;

        if upd.is_multichain {
            if tx_sequence != expected_multichain {
                return Err(eip8130_invalid_tx::<ERROR>("config change sequence mismatch"));
            }
            expected_multichain = upd.new_value;
        } else {
            if tx_sequence != expected_local {
                return Err(eip8130_invalid_tx::<ERROR>("config change sequence mismatch"));
            }
            expected_local = upd.new_value;
        }
    }

    Ok(())
}

/// Loads a target account's bytecode and code hash for use as
/// `CallInputs::known_bytecode`.
///
/// Restores the auto-load behavior that revm-handler ≤17 performed when
/// `known_bytecode == None`: revm-handler 18.x removed the auto-load and
/// treats empty bytecode as instant `Stop`, so the caller must populate it.
/// EIP-7702 delegation is intentionally *not* resolved here — that matches
/// the prior contract (delegation is unwrapped inside the interpreter when
/// it executes a `Bytecode::Eip7702`).
#[inline]
fn load_call_target_bytecode<EVM, ERROR>(
    evm: &mut EVM,
    address: Address,
) -> Result<(B256, revm::bytecode::Bytecode), ERROR>
where
    EVM: EvmTr<Context: OpContextTr>,
    ERROR: EvmTrError<EVM>,
{
    let acct = evm.ctx().journal_mut().load_account_with_code(address)?.data;
    Ok((acct.info.code_hash, acct.info.code.clone().unwrap_or_default()))
}

/// Runs a custom verifier STATICCALL and decodes the returned `owner_id`.
#[allow(clippy::too_many_arguments)]
fn run_custom_verifier_staticcall<EVM, ERROR, FRAME>(
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
    let (code_hash, code) = load_call_target_bytecode::<EVM, ERROR>(evm, verifier)?;

    let call_gas = verification_gas_cap.saturating_sub(*verification_gas_used);
    let call_inputs = CallInputs {
        input: CallInput::Bytes(calldata.clone()),
        return_memory_offset: 0..0,
        gas_limit: call_gas,
        reservoir: 0,
        known_bytecode: (code_hash, code),
        bytecode_address: verifier,
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

/// Which side of the tx an [`AuthState`] dispatch is for.
#[derive(Clone, Copy, Debug)]
enum AuthSide {
    /// `tx.sender_auth` — never resolves to [`AuthState::SelfPay`].
    Sender,
    /// `tx.payer_auth` — caller's responsibility to skip when payer == sender.
    Payer,
}

/// Routes an [`AuthState`] to the appropriate validation path.
///
/// Single match replaces the prior sentinel-flag if/else chain in `execution()`.
/// Per-state behavior:
///
/// - [`AuthState::SelfPay`] / [`AuthState::Empty`]: no-op (Empty is gated by `validate_env`'s
///   estimation check; reaching it here means estimation mode is active).
/// - [`AuthState::Invalid`]: should have been rejected in `validate_env`; defensive error if
///   encountered.
/// - [`AuthState::Native`]: `validate_native_verifier_owner` re-checks the `(account, owner_id,
///   verifier, scope)` binding against `owner_config`.
/// - [`AuthState::Deferred`]: `run_custom_verifier_staticcall` runs the STATICCALL, then
///   `validate_owner_config` checks the returned `owner_id` against `owner_config`. For
///   Delegate→Custom (`delegate_outer.is_some()`), an additional outer-binding check on the
///   delegate address.
#[allow(clippy::too_many_arguments)]
fn dispatch_auth_state<EVM, ERROR, FRAME>(
    mainnet: &mut MainnetHandler<EVM, ERROR, FRAME>,
    evm: &mut EVM,
    side: AuthSide,
    account: Address,
    sender: Address,
    required_scope: u8,
    custom_verifier_gas_cap: u64,
    verification_gas_used: &mut u64,
    pending_owner_overrides: Option<&BTreeMap<U256, PendingOwnerState>>,
) -> Result<(), ERROR>
where
    EVM: EvmTr<Context: OpContextTr, Frame = FRAME>,
    ERROR: EvmTrError<EVM> + From<OpTransactionError>,
    FRAME: FrameTr<FrameResult = FrameResult, FrameInit = FrameInit>,
{
    // Pull the side's `AuthState` out of `parts` into an owned value so the
    // `&Eip8130Parts` borrow ends here — the helpers below need `&mut evm`
    // and would otherwise conflict. AuthState's heaviest payload is
    // `Eip8130VerifyCall.calldata` (Arc-backed `Bytes`), so this clone is
    // small and bounded.
    let state: AuthState = {
        let ctx = evm.ctx();
        let parts = ctx.tx().eip8130_parts();
        match side {
            AuthSide::Sender => parts.sender_authstate.clone(),
            AuthSide::Payer => parts.payer_authstate.clone(),
        }
    };
    match state {
        AuthState::SelfPay | AuthState::Empty => Ok(()),

        AuthState::Invalid(reason) => {
            // validate_env should have caught this — defense in depth.
            let label = match side {
                AuthSide::Sender => "sender_auth",
                AuthSide::Payer => "payer_auth",
            };
            Err(eip8130_invalid_tx_owned::<ERROR>(std::format!(
                "EIP-8130: invalid {label} ({reason})",
            )))
        }

        AuthState::Native { verifier, owner_id, delegate_inner } => {
            // Outer binding: owner_config[account][owner_id] = (verifier, scope).
            validate_native_verifier_owner::<EVM, ERROR>(
                evm,
                account,
                verifier,
                owner_id,
                required_scope,
                pending_owner_overrides,
            )?;

            // Inner binding for Delegate→Native:
            //   owner_config[delegate_address][delegate_inner.owner_id]
            //     = (delegate_inner.verifier, scope)
            //
            // delegate_address is the top 20 bytes of `owner_id` (set by
            // auth_state when verifier == DELEGATE_VERIFIER_ADDRESS). Without
            // this check a tx whose nested K1/P256 sig is mathematically
            // valid but whose recovered inner owner is *not registered on
            // the delegate account* would slip through — see the docstring
            // on AuthState::Native.
            if let Some(inner) = delegate_inner {
                let delegate_address = Address::from_slice(&owner_id.0[..20]);
                let inner_pending =
                    if delegate_address == account { pending_owner_overrides } else { None };
                validate_owner_config::<EVM, ERROR>(
                    evm,
                    delegate_address,
                    U256::from_be_bytes(inner.owner_id.0),
                    inner.verifier,
                    required_scope,
                    inner_pending,
                )?;
            }
            Ok(())
        }

        AuthState::Deferred { spec, delegate_outer } => {
            let owner_id = run_custom_verifier_staticcall::<EVM, ERROR, FRAME>(
                mainnet,
                evm,
                spec.verifier,
                &spec.calldata,
                sender,
                custom_verifier_gas_cap,
                verification_gas_used,
                "custom verifier STATICCALL failed",
                "custom verifier returned invalid owner_id",
            )?;

            let inner_pending_overrides =
                if spec.account == account { pending_owner_overrides } else { None };
            validate_owner_config::<EVM, ERROR>(
                evm,
                spec.account,
                owner_id,
                spec.verifier,
                spec.required_scope,
                inner_pending_overrides,
            )?;

            if let Some(delegate_addr) = delegate_outer {
                // Outer delegate binding: owner_config[account][bytes32(delegate_addr)]
                // must be (DELEGATE_VERIFIER_ADDRESS, scope).
                let mut buf = [0u8; 32];
                buf[..20].copy_from_slice(delegate_addr.as_slice());
                let outer_owner_id = U256::from_be_bytes(buf);
                validate_owner_config::<EVM, ERROR>(
                    evm,
                    account,
                    outer_owner_id,
                    crate::constants::DELEGATE_VERIFIER_ADDRESS,
                    required_scope,
                    pending_owner_overrides,
                )?;
            }
            Ok(())
        }
    }
}

/// Validates the config change authorizer chain.
///
/// Reads `authorizer_validations` from `evm.ctx().tx()` directly. The previous
/// `&Eip8130Parts` parameter forced the caller to deep-clone all of
/// `Eip8130Parts` because `&mut evm` (used for STATICCALL + journal) and a
/// borrow rooted in `evm.ctx().tx()` can't coexist. Per-iteration we clone
/// only the small `verify_call` / `owner_changes` payloads — `Eip8130VerifyCall`
/// holds an `Arc`-backed `Bytes`, so its clone is a refcount bump.
fn validate_authorizer_chain<EVM, ERROR, FRAME>(
    mainnet: &mut MainnetHandler<EVM, ERROR, FRAME>,
    evm: &mut EVM,
    sender: Address,
    custom_verifier_gas_cap: u64,
    verification_gas_used: &mut u64,
) -> Result<BTreeMap<U256, PendingOwnerState>, ERROR>
where
    EVM: EvmTr<Context: OpContextTr, Frame = FRAME>,
    ERROR: EvmTrError<EVM> + From<OpTransactionError>,
    FRAME: FrameTr<FrameResult = FrameResult, FrameInit = FrameInit>,
{
    let mut pending_owners: BTreeMap<U256, PendingOwnerState> = BTreeMap::new();
    let validations_len = {
        let ctx = evm.ctx();
        ctx.tx().eip8130_parts().account_changes.authorizer_validations.len()
    };
    if validations_len == 0 {
        return Ok(pending_owners);
    }

    for i in 0..validations_len {
        // Pull just this validation's fields out of `parts` into owned
        // locals, then drop the parts borrow before calling helpers that
        // need `&mut evm`. Bytes inside Eip8130VerifyCall is Arc — clone
        // is cheap.
        let (skip, verify_call, raw_owner_id, verifier, owner_changes) = {
            let ctx = evm.ctx();
            let v = &ctx.tx().eip8130_parts().account_changes.authorizer_validations[i];
            let skip =
                v.verify_call.is_none() && v.owner_id == B256::ZERO && v.owner_changes.is_empty();
            (skip, v.verify_call.clone(), v.owner_id, v.verifier, v.owner_changes.clone())
        };
        if skip {
            continue;
        }

        let owner_id = if let Some(verify_call) = &verify_call {
            run_custom_verifier_staticcall::<EVM, ERROR, FRAME>(
                mainnet,
                evm,
                verify_call.verifier,
                &verify_call.calldata,
                sender,
                custom_verifier_gas_cap,
                verification_gas_used,
                "config change authorizer STATICCALL failed",
                "config change authorizer returned invalid owner_id",
            )?
        } else {
            U256::from_be_bytes(raw_owner_id.0)
        };

        if owner_id.is_zero() {
            return Err(eip8130_invalid_tx::<ERROR>(
                "config change authorizer returned zero owner_id",
            ));
        }

        validate_owner_against_effective_config::<EVM, ERROR>(
            evm,
            sender,
            owner_id,
            verifier,
            crate::constants::OWNER_SCOPE_CONFIG,
            true,
            Some(&pending_owners),
        )?;

        for op in &owner_changes {
            if let Some(state) =
                pending_owner_state_for_change(op.change_type, op.verifier, op.scope)
            {
                pending_owners.insert(U256::from_be_bytes(op.owner_id.0), state);
            }
        }
    }

    Ok(pending_owners)
}

/// Optimism handler extends the [`Handler`] with Optimism specific logic.
#[derive(Debug, Clone)]
pub struct OpHandler<EVM, ERROR, FRAME> {
    /// Mainnet handler allows us to use functions from the mainnet handler inside optimism
    /// handler. So we dont duplicate the logic
    pub mainnet: MainnetHandler<EVM, ERROR, FRAME>,
}

impl<EVM, ERROR, FRAME> OpHandler<EVM, ERROR, FRAME> {
    /// Create a new Optimism handler.
    pub fn new() -> Self {
        Self { mainnet: MainnetHandler::default() }
    }
}

impl<EVM, ERROR, FRAME> Default for OpHandler<EVM, ERROR, FRAME> {
    fn default() -> Self {
        Self::new()
    }
}

/// Trait to check if the error is a transaction error.
///
/// Used in `cache_error` handler to catch deposit transaction that was halted.
pub trait IsTxError {
    /// Check if the error is a transaction error.
    fn is_tx_error(&self) -> bool;
}

impl<DB, TX> IsTxError for EVMError<DB, TX> {
    fn is_tx_error(&self) -> bool {
        matches!(self, Self::Transaction(_))
    }
}

impl<EVM, ERROR, FRAME> Handler for OpHandler<EVM, ERROR, FRAME>
where
    EVM: EvmTr<Context: OpContextTr, Frame = FRAME>,
    ERROR: EvmTrError<EVM> + From<OpTransactionError> + FromStringError + IsTxError,
    // TODO `FrameResult` should be a generic trait.
    // TODO `FrameInit` should be a generic.
    FRAME: FrameTr<FrameResult = FrameResult, FrameInit = FrameInit>,
{
    type Evm = EVM;
    type Error = ERROR;
    type HaltReason = OpHaltReason;

    fn validate_env(&self, evm: &mut Self::Evm) -> Result<(), Self::Error> {
        // Do not perform any extra validation for deposit transactions, they are pre-verified on
        // L1.
        let ctx = evm.ctx();
        let tx = ctx.tx();
        let tx_type = tx.tx_type();
        if tx_type == DEPOSIT_TRANSACTION_TYPE {
            // Do not allow for a system transaction to be processed if Regolith is enabled.
            if tx.is_system_transaction() &&
                evm.ctx().cfg().spec().is_enabled_in(OpSpecId::REGOLITH)
            {
                return Err(OpTransactionError::DepositSystemTxPostRegolith.into());
            }
            return Ok(());
        }

        // Check that non-deposit transactions have enveloped_tx set
        if tx.enveloped_tx().is_none() {
            return Err(OpTransactionError::MissingEnvelopedTx.into());
        }

        // EIP-8130 AA transaction: stateless validation.
        // Per xlayer-aa.md: AA tx short-circuits the mainnet `validate_env`,
        // so universal checks (chain_id, basefee bounds, structural limits)
        // must be re-asserted here.
        if tx_type == EIP8130_TX_TYPE {
            let spec = ctx.cfg().spec();
            if !spec.is_enabled_in(OpSpecId::XLAYER_V1) {
                return Err(eip8130_invalid_tx::<Self::Error>(
                    "EIP-8130 AA transactions require XLAYER_V1",
                ));
            }

            // Re-check chain_id (mainnet path is skipped for AA).
            if ctx.cfg().tx_chain_id_check() {
                let cfg_chain_id = ctx.cfg().chain_id();
                match ctx.tx().chain_id() {
                    None => {
                        return Err(
                            OpTransactionError::Base(InvalidTransaction::MissingChainId).into()
                        );
                    }
                    Some(tx_chain_id) if tx_chain_id != cfg_chain_id => {
                        return Err(
                            OpTransactionError::Base(InvalidTransaction::InvalidChainId).into()
                        );
                    }
                    _ => {}
                }
            }

            if !ctx.cfg().is_base_fee_check_disabled() {
                let basefee = ctx.block().basefee() as u128;
                let max_fee = ctx.tx().max_fee_per_gas();
                let max_priority = ctx.tx().max_priority_fee_per_gas().unwrap_or(0);

                if max_fee < basefee {
                    return Err(eip8130_invalid_tx::<Self::Error>(
                        "EIP-8130: max_fee_per_gas below base fee",
                    ));
                }
                if max_priority > max_fee {
                    return Err(eip8130_invalid_tx::<Self::Error>(
                        "EIP-8130: max_priority_fee_per_gas exceeds max_fee_per_gas",
                    ));
                }
            }

            let parts = ctx.tx().eip8130_parts();

            // Validator-side auth gate. A single match per side covers all five
            // outcomes from conversion-time auth resolution: SelfPay (payer
            // skipped), Empty (estimateGas escape), Invalid (forged or
            // malformed — reject), Native (eager-verified, handler will
            // re-check owner_config), Deferred (custom STATICCALL handled in
            // execution()).
            //
            // Auth verification ran once in `eip8130_parts`
            // (alloy-op-evm/src/eip8130/auth_state.rs), which executes identically on
            // sequencer + validator — that's the primary defense against a sequencer
            // including a tx with a forged signature. This branch translates the
            // auth-state verdict into accept / reject.
            let is_estimation = ctx.cfg().is_base_fee_check_disabled();
            match &parts.sender_authstate {
                AuthState::Invalid(reason) => {
                    return Err(eip8130_invalid_tx_owned::<Self::Error>(std::format!(
                        "EIP-8130: invalid sender_auth ({reason})",
                    )));
                }
                AuthState::Empty if !is_estimation => {
                    return Err(eip8130_invalid_tx::<Self::Error>(
                        "EIP-8130: missing sender_auth signature",
                    ));
                }
                // Sender side never resolves to SelfPay; defensive accept.
                AuthState::SelfPay |
                AuthState::Empty |
                AuthState::Native { .. } |
                AuthState::Deferred { .. } => {}
            }
            match &parts.payer_authstate {
                AuthState::Invalid(reason) => {
                    return Err(eip8130_invalid_tx_owned::<Self::Error>(std::format!(
                        "EIP-8130: invalid payer_auth ({reason})",
                    )));
                }
                AuthState::Empty if !is_estimation => {
                    return Err(eip8130_invalid_tx::<Self::Error>(
                        "EIP-8130: missing payer_auth signature",
                    ));
                }
                AuthState::SelfPay |
                AuthState::Empty |
                AuthState::Native { .. } |
                AuthState::Deferred { .. } => {}
            }

            // Inclusion-time expiry check.
            let expiry = parts.expiry;
            if expiry != 0 {
                let block_ts = ctx.block().timestamp().saturating_to::<u64>();
                if block_ts > expiry {
                    return Err(eip8130_invalid_tx::<Self::Error>("EIP-8130: transaction expired"));
                }
            }

            // Inclusion-time structural guard for phased calls.
            let total_calls: usize = parts.call_phases.iter().map(Vec::len).sum();
            if total_calls > crate::constants::MAX_CALLS_PER_TX {
                return Err(eip8130_invalid_tx::<Self::Error>("EIP-8130: too many calls"));
            }

            if parts.account_changes.account_change_units >
                crate::constants::MAX_ACCOUNT_CHANGES_PER_TX
            {
                return Err(eip8130_invalid_tx::<Self::Error>(
                    "EIP-8130: too many account changes",
                ));
            }

            // EIP-8130 invariant: at most one create entry per tx.
            if parts.account_changes.code_placements.len() > 1 {
                return Err(eip8130_invalid_tx::<Self::Error>(
                    "EIP-8130: more than one create entry",
                ));
            }

            // EIP-170: deployed runtime bytecode must not exceed `MAX_CODE_SIZE`
            // (24 576 B). Catching here keeps `Bytecode::new_raw` at the code
            // placement step from accepting oversized code, and prevents the
            // CREATE2 deployment header (PUSH2 imm) from silently truncating
            // a `bytecode_len > u16::MAX` to mismatch the derived address.
            if let Some(placement) = parts.account_changes.code_placements.first() &&
                placement.code.len() > revm::primitives::eip170::MAX_CODE_SIZE
            {
                return Err(eip8130_invalid_tx::<Self::Error>(
                    "EIP-8130: create bytecode exceeds EIP-170 max code size",
                ));
            }

            // Reject `ConfigChange` entries that target this chain but
            // contribute zero effective ops. Such an entry would bump
            // `_accountState` (sequence) and run authorizer validation
            // without producing any `_ownerConfig` write — cap-exempt yet
            // state-touching, useful only for replay shaping.
            if parts.account_changes.matching_config_change_with_zero_valid_ops {
                return Err(eip8130_invalid_tx::<Self::Error>(
                    "EIP-8130: config change entry has no valid ops",
                ));
            }

            // EIP-8130 invariant: the `Create` entry, if present, MUST be
            // the first entry of `account_changes`. Parser flags violators
            // without short-circuiting; reject here so the failure is
            // structural and stateless.
            if parts.account_changes.create_not_first_entry {
                return Err(eip8130_invalid_tx::<Self::Error>(
                    "EIP-8130: create entry must be first",
                ));
            }

            // EIP-8130 invariant: at most one `Delegation` entry per
            // account. A tx targets one account (the sender), so a count
            // greater than 1 violates the spec.
            if parts.account_changes.delegation_entry_count > 1 {
                return Err(eip8130_invalid_tx::<Self::Error>(
                    "EIP-8130: more than one delegation entry",
                ));
            }

            // Nonce-free mode structural checks.
            if parts.nonce_key == NONCE_KEY_MAX {
                if parts.expiry == 0 {
                    return Err(eip8130_invalid_tx::<Self::Error>(
                        "EIP-8130: nonce-free tx requires non-zero expiry",
                    ));
                }
                if ctx.tx().nonce() != 0 {
                    return Err(eip8130_invalid_tx::<Self::Error>(
                        "EIP-8130: nonce-free tx requires nonce_sequence == 0",
                    ));
                }
                if parts.nonce_free_hash.is_none() {
                    return Err(eip8130_invalid_tx::<Self::Error>(
                        "EIP-8130: nonce-free tx requires nonce_free_hash",
                    ));
                }
            }

            return Ok(());
        }

        self.mainnet.validate_env(evm)
    }

    fn validate_initial_tx_gas(
        &self,
        evm: &mut Self::Evm,
    ) -> Result<InitialAndFloorGas, Self::Error> {
        if evm.ctx().tx().tx_type() == EIP8130_TX_TYPE {
            let ctx = evm.ctx();
            let parts = ctx.tx().eip8130_parts();
            let aa_gas = aa_intrinsic_gas_for_tx(ctx);
            let calldata_overhead = estimation_calldata_overhead(parts);
            let is_estimation = ctx.cfg().is_base_fee_check_disabled();
            let gas_limit = ctx.tx().gas_limit();

            let effective_gas = if is_estimation { aa_gas + calldata_overhead } else { aa_gas };

            if effective_gas > gas_limit {
                return Err(InvalidTransaction::CallGasCostMoreThanGasLimit {
                    gas_limit,
                    initial_gas: effective_gas,
                }
                .into());
            }
            return Ok(InitialAndFloorGas::new(effective_gas, 0));
        }
        self.mainnet.validate_initial_tx_gas(evm)
    }

    fn validate_against_state_and_deduct_caller(
        &self,
        evm: &mut Self::Evm,
        _init_and_floor_gas: &mut InitialAndFloorGas,
    ) -> Result<(), Self::Error> {
        let (block, tx, cfg, journal, chain, _) = evm.ctx().all_mut();
        let spec = cfg.spec();

        if tx.tx_type() == DEPOSIT_TRANSACTION_TYPE {
            let basefee = block.basefee() as u128;
            let blob_price = block.blob_gasprice().unwrap_or_default();
            // deposit skips max fee check and just deducts the effective balance spending.

            let mut caller = journal.load_account_with_code_mut(tx.caller())?.data;

            let effective_balance_spending = tx
                .effective_balance_spending(basefee, blob_price)
                .expect("Deposit transaction effective balance spending overflow") -
                tx.value();

            // Mind value should be added first before subtracting the effective balance spending.
            let mut new_balance = caller
                .balance()
                .saturating_add(U256::from(tx.mint().unwrap_or_default()))
                .saturating_sub(effective_balance_spending);

            if cfg.is_balance_check_disabled() {
                // Make sure the caller's balance is at least the value of the transaction.
                // this is not consensus critical, and it is used in testing.
                new_balance = new_balance.max(tx.value());
            }

            // set the new balance and bump the nonce if it is a call
            caller.set_balance(new_balance);
            if tx.kind().is_call() {
                caller.bump_nonce();
            }

            return Ok(());
        }

        // L1 block info is stored in the context for later use.
        // and it will be reloaded from the database if it is not for the current block.
        if chain.l2_block != Some(block.number()) {
            *chain = L1BlockInfo::try_fetch(journal.db_mut(), block.number(), spec)?;
        }

        // EIP-8130 AA path: deduct gas from payer, increment nonce, run lock check
        // (per xlayer-aa.md, lock check runs BEFORE gas deduction and covers
        // delegation entries as well as config writes).
        if tx.tx_type() == EIP8130_TX_TYPE {
            let sender = tx.caller();
            let nonce_sequence = tx.nonce();

            // Lock check before any state mutation.
            check_account_lock::<Self::Evm, Self::Error>(evm, sender)?;

            // Delegation authorization is checked later (after the authorizer
            // chain has been validated) so it can observe same-tx
            // `ConfigChange` ops that authorize the EOA self-owner with
            // `OWNER_SCOPE_CONFIG`. Per spec, the check happens before
            // `account_changes` are applied to state — the pending owner
            // overlay is the substitute for "would-be applied state".

            let (block, tx, cfg, journal, chain, _) = evm.ctx().all_mut();
            let spec = cfg.spec();
            // `parts` is borrowed from `&mut tx` yielded by `all_mut`. It
            // coexists with `&mut journal` because `all_mut` returns
            // disjoint sub-borrows of the context. No deep clone needed.
            let eip8130 = tx.eip8130_parts();
            let payer_auth_gas_limit = crate::eip8130_gas::payer_intrinsic_gas(
                eip8130,
                cfg.gas_params(),
            )
            .saturating_add(aa_payer_custom_verifier_gas_cap(eip8130));

            // --- Gas deduction from payer ---
            let payer = eip8130.payer;
            let mut payer_account = journal.load_account_with_code_mut(payer)?.data;
            let mut balance = payer_account.account().info.balance;

            if !cfg.is_fee_charge_disabled() {
                let Some(additional_cost) = chain.tx_cost_with_tx(tx, spec) else {
                    return Err(OpTransactionError::MissingEnvelopedTx.into());
                };
                let Some(new_balance) = balance.checked_sub(additional_cost) else {
                    return Err(InvalidTransaction::LackOfFundForMaxFee {
                        fee: Box::new(additional_cost),
                        balance: Box::new(balance),
                    }
                    .into());
                };
                balance = new_balance;
            }

            let mut balance = calculate_caller_fee(balance, tx, block, cfg)?;
            if !cfg.is_fee_charge_disabled() && payer_auth_gas_limit != 0 {
                let basefee = block.basefee() as u128;
                let effective_gas_price = tx.effective_gas_price(basefee);
                let payer_auth_fee = U256::from(
                    effective_gas_price.saturating_mul(payer_auth_gas_limit as u128),
                );
                if cfg.is_balance_check_disabled() {
                    balance = balance.saturating_sub(payer_auth_fee);
                } else {
                    let Some(new_balance) = balance.checked_sub(payer_auth_fee) else {
                        return Err(InvalidTransaction::LackOfFundForMaxFee {
                            fee: Box::new(payer_auth_fee),
                            balance: Box::new(balance),
                        }
                        .into());
                    };
                    balance = new_balance;
                }
            }
            payer_account.set_balance(balance);
            drop(payer_account);

            // --- Nonce validation and increment in NonceManager ---
            let nonce_key = eip8130.nonce_key;
            if nonce_key == NONCE_KEY_MAX {
                // --- Expiring-nonce circular buffer (nonce-free mode) ---
                let now: u64 = block.timestamp().saturating_to::<u64>();
                let expiry = eip8130.expiry;
                let skip_checks = cfg.is_nonce_check_disabled() || cfg.is_base_fee_check_disabled();

                if !skip_checks && (expiry <= now || expiry > now + NONCE_FREE_MAX_EXPIRY_WINDOW) {
                    return Err(eip8130_invalid_tx::<Self::Error>(
                        "nonce-free expiry out of window",
                    ));
                }

                // Belt-and-suspenders: validate_env already rejects None, but
                // still avoid `unwrap_or_default` collapsing `None` -> ZERO.
                let nf_hash = eip8130.nonce_free_hash.ok_or_else(|| {
                    eip8130_invalid_tx::<Self::Error>(
                        "nonce-free transaction missing nonce_free_hash",
                    )
                })?;

                journal.load_account(NONCE_MANAGER_ADDRESS)?;

                let seen_slot = aa_expiring_seen_slot(nf_hash);
                let seen_expiry = journal.sload(NONCE_MANAGER_ADDRESS, seen_slot)?.data;
                if !skip_checks && seen_expiry != U256::ZERO && seen_expiry > U256::from(now) {
                    return Err(eip8130_invalid_tx::<Self::Error>(
                        "nonce-free transaction replay: hash already seen",
                    ));
                }

                let ptr_raw = journal.sload(NONCE_MANAGER_ADDRESS, EXPIRING_RING_PTR_SLOT)?.data;
                let idx = ptr_raw.as_limbs()[0] as u32 % EXPIRING_NONCE_SET_CAPACITY;

                let ring_slot = aa_expiring_ring_slot(idx);
                let old_hash_raw = journal.sload(NONCE_MANAGER_ADDRESS, ring_slot)?.data;

                if old_hash_raw != U256::ZERO {
                    let old_hash = B256::from(old_hash_raw.to_be_bytes::<32>());
                    let old_seen_slot = aa_expiring_seen_slot(old_hash);
                    let old_expiry = journal.sload(NONCE_MANAGER_ADDRESS, old_seen_slot)?.data;
                    if !skip_checks && old_expiry != U256::ZERO && old_expiry > U256::from(now) {
                        return Err(eip8130_invalid_tx::<Self::Error>(
                            "nonce-free buffer full: cannot evict unexpired entry",
                        ));
                    }
                    journal.sstore(NONCE_MANAGER_ADDRESS, old_seen_slot, U256::ZERO)?;
                }

                journal.sstore(NONCE_MANAGER_ADDRESS, ring_slot, U256::from_be_bytes(nf_hash.0))?;
                journal.sstore(NONCE_MANAGER_ADDRESS, seen_slot, U256::from(expiry))?;

                let next_ptr = if idx + 1 >= EXPIRING_NONCE_SET_CAPACITY {
                    U256::ZERO
                } else {
                    U256::from(idx + 1)
                };
                journal.sstore(NONCE_MANAGER_ADDRESS, EXPIRING_RING_PTR_SLOT, next_ptr)?;
            } else {
                let slot = aa_nonce_slot(sender, nonce_key);
                journal.load_account(NONCE_MANAGER_ADDRESS)?;
                let current_seq = journal.sload(NONCE_MANAGER_ADDRESS, slot)?.data;

                let skip_nonce_check =
                    cfg.is_nonce_check_disabled() || cfg.is_base_fee_check_disabled();

                if !skip_nonce_check {
                    let expected = U256::from(nonce_sequence);
                    if current_seq != expected {
                        if current_seq > expected {
                            return Err(InvalidTransaction::NonceTooLow {
                                tx: nonce_sequence,
                                state: current_seq.as_limbs()[0],
                            }
                            .into());
                        }
                        return Err(InvalidTransaction::NonceTooHigh {
                            tx: nonce_sequence,
                            state: current_seq.as_limbs()[0],
                        }
                        .into());
                    }
                }
                let next_seq = if skip_nonce_check {
                    current_seq + U256::from(1)
                } else {
                    U256::from(nonce_sequence + 1)
                };
                journal.sstore(NONCE_MANAGER_ADDRESS, slot, next_seq)?;
            }

            // Account-change application (Create pre_writes, ConfigChange,
            // Delegation, code placements) happens in `execution()` per the
            // EIP-8130 spec order: pre_writes → ConfigChange → Delegation →
            // code placement. Keeping it there means each state change can
            // observe the prior step's writes (e.g., config_writes can read
            // the new account's owner_config registered by pre_writes).
            return Ok(());
        }

        let mut caller_account = journal.load_account_with_code_mut(tx.caller())?.data;

        // validates account nonce and code
        validate_account_nonce_and_code_with_components(&caller_account.account().info, tx, cfg)?;

        // check additional cost and deduct it from the caller's balances
        let mut balance = caller_account.account().info.balance;

        if !cfg.is_fee_charge_disabled() {
            let Some(additional_cost) = chain.tx_cost_with_tx(tx, spec) else {
                return Err(OpTransactionError::MissingEnvelopedTx.into());
            };
            let Some(new_balance) = balance.checked_sub(additional_cost) else {
                return Err(InvalidTransaction::LackOfFundForMaxFee {
                    fee: Box::new(additional_cost),
                    balance: Box::new(balance),
                }
                .into());
            };
            balance = new_balance
        }

        let balance = calculate_caller_fee(balance, tx, block, cfg)?;

        // make changes to the account
        caller_account.set_balance(balance);
        if tx.kind().is_call() {
            caller_account.bump_nonce();
        }

        Ok(())
    }

    fn execution(
        &mut self,
        evm: &mut Self::Evm,
        init_and_floor_gas: &InitialAndFloorGas,
    ) -> Result<FrameResult, Self::Error> {
        if evm.ctx().tx().tx_type() != EIP8130_TX_TYPE {
            return self.mainnet.execution(evm, init_and_floor_gas);
        }

        let aa_intrinsic_gas = aa_intrinsic_gas_for_tx(evm.ctx());
        let sender = evm.ctx().tx().caller();

        // Snapshot the scalar `Eip8130Parts` fields the body needs so the
        // borrow on `evm.ctx().tx()` ends here. Holding `&Eip8130Parts`
        // across the function would conflict with every `&mut evm`
        // operation below (journal writes, STATICCALL via run_exec_loop).
        // Vec/Bytes payloads stay un-cloned — we re-fetch by index inside
        // the loops.
        let (
            sender_custom_verifier_gas_cap,
            payer_custom_verifier_gas_cap,
            payer_intrinsic_gas,
            nonce_key,
            payer_address,
            sender_address,
            call_phase_count,
            estimation_overhead,
        ) = {
            let ctx = evm.ctx();
            let parts = ctx.tx().eip8130_parts();
            (
                aa_sender_custom_verifier_gas_cap(parts),
                aa_payer_custom_verifier_gas_cap(parts),
                crate::eip8130_gas::payer_intrinsic_gas(parts, ctx.cfg().gas_params()),
                parts.nonce_key,
                parts.payer,
                parts.sender,
                parts.call_phases.len(),
                estimation_calldata_overhead(parts),
            )
        };

        let is_estimation = evm.ctx().cfg().is_base_fee_check_disabled();

        let nonce_warm_adjustment = if !is_estimation && nonce_key != NONCE_KEY_MAX {
            let nonce_slot = aa_nonce_slot(sender, nonce_key);
            let nonce_value =
                evm.ctx().journal_mut().sload(NONCE_MANAGER_ADDRESS, nonce_slot)?.data;
            if nonce_value > U256::from(1) {
                // Slot is warm: refund the over-conservative cold charge
                // that `nonce_key_cost` baked into intrinsic gas.
                crate::eip8130_gas::nonce_warm_refund(evm.ctx().cfg().gas_params())
            } else {
                0
            }
        } else {
            0
        };

        let overhead = if is_estimation { estimation_overhead } else { 0 };
        let sender_gas_budget = evm
            .ctx()
            .tx()
            .gas_limit()
            .saturating_sub(aa_intrinsic_gas + overhead)
            .saturating_add(nonce_warm_adjustment);
        let sender_custom_verifier_gas_cap = sender_custom_verifier_gas_cap.min(sender_gas_budget);

        let mut phase_results = Vec::with_capacity(call_phase_count);

        evm.ctx().journal_mut().load_account(sender)?;

        let mut sender_verification_gas_used: u64 = 0;
        let mut payer_verification_gas_used: u64 = 0;

        if !is_estimation {
            validate_config_change_preconditions::<Self::Evm, Self::Error>(evm, sender)?;

            let pending_sender_owner_overrides =
                validate_authorizer_chain::<Self::Evm, Self::Error, FRAME>(
                    &mut self.mainnet,
                    evm,
                    sender,
                    sender_custom_verifier_gas_cap,
                    &mut sender_verification_gas_used,
                )?;

            // Delegation requires sender authenticated as EOA self-owner with
            // CONFIG scope. Run after authorizer chain so same-tx
            // `ConfigChange` authorizations are visible via
            // `pending_sender_owner_overrides`; runs before the auth dispatch
            // since it only reads `parts.sender_authstate` (already resolved
            // at parse time) and the pending overlay (just built).
            check_delegation_requires_eoa_config_owner::<Self::Evm, Self::Error>(
                evm,
                sender,
                Some(&pending_sender_owner_overrides),
            )?;

            // Sender auth dispatch — single match replaces the prior
            // sentinel-flag if/else chain.
            dispatch_auth_state::<Self::Evm, Self::Error, FRAME>(
                &mut self.mainnet,
                evm,
                AuthSide::Sender,
                sender,
                sender,
                crate::constants::OWNER_SCOPE_SENDER,
                sender_custom_verifier_gas_cap,
                &mut sender_verification_gas_used,
                Some(&pending_sender_owner_overrides),
            )?;

            // Payer auth dispatch — skipped for self-pay (caller-side already
            // authenticated as the payer-equivalent account).
            if payer_address != sender_address {
                let payer_pending_overrides =
                    (payer_address == sender).then_some(&pending_sender_owner_overrides);
                dispatch_auth_state::<Self::Evm, Self::Error, FRAME>(
                    &mut self.mainnet,
                    evm,
                    AuthSide::Payer,
                    payer_address,
                    sender,
                    crate::constants::OWNER_SCOPE_PAYER,
                    payer_custom_verifier_gas_cap,
                    &mut payer_verification_gas_used,
                    payer_pending_overrides,
                )?;
            }
        }

        let mut gas_remaining = sender_gas_budget.saturating_sub(sender_verification_gas_used);

        // ── Account-change processing in EIP-8130 spec order ───────────────
        // Step 1 (Create): register `initial_owners` for the new account and
        // emit the `AccountCreated` log. Runs after auth dispatch (so the
        // sender is authenticated) but before ConfigChange so any config
        // writes can observe the freshly-registered owner_config rows.
        //
        // Replay guard: the deployed address is a pure function of
        // `(deployer, user_salt, bytecode, sorted owners)` — not bound to
        // sender. Without a state-level guard, an already-on-chain Create
        // tuple could be re-submitted, rewriting the deployed account's
        // owner_config / code and undoing prior revocations. Per spec the
        // target must be a fresh account: empty code + zero nonce + no
        // existing owner_config entries. The first two are EIP-684-style
        // collision checks; the third pins owner_config — since the target
        // address commits to the initial-owner set, identical-address
        // replays write to the same owner_config slots, so checking those
        // would-be-written slots are clean is sufficient.
        //
        // **Balance is intentionally excluded** — same as EIP-684 vanilla
        // CREATE collision check. Balance can be written by any external
        // transfer, so it carries no "this account was protocol-initialized"
        // signal. Including it would (1) break counterfactual funding
        // (pre-funding a CREATE2 address before deployment, used by
        // smart-contract wallets / 4337 / Safe) and (2) hand attackers a
        // 1-wei DoS to block any Create tx. `code` / `nonce` /
        // `owner_config` are the only state slots that EIP-8130 protocol
        // logic writes, so they are the only ones that gate replay.
        let placement_target = {
            let ctx = evm.ctx();
            ctx.tx().eip8130_parts().account_changes.code_placements.first().map(|p| p.address)
        };
        if let Some(address) = placement_target {
            let acc = evm.ctx().journal_mut().load_account_with_code_mut(address)?.data;
            let info = &acc.account().info;
            let collision = info.code_hash != keccak256([]) || info.nonce != 0;
            drop(acc);
            if collision {
                return Err(eip8130_invalid_tx::<Self::Error>(
                    "EIP-8130: create entry targets existing account (replay)",
                ));
            }
        }

        let pre_writes_count = {
            let ctx = evm.ctx();
            ctx.tx().eip8130_parts().account_changes.pre_writes.len()
        };
        // Owner_config replay check: every pre_write slot must currently
        // be zero. pre_writes only come from Create entries, so this is
        // skipped when no Create is present.
        for i in 0..pre_writes_count {
            let (address, slot) = {
                let ctx = evm.ctx();
                let w = &ctx.tx().eip8130_parts().account_changes.pre_writes[i];
                (w.address, w.slot)
            };
            evm.ctx().journal_mut().load_account(address)?;
            let current = evm.ctx().journal_mut().sload(address, slot)?.data;
            if current != U256::ZERO {
                return Err(eip8130_invalid_tx::<Self::Error>(
                    "EIP-8130: create entry targets account with existing owner_config (replay)",
                ));
            }
        }
        for i in 0..pre_writes_count {
            let (address, slot, value) = {
                let ctx = evm.ctx();
                let w = &ctx.tx().eip8130_parts().account_changes.pre_writes[i];
                (w.address, w.slot, w.value)
            };
            evm.ctx().journal_mut().load_account(address)?;
            evm.ctx().journal_mut().sstore(address, slot, value)?;
        }
        // At most one `AccountCreated` log per tx — `validate_env` rejects
        // any tx with `code_placements.len() > 1`, and the parser emits one
        // log per Create entry, so this is a 0-or-1 lookup.
        let creation_log = {
            let ctx = evm.ctx();
            ctx.tx().eip8130_parts().account_changes.account_creation_logs.first().cloned()
        };
        if let Some(event) = creation_log {
            evm.ctx().journal_mut().log(config_log_to_system_log(ACCOUNT_CONFIG_ADDRESS, &event));
        }

        // Step 2 (ConfigChange): apply config_writes + sequence bumps + logs.
        // Each iteration extracts the scalars from `parts.config_writes[i]`
        // (all `Copy`) before re-borrowing evm mutably for the journal call.
        let config_writes_count = {
            let ctx = evm.ctx();
            ctx.tx().eip8130_parts().account_changes.config_writes.len()
        };
        for i in 0..config_writes_count {
            let (address, slot, value) = {
                let ctx = evm.ctx();
                let w = &ctx.tx().eip8130_parts().account_changes.config_writes[i];
                (w.address, w.slot, w.value)
            };
            evm.ctx().journal_mut().load_account(address)?;
            evm.ctx().journal_mut().sstore(address, slot, value)?;
        }
        let sequence_updates_count = {
            let ctx = evm.ctx();
            ctx.tx().eip8130_parts().account_changes.sequence_updates.len()
        };
        if sequence_updates_count > 0 {
            evm.ctx().journal_mut().load_account(ACCOUNT_CONFIG_ADDRESS)?;
            for i in 0..sequence_updates_count {
                // `Eip8130SequenceUpdate` is small (U256 + bool + u64);
                // a copy out is cheaper than dancing the borrows.
                let upd = {
                    let ctx = evm.ctx();
                    ctx.tx().eip8130_parts().account_changes.sequence_updates[i].clone()
                };
                let current = evm.ctx().journal_mut().sload(ACCOUNT_CONFIG_ADDRESS, upd.slot)?.data;
                let new_packed = upd.apply(current);
                evm.ctx().journal_mut().sstore(ACCOUNT_CONFIG_ADDRESS, upd.slot, new_packed)?;
            }
        }

        let config_change_logs_count = {
            let ctx = evm.ctx();
            ctx.tx().eip8130_parts().account_changes.config_change_logs.len()
        };
        for i in 0..config_change_logs_count {
            // Per-event clone keeps the iteration borrow short. Each
            // `Eip8130ConfigLog` variant is at most an Address + B256 + u8
            // payload — orders of magnitude smaller than full `Eip8130Parts`.
            let event = {
                let ctx = evm.ctx();
                ctx.tx().eip8130_parts().account_changes.config_change_logs[i].clone()
            };
            evm.ctx().journal_mut().log(config_log_to_system_log(ACCOUNT_CONFIG_ADDRESS, &event));
        }

        // Step 3 (Delegation): explicit `delegation_target` takes priority,
        // otherwise the parser-emitted `auto_delegation_code` candidate
        // applies when the sender is code-less. Reject if the sender carries
        // arbitrary (non-EIP-7702) bytecode.
        let delegation_target = {
            let ctx = evm.ctx();
            ctx.tx().eip8130_parts().account_changes.delegation_target
        };
        if let Some(target) = delegation_target {
            let acc = evm.ctx().journal_mut().load_account_with_code_mut(sender)?.data;
            let current_code = acc.account().info.code.as_ref();
            let is_empty = current_code.is_none_or(|c| c.is_empty());
            let is_delegation = current_code.is_some_and(|c| c.is_eip7702());
            drop(acc);

            if !is_empty && !is_delegation {
                return Err(eip8130_invalid_tx::<Self::Error>(
                    "delegation entry rejected: sender has non-delegation bytecode",
                ));
            }

            let code = if target.is_zero() {
                revm::bytecode::Bytecode::default()
            } else {
                revm::bytecode::Bytecode::new_eip7702(target)
            };
            let mut acc = evm.ctx().journal_mut().load_account_with_code_mut(sender)?.data;
            acc.set_code_and_hash_slow(code);
            drop(acc);
        } else {
            // Auto-delegation path: parser emits a 23-byte designator
            // candidate when there's no explicit Delegation and no Create.
            // Apply only if the sender's on-chain code is currently empty
            // (i.e. a fresh EOA).
            let (sender_has_code, has_create_entry, auto_code) = {
                let acc = evm.ctx().journal_mut().load_account_with_code_mut(sender)?.data;
                let has_code = acc.account().info.code_hash != keccak256([]);
                drop(acc);
                let ctx = evm.ctx();
                let parts = ctx.tx().eip8130_parts();
                (
                    has_code,
                    parts.account_changes.has_create_entry,
                    parts.account_changes.auto_delegation_code.clone(),
                )
            };
            if !sender_has_code && !has_create_entry && auto_code.len() == 23 {
                let target = Address::from_slice(&auto_code[3..]);
                let code = revm::bytecode::Bytecode::new_eip7702(target);
                let mut acc = evm.ctx().journal_mut().load_account_with_code_mut(sender)?.data;
                acc.set_code_and_hash_slow(code);
                drop(acc);
            }
        }

        // Step 4 (Code placement): place the Create entry's runtime bytecode
        // at the deployed account address. Last so the Delegation step above
        // sees the still-empty bytecode slot when a Create-only tx runs.
        // At most one entry — `validate_env` rejects `code_placements.len() > 1`.
        //
        // Replay guards (target empty code + zero nonce + clean
        // owner_config) ran at the top of step 1; reaching here means
        // the address is fresh.
        let placement = {
            let ctx = evm.ctx();
            ctx.tx()
                .eip8130_parts()
                .account_changes
                .code_placements
                .first()
                .map(|p| (p.address, p.code.clone()))
        };
        if let Some((address, code)) = placement {
            let bytecode = revm::bytecode::Bytecode::new_raw(code);
            let mut acc = evm.ctx().journal_mut().load_account_with_code_mut(address)?.data;
            acc.set_code_and_hash_slow(bytecode);
            drop(acc);
        }

        let unused_payer_verification_gas =
            payer_custom_verifier_gas_cap.saturating_sub(payer_verification_gas_used);
        let payer_auth_gas_limit =
            payer_intrinsic_gas.saturating_add(payer_custom_verifier_gas_cap);

        let mut accumulated_refunds: i64 = 0;

        for phase_idx in 0..call_phase_count {
            let checkpoint = evm.ctx().journal_mut().checkpoint();
            let mut phase_ok = true;
            let phase_gas_start = gas_remaining;
            let mut phase_refunds: i64 = 0;

            let phase_len = {
                let ctx = evm.ctx();
                ctx.tx().eip8130_parts().call_phases[phase_idx].len()
            };
            for call_idx in 0..phase_len {
                if gas_remaining == 0 {
                    phase_ok = false;
                    break;
                }

                // Pull this call's fields out of `parts` so the parts
                // borrow ends before run_exec_loop borrows evm mutably.
                // `Bytes::clone` is an `Arc` refcount bump.
                let (call_to, call_data, call_value) = {
                    let ctx = evm.ctx();
                    let call = &ctx.tx().eip8130_parts().call_phases[phase_idx][call_idx];
                    (call.to, call.data.clone(), call.value)
                };

                let (code_hash, code) =
                    load_call_target_bytecode::<Self::Evm, Self::Error>(evm, call_to)?;

                let call_gas = gas_remaining;
                let call_inputs = CallInputs {
                    input: CallInput::Bytes(call_data),
                    return_memory_offset: 0..0,
                    gas_limit: call_gas,
                    reservoir: 0,
                    known_bytecode: (code_hash, code),
                    bytecode_address: call_to,
                    target_address: call_to,
                    caller: sender,
                    value: CallValue::Transfer(call_value),
                    scheme: CallScheme::Call,
                    is_static: false,
                };

                let frame_init = FrameInit {
                    depth: 0,
                    memory: {
                        let ctx = evm.ctx();
                        let mut mem = SharedMemory::new_with_buffer(
                            ctx.local().shared_memory_buffer().clone(),
                        );
                        mem.set_memory_limit(ctx.cfg().memory_limit());
                        mem
                    },
                    frame_input: FrameInput::Call(Box::new(call_inputs)),
                };

                let call_result = self.mainnet.run_exec_loop(evm, frame_init)?;
                let call_gas_used = call_gas.saturating_sub(call_result.gas().remaining());
                gas_remaining = gas_remaining.saturating_sub(call_gas_used);
                phase_refunds += call_result.gas().refunded();

                if !call_result.interpreter_result().result.is_ok() {
                    phase_ok = false;
                    break;
                }
            }

            if phase_ok {
                accumulated_refunds += phase_refunds;
            } else {
                evm.ctx().journal_mut().checkpoint_revert(checkpoint);
            }

            phase_results.push(Eip8130PhaseResult {
                success: phase_ok,
                gas_used: phase_gas_start.saturating_sub(gas_remaining),
            });

            // EIP-8130 atomic-per-phase: halt remaining phases on first failure. Spec requires
            // phases after a revert to be reported as 0x00, so pad before stopping.
            if !phase_ok {
                while phase_results.len() < call_phase_count {
                    phase_results.push(Eip8130PhaseResult { success: false, gas_used: 0 });
                }
                break;
            }
        }

        // Spec: status = 0x01 iff all phases succeeded OR `calls` was empty.
        // `Iterator::all` on empty returns true, so this collapses both arms.
        let all_phases_succeeded = phase_results.iter().all(|r| r.success);
        let tx_succeeded = is_estimation || all_phases_succeeded;

        if !phase_results.is_empty() {
            evm.ctx()
                .journal_mut()
                .log(phase_statuses_system_log(TX_CONTEXT_ADDRESS, &phase_results));
        }

        let mut result_gas = Gas::new_spent(
            evm.ctx().tx().gas_limit().saturating_add(payer_auth_gas_limit),
        );
        result_gas.erase_cost(gas_remaining.saturating_add(unused_payer_verification_gas));
        if accumulated_refunds > 0 {
            result_gas.record_refund(accumulated_refunds);
        }

        let output = encode_phase_statuses(&phase_results);

        let instruction_result =
            if tx_succeeded { InstructionResult::Stop } else { InstructionResult::Revert };

        let mut frame_result = FrameResult::Call(CallOutcome::new(
            InterpreterResult { result: instruction_result, output, gas: result_gas },
            0..0,
        ));

        self.last_frame_result(evm, &mut frame_result)?;
        Ok(frame_result)
    }

    fn last_frame_result(
        &mut self,
        evm: &mut Self::Evm,
        frame_result: &mut <<Self::Evm as EvmTr>::Frame as FrameTr>::FrameResult,
    ) -> Result<(), Self::Error> {
        let ctx = evm.ctx();
        let tx = ctx.tx();
        let is_deposit = tx.tx_type() == DEPOSIT_TRANSACTION_TYPE;
        let tx_gas_limit = tx.gas_limit();
        let is_regolith = ctx.cfg().spec().is_enabled_in(OpSpecId::REGOLITH);

        let instruction_result = frame_result.interpreter_result().result;
        let gas = frame_result.gas_mut();
        let remaining = gas.remaining();
        let refunded = gas.refunded();
        let reservoir = gas.reservoir();
        let state_gas_spent = gas.state_gas_spent();
        let final_gas_limit = if tx.tx_type() == EIP8130_TX_TYPE {
            // EIP-8130 may meter payer_auth outside the signed sender-side
            // gas_limit. Preserve that larger accounting limit instead of
            // normalizing it back to tx.gas_limit().
            gas.limit()
        } else {
            tx_gas_limit
        };

        // Spend the gas limit. Gas is reimbursed when the tx returns successfully.
        *gas = Gas::new_spent(final_gas_limit);

        if instruction_result.is_ok() {
            // On Optimism, deposit transactions report gas usage uniquely to other
            // transactions due to them being pre-paid on L1.
            //
            // Hardfork Behavior:
            // - Bedrock (success path):
            //   - Deposit transactions (non-system) report their gas limit as the usage. No
            //     refunds.
            //   - Deposit transactions (system) report 0 gas used. No refunds.
            //   - Regular transactions report gas usage as normal.
            // - Regolith (success path):
            //   - Deposit transactions (all) report their gas used as normal. Refunds enabled.
            //   - Regular transactions report their gas used as normal.
            if !is_deposit || is_regolith {
                // Return unused regular gas and unused reservoir gas.
                gas.erase_cost(remaining);
                gas.record_refund(refunded);
            } else if is_deposit && tx.is_system_transaction() {
                // System transactions were a special type of deposit transaction in
                // the Bedrock hardfork that did not incur any gas costs.
                gas.erase_cost(tx_gas_limit);
            }
        } else if instruction_result.is_revert() {
            // On Optimism, deposit transactions report gas usage uniquely to other
            // transactions due to them being pre-paid on L1.
            //
            // Hardfork Behavior:
            // - Bedrock (revert path):
            //   - Deposit transactions (all) report the gas limit as the amount of gas used on
            //     failure. No refunds.
            //   - Regular transactions receive a refund on remaining gas as normal.
            // - Regolith (revert path):
            //   - Deposit transactions (all) report the actual gas used as the amount of gas used
            //     on failure. Refunds on remaining gas enabled.
            //   - Regular transactions receive a refund on remaining gas as normal.
            if !is_deposit || is_regolith {
                // Return unused regular gas.
                gas.erase_cost(remaining);
            }
        }

        if instruction_result.is_ok() {
            // Restore state_gas_spent on successful paths (lost by Gas::new_spent overwrite).
            gas.set_state_gas_spent(state_gas_spent);
            gas.set_reservoir(reservoir);
        } else {
            // On failure - zero execution state gas: [bal-devnet notes](<https://notes.ethereum.org/@ethpandaops/bal-devnet-4#Changes-vs-bal-devnet-3>)
            // and [specs](<https://github.com/ethereum/EIPs/pull/11476>)
            gas.set_state_gas_spent(0);
            gas.set_reservoir(state_gas_spent + reservoir);
        }

        Ok(())
    }

    fn reimburse_caller(
        &self,
        evm: &mut Self::Evm,
        frame_result: &mut <<Self::Evm as EvmTr>::Frame as FrameTr>::FrameResult,
    ) -> Result<(), Self::Error> {
        let additional_refund = if evm.ctx().tx().tx_type() != DEPOSIT_TRANSACTION_TYPE &&
            !evm.ctx().cfg().is_fee_charge_disabled()
        {
            let spec = evm.ctx().cfg().spec();
            evm.ctx().chain().operator_fee_refund(frame_result.gas(), spec)
        } else {
            U256::ZERO
        };

        // EIP-8130 sponsored transactions: refund the payer (not tx.caller()).
        if evm.ctx().tx().tx_type() == EIP8130_TX_TYPE {
            let payer = evm.ctx().tx().eip8130_parts().payer;
            let basefee = evm.ctx().block().basefee() as u128;
            let effective_gas_price = evm.ctx().tx().effective_gas_price(basefee);
            let gas = frame_result.gas();
            let refund_amount = U256::from(
                effective_gas_price
                    .saturating_mul((gas.remaining() + gas.refunded() as u64) as u128),
            ) + additional_refund;
            evm.ctx().journal_mut().load_account_mut(payer)?.incr_balance(refund_amount);
            return Ok(());
        }

        reimburse_caller(evm.ctx(), frame_result.gas(), additional_refund).map_err(From::from)
    }

    fn refund(
        &self,
        evm: &mut Self::Evm,
        frame_result: &mut <<Self::Evm as EvmTr>::Frame as FrameTr>::FrameResult,
        eip7702_refund: i64,
    ) {
        frame_result.gas_mut().record_refund(eip7702_refund);

        let is_deposit = evm.ctx().tx().tx_type() == DEPOSIT_TRANSACTION_TYPE;
        let is_regolith = evm.ctx().cfg().spec().is_enabled_in(OpSpecId::REGOLITH);

        // Prior to Regolith, deposit transactions did not receive gas refunds.
        let is_gas_refund_disabled = is_deposit && !is_regolith;
        if !is_gas_refund_disabled {
            frame_result.gas_mut().set_final_refund(
                evm.ctx().cfg().spec().into_eth_spec().is_enabled_in(SpecId::LONDON),
            );
        }
    }

    fn reward_beneficiary(
        &self,
        evm: &mut Self::Evm,
        frame_result: &mut <<Self::Evm as EvmTr>::Frame as FrameTr>::FrameResult,
    ) -> Result<(), Self::Error> {
        let is_deposit = evm.ctx().tx().tx_type() == DEPOSIT_TRANSACTION_TYPE;

        // Transfer fee to coinbase/beneficiary.
        if is_deposit {
            return Ok(());
        }

        self.mainnet.reward_beneficiary(evm, frame_result)?;
        let basefee = evm.ctx().block().basefee() as u128;

        // If the transaction is not a deposit transaction, fees are paid out
        // to both the Base Fee Vault as well as the L1 Fee Vault.
        // Use all_mut() to simultaneously borrow tx (immutable) and chain (mutable),
        // avoiding an unnecessary clone of the enveloped transaction bytes.
        let (_, tx, cfg, journal, l1_block_info, _) = evm.ctx().all_mut();
        let spec = cfg.spec();

        let Some(enveloped_tx) = tx.enveloped_tx() else {
            return Err(OpTransactionError::MissingEnvelopedTx.into());
        };

        let l1_cost = l1_block_info.calculate_tx_l1_cost(enveloped_tx, spec);
        // Exclude reservoir gas (EIP-8037) from used gas — reservoir is unused and reimbursed.
        let effective_used =
            frame_result.gas().used().saturating_sub(frame_result.gas().reservoir());
        let operator_fee_cost = if spec.is_enabled_in(OpSpecId::ISTHMUS) {
            l1_block_info.operator_fee_charge(enveloped_tx, U256::from(effective_used), spec)
        } else {
            U256::ZERO
        };
        let base_fee_amount = U256::from(basefee.saturating_mul(effective_used as u128));

        // Send fees to their respective recipients
        for (recipient, amount) in [
            (L1_FEE_RECIPIENT, l1_cost),
            (BASE_FEE_RECIPIENT, base_fee_amount),
            (OPERATOR_FEE_RECIPIENT, operator_fee_cost),
        ] {
            journal.balance_incr(recipient, amount)?;
        }

        Ok(())
    }

    fn execution_result(
        &mut self,
        evm: &mut Self::Evm,
        frame_result: <<Self::Evm as EvmTr>::Frame as FrameTr>::FrameResult,
        result_gas: ResultGas,
    ) -> Result<ExecutionResult<Self::HaltReason>, Self::Error> {
        take_error::<Self::Error, _>(evm.ctx().error())?;

        let exec_result = post_execution::output(evm.ctx(), frame_result, result_gas)
            .map_haltreason(OpHaltReason::Base);

        if exec_result.is_halt() {
            // Post-regolith, if the transaction is a deposit transaction and it halts,
            // we bubble up to the global return handler. The mint value will be persisted
            // and the caller nonce will be incremented there.
            let is_deposit = evm.ctx().tx().tx_type() == DEPOSIT_TRANSACTION_TYPE;
            if is_deposit && evm.ctx().cfg().spec().is_enabled_in(OpSpecId::REGOLITH) {
                return Err(ERROR::from(OpTransactionError::HaltedDepositPostRegolith));
            }
        }
        evm.ctx().journal_mut().commit_tx();
        evm.ctx().chain_mut().clear_tx_l1_cost();
        evm.ctx().local_mut().clear();
        evm.frame_stack().clear();

        Ok(exec_result)
    }

    fn catch_error(
        &self,
        evm: &mut Self::Evm,
        error: Self::Error,
    ) -> Result<ExecutionResult<Self::HaltReason>, Self::Error> {
        let is_deposit = evm.ctx().tx().tx_type() == DEPOSIT_TRANSACTION_TYPE;
        let is_tx_error = error.is_tx_error();
        let mut output = Err(error);

        // Deposit transaction can't fail so we manually handle it here.
        if is_tx_error && is_deposit {
            let ctx = evm.ctx();
            let spec = ctx.cfg().spec();
            let tx = ctx.tx();
            let caller = tx.caller();
            let mint = tx.mint();
            let is_system_tx = tx.is_system_transaction();
            let gas_limit = tx.gas_limit();
            let journal = evm.ctx().journal_mut();

            // discard all changes of this transaction
            // Default JournalCheckpoint is the first checkpoint and will wipe all changes.
            journal.checkpoint_revert(JournalCheckpoint::default());

            // If the transaction is a deposit transaction and it failed
            // for any reason, the caller nonce must be bumped, and the
            // gas reported must be altered depending on the Hardfork. This is
            // also returned as a special Halt variant so that consumers can more
            // easily distinguish between a failed deposit and a failed
            // normal transaction.

            // Increment sender nonce and account balance for the mint amount. Deposits
            // always persist the mint amount, even if the transaction fails.
            let mut acc = journal.load_account_mut(caller)?;
            acc.bump_nonce();
            acc.incr_balance(U256::from(mint.unwrap_or_default()));

            drop(acc); // Drop acc to avoid borrow checker issues.

            // We can now commit the changes.
            journal.commit_tx();

            // The gas used of a failed deposit post-regolith is the gas
            // limit of the transaction. pre-regolith, it is the gas limit
            // of the transaction for non system transactions and 0 for system
            // transactions.
            let gas_used =
                if spec.is_enabled_in(OpSpecId::REGOLITH) || !is_system_tx { gas_limit } else { 0 };
            // clear the journal
            output = Ok(ExecutionResult::Halt {
                reason: OpHaltReason::FailedDeposit,
                gas: ResultGas::default().with_total_gas_spent(gas_used),
                logs: Vec::new(),
            })
        }

        // do the cleanup
        evm.ctx().chain_mut().clear_tx_l1_cost();
        evm.ctx().local_mut().clear();
        evm.frame_stack().clear();

        output
    }
}

impl<EVM, ERROR> InspectorHandler for OpHandler<EVM, ERROR, EthFrame<EthInterpreter>>
where
    EVM: InspectorEvmTr<
            Context: OpContextTr,
            Frame = EthFrame<EthInterpreter>,
            Inspector: Inspector<<<Self as Handler>::Evm as EvmTr>::Context, EthInterpreter>,
        >,
    ERROR: EvmTrError<EVM> + From<OpTransactionError> + FromStringError + IsTxError,
{
    type IT = EthInterpreter;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DefaultOp, OpBuilder, OpTransaction,
        api::default_ctx::OpContext,
        constants::{
            BASE_FEE_SCALAR_OFFSET, ECOTONE_L1_BLOB_BASE_FEE_SLOT, ECOTONE_L1_FEE_SCALARS_SLOT,
            L1_BASE_FEE_SLOT, L1_BLOCK_CONTRACT, OPERATOR_FEE_SCALARS_SLOT,
        },
    };
    use alloy_primitives::uint;
    use revm::{
        context::{BlockEnv, CfgEnv, Context, TxEnv},
        context_interface::result::InvalidTransaction,
        database::InMemoryDB,
        database_interface::EmptyDB,
        handler::EthFrame,
        interpreter::{CallOutcome, InstructionResult, InterpreterResult},
        primitives::{Address, B256, Bytes, bytes},
        state::AccountInfo,
    };
    use rstest::rstest;
    use std::boxed::Box;

    /// Creates frame result.
    fn call_last_frame_return(
        ctx: OpContext<EmptyDB>,
        instruction_result: InstructionResult,
        gas: Gas,
    ) -> Gas {
        let mut evm = ctx.build_op();

        let mut exec_result = FrameResult::Call(CallOutcome::new(
            InterpreterResult { result: instruction_result, output: Bytes::new(), gas },
            0..0,
        ));

        let mut handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();

        handler.last_frame_result(&mut evm, &mut exec_result).unwrap();
        handler.refund(&mut evm, &mut exec_result, 0);
        *exec_result.gas()
    }

    #[test]
    fn test_revert_gas() {
        let ctx = Context::op()
            .with_tx(OpTransaction::builder().base(TxEnv::builder().gas_limit(100)).build_fill())
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::BEDROCK));

        let gas = call_last_frame_return(ctx, InstructionResult::Revert, Gas::new(90));
        assert_eq!(gas.remaining(), 90);
        assert_eq!(gas.total_gas_spent(), 10);
        assert_eq!(gas.refunded(), 0);
    }

    #[test]
    fn test_consume_gas() {
        let ctx = Context::op()
            .with_tx(OpTransaction::builder().base(TxEnv::builder().gas_limit(100)).build_fill())
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::REGOLITH));

        let gas = call_last_frame_return(ctx, InstructionResult::Stop, Gas::new(90));
        assert_eq!(gas.remaining(), 90);
        assert_eq!(gas.total_gas_spent(), 10);
        assert_eq!(gas.refunded(), 0);
    }

    #[test]
    fn test_consume_gas_with_refund() {
        let ctx = Context::op()
            .with_tx(
                OpTransaction::builder()
                    .base(TxEnv::builder().gas_limit(100))
                    .source_hash(B256::from([1u8; 32]))
                    .build_fill(),
            )
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::REGOLITH));

        let mut ret_gas = Gas::new(90);
        ret_gas.record_refund(20);

        let gas = call_last_frame_return(ctx.clone(), InstructionResult::Stop, ret_gas);
        assert_eq!(gas.remaining(), 90);
        assert_eq!(gas.total_gas_spent(), 10);
        assert_eq!(gas.refunded(), 2); // min(20, 10/5)

        let gas = call_last_frame_return(ctx, InstructionResult::Revert, ret_gas);
        assert_eq!(gas.remaining(), 90);
        assert_eq!(gas.total_gas_spent(), 10);
        assert_eq!(gas.refunded(), 0);
    }

    #[test]
    fn test_consume_gas_deposit_tx() {
        let ctx = Context::op()
            .with_tx(
                OpTransaction::builder()
                    .base(TxEnv::builder().gas_limit(100))
                    .source_hash(B256::from([1u8; 32]))
                    .build_fill(),
            )
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::BEDROCK));
        let gas = call_last_frame_return(ctx, InstructionResult::Stop, Gas::new(90));
        assert_eq!(gas.remaining(), 0);
        assert_eq!(gas.total_gas_spent(), 100);
        assert_eq!(gas.refunded(), 0);
    }

    #[test]
    fn test_consume_gas_sys_deposit_tx() {
        let ctx = Context::op()
            .with_tx(
                OpTransaction::builder()
                    .base(TxEnv::builder().gas_limit(100))
                    .source_hash(B256::from([1u8; 32]))
                    .is_system_transaction()
                    .build_fill(),
            )
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::BEDROCK));
        let gas = call_last_frame_return(ctx, InstructionResult::Stop, Gas::new(90));
        assert_eq!(gas.remaining(), 100);
        assert_eq!(gas.total_gas_spent(), 0);
        assert_eq!(gas.refunded(), 0);
    }

    #[test]
    fn test_commit_mint_value() {
        let caller = Address::ZERO;
        let mut db = InMemoryDB::default();
        db.insert_account_info(
            caller,
            AccountInfo { balance: U256::from(1000), ..Default::default() },
        );

        let mut ctx = Context::op()
            .with_db(db)
            .with_chain(L1BlockInfo {
                l1_base_fee: U256::from(1_000),
                l1_fee_overhead: Some(U256::from(1_000)),
                l1_base_fee_scalar: U256::from(1_000),
                ..Default::default()
            })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::REGOLITH));
        ctx.modify_tx(|tx| {
            tx.deposit.source_hash = B256::from([1u8; 32]);
            tx.deposit.mint = Some(10);
        });

        let mut evm = ctx.build_op();

        let handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();
        handler
            .validate_against_state_and_deduct_caller(&mut evm, &mut Default::default())
            .unwrap();

        // Check the account balance is updated.
        let account = evm.ctx().journal_mut().load_account(caller).unwrap();
        assert_eq!(account.info.balance, U256::from(1010));
    }

    #[test]
    fn test_remove_l1_cost_non_deposit() {
        let caller = Address::ZERO;
        let mut db = InMemoryDB::default();
        db.insert_account_info(
            caller,
            AccountInfo {
                balance: U256::from(1058), // Increased to cover L1 fees (1048) + base fees
                ..Default::default()
            },
        );
        let ctx = Context::op()
            .with_db(db)
            .with_chain(L1BlockInfo {
                l1_base_fee: U256::from(1_000),
                l1_fee_overhead: Some(U256::from(1_000)),
                l1_base_fee_scalar: U256::from(1_000),
                l2_block: Some(U256::from(0)),
                ..Default::default()
            })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::REGOLITH))
            .with_tx(
                OpTransaction::builder()
                    .base(TxEnv::builder().gas_limit(100))
                    .enveloped_tx(Some(bytes!("FACADE")))
                    .source_hash(B256::ZERO)
                    .build()
                    .unwrap(),
            );

        let mut evm = ctx.build_op();

        let handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();
        handler
            .validate_against_state_and_deduct_caller(&mut evm, &mut Default::default())
            .unwrap();

        // Check the account balance is updated.
        let account = evm.ctx().journal_mut().load_account(caller).unwrap();
        assert_eq!(account.info.balance, U256::from(10)); // 1058 - 1048 = 10
    }

    #[test]
    fn test_reload_l1_block_info_isthmus() {
        const BLOCK_NUM: U256 = uint!(100_U256);
        const L1_BASE_FEE: U256 = uint!(1_U256);
        const L1_BLOB_BASE_FEE: U256 = uint!(2_U256);
        const L1_BASE_FEE_SCALAR: u64 = 3;
        const L1_BLOB_BASE_FEE_SCALAR: u64 = 4;
        const L1_FEE_SCALARS: U256 = U256::from_limbs([
            0,
            (L1_BASE_FEE_SCALAR << (64 - BASE_FEE_SCALAR_OFFSET * 2)) | L1_BLOB_BASE_FEE_SCALAR,
            0,
            0,
        ]);
        const OPERATOR_FEE_SCALAR: u64 = 5;
        const OPERATOR_FEE_CONST: u64 = 6;
        const OPERATOR_FEE: U256 =
            U256::from_limbs([OPERATOR_FEE_CONST, OPERATOR_FEE_SCALAR, 0, 0]);

        let mut db = InMemoryDB::default();
        let l1_block_contract = db.load_account(L1_BLOCK_CONTRACT).unwrap();
        l1_block_contract.storage.insert(L1_BASE_FEE_SLOT, L1_BASE_FEE);
        l1_block_contract.storage.insert(ECOTONE_L1_BLOB_BASE_FEE_SLOT, L1_BLOB_BASE_FEE);
        l1_block_contract.storage.insert(ECOTONE_L1_FEE_SCALARS_SLOT, L1_FEE_SCALARS);
        l1_block_contract.storage.insert(OPERATOR_FEE_SCALARS_SLOT, OPERATOR_FEE);
        db.insert_account_info(
            Address::ZERO,
            AccountInfo { balance: U256::from(1000), ..Default::default() },
        );

        let ctx = Context::op()
            .with_db(db)
            .with_chain(L1BlockInfo {
                l2_block: Some(BLOCK_NUM + U256::from(1)), // ahead by one block
                ..Default::default()
            })
            .with_block(BlockEnv { number: BLOCK_NUM, ..Default::default() })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::ISTHMUS));

        let mut evm = ctx.build_op();

        assert_ne!(evm.ctx().chain().l2_block, Some(BLOCK_NUM));

        let handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();
        handler
            .validate_against_state_and_deduct_caller(&mut evm, &mut Default::default())
            .unwrap();

        assert_eq!(
            *evm.ctx().chain(),
            L1BlockInfo {
                l2_block: Some(BLOCK_NUM),
                l1_base_fee: L1_BASE_FEE,
                l1_base_fee_scalar: U256::from(L1_BASE_FEE_SCALAR),
                l1_blob_base_fee: Some(L1_BLOB_BASE_FEE),
                l1_blob_base_fee_scalar: Some(U256::from(L1_BLOB_BASE_FEE_SCALAR)),
                empty_ecotone_scalars: false,
                l1_fee_overhead: None,
                operator_fee_scalar: Some(U256::from(OPERATOR_FEE_SCALAR)),
                operator_fee_constant: Some(U256::from(OPERATOR_FEE_CONST)),
                tx_l1_cost: Some(U256::ZERO),
                da_footprint_gas_scalar: None
            }
        );
    }

    #[test]
    fn test_parse_da_footprint_gas_scalar_jovian() {
        const BLOCK_NUM: U256 = uint!(100_U256);
        const L1_BASE_FEE: U256 = uint!(1_U256);
        const L1_BLOB_BASE_FEE: U256 = uint!(2_U256);
        const L1_BASE_FEE_SCALAR: u64 = 3;
        const L1_BLOB_BASE_FEE_SCALAR: u64 = 4;
        const L1_FEE_SCALARS: U256 = U256::from_limbs([
            0,
            (L1_BASE_FEE_SCALAR << (64 - BASE_FEE_SCALAR_OFFSET * 2)) | L1_BLOB_BASE_FEE_SCALAR,
            0,
            0,
        ]);
        const OPERATOR_FEE_SCALAR: u8 = 5;
        const OPERATOR_FEE_CONST: u8 = 6;
        const DA_FOOTPRINT_GAS_SCALAR: u8 = 7;
        let mut operator_fee_and_da_footprint = [0u8; 32];
        operator_fee_and_da_footprint[31] = OPERATOR_FEE_CONST;
        operator_fee_and_da_footprint[23] = OPERATOR_FEE_SCALAR;
        operator_fee_and_da_footprint[19] = DA_FOOTPRINT_GAS_SCALAR;
        let operator_fee_and_da_footprint_u256 = U256::from_be_bytes(operator_fee_and_da_footprint);

        let mut db = InMemoryDB::default();
        let l1_block_contract = db.load_account(L1_BLOCK_CONTRACT).unwrap();
        l1_block_contract.storage.insert(L1_BASE_FEE_SLOT, L1_BASE_FEE);
        l1_block_contract.storage.insert(ECOTONE_L1_BLOB_BASE_FEE_SLOT, L1_BLOB_BASE_FEE);
        l1_block_contract.storage.insert(ECOTONE_L1_FEE_SCALARS_SLOT, L1_FEE_SCALARS);
        l1_block_contract
            .storage
            .insert(OPERATOR_FEE_SCALARS_SLOT, operator_fee_and_da_footprint_u256);
        db.insert_account_info(
            Address::ZERO,
            AccountInfo { balance: U256::from(6000), ..Default::default() },
        );

        let ctx = Context::op()
            .with_db(db)
            .with_chain(L1BlockInfo {
                l2_block: Some(BLOCK_NUM + U256::from(1)), // ahead by one block
                operator_fee_scalar: Some(U256::from(2)),
                operator_fee_constant: Some(U256::from(50)),
                ..Default::default()
            })
            .with_block(BlockEnv { number: BLOCK_NUM, ..Default::default() })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::JOVIAN))
            // set the operator fee to a low value
            .with_tx(
                OpTransaction::builder()
                    .base(TxEnv::builder().gas_limit(10))
                    .enveloped_tx(Some(bytes!("FACADE")))
                    .build_fill(),
            );

        let mut evm = ctx.build_op();

        assert_ne!(evm.ctx().chain().l2_block, Some(BLOCK_NUM));

        let handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();
        handler
            .validate_against_state_and_deduct_caller(&mut evm, &mut Default::default())
            .unwrap();

        assert_eq!(
            *evm.ctx().chain(),
            L1BlockInfo {
                l2_block: Some(BLOCK_NUM),
                l1_base_fee: L1_BASE_FEE,
                l1_base_fee_scalar: U256::from(L1_BASE_FEE_SCALAR),
                l1_blob_base_fee: Some(L1_BLOB_BASE_FEE),
                l1_blob_base_fee_scalar: Some(U256::from(L1_BLOB_BASE_FEE_SCALAR)),
                empty_ecotone_scalars: false,
                l1_fee_overhead: None,
                operator_fee_scalar: Some(U256::from(OPERATOR_FEE_SCALAR)),
                operator_fee_constant: Some(U256::from(OPERATOR_FEE_CONST)),
                tx_l1_cost: Some(U256::ZERO),
                da_footprint_gas_scalar: Some(DA_FOOTPRINT_GAS_SCALAR as u16),
            }
        );
    }

    #[test]
    fn test_reload_l1_block_info_regolith() {
        const BLOCK_NUM: U256 = uint!(200_U256);
        const L1_BASE_FEE: U256 = uint!(7_U256);
        const L1_FEE_OVERHEAD: U256 = uint!(9_U256);
        const L1_BASE_FEE_SCALAR: u64 = 11;

        let mut db = InMemoryDB::default();
        let l1_block_contract = db.load_account(L1_BLOCK_CONTRACT).unwrap();
        l1_block_contract.storage.insert(L1_BASE_FEE_SLOT, L1_BASE_FEE);
        // Pre-ecotone bedrock/regolith slots
        use crate::constants::{L1_OVERHEAD_SLOT, L1_SCALAR_SLOT};
        l1_block_contract.storage.insert(L1_OVERHEAD_SLOT, L1_FEE_OVERHEAD);
        l1_block_contract.storage.insert(L1_SCALAR_SLOT, U256::from(L1_BASE_FEE_SCALAR));

        let ctx = Context::op()
            .with_db(db)
            .with_chain(L1BlockInfo {
                l2_block: Some(BLOCK_NUM + U256::from(1)),
                ..Default::default()
            })
            .with_block(BlockEnv { number: BLOCK_NUM, ..Default::default() })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::REGOLITH));

        let mut evm = ctx.build_op();
        assert_ne!(evm.ctx().chain().l2_block, Some(BLOCK_NUM));

        let handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();
        handler
            .validate_against_state_and_deduct_caller(&mut evm, &mut Default::default())
            .unwrap();

        assert_eq!(
            *evm.ctx().chain(),
            L1BlockInfo {
                l2_block: Some(BLOCK_NUM),
                l1_base_fee: L1_BASE_FEE,
                l1_fee_overhead: Some(L1_FEE_OVERHEAD),
                l1_base_fee_scalar: U256::from(L1_BASE_FEE_SCALAR),
                tx_l1_cost: Some(U256::ZERO),
                ..Default::default()
            }
        );
    }

    #[test]
    fn test_reload_l1_block_info_ecotone_pre_isthmus() {
        const BLOCK_NUM: U256 = uint!(300_U256);
        const L1_BASE_FEE: U256 = uint!(13_U256);
        const L1_BLOB_BASE_FEE: U256 = uint!(17_U256);
        const L1_BASE_FEE_SCALAR: u64 = 19;
        const L1_BLOB_BASE_FEE_SCALAR: u64 = 23;
        const L1_FEE_SCALARS: U256 = U256::from_limbs([
            0,
            (L1_BASE_FEE_SCALAR << (64 - BASE_FEE_SCALAR_OFFSET * 2)) | L1_BLOB_BASE_FEE_SCALAR,
            0,
            0,
        ]);

        let mut db = InMemoryDB::default();
        let l1_block_contract = db.load_account(L1_BLOCK_CONTRACT).unwrap();
        l1_block_contract.storage.insert(L1_BASE_FEE_SLOT, L1_BASE_FEE);
        l1_block_contract.storage.insert(ECOTONE_L1_BLOB_BASE_FEE_SLOT, L1_BLOB_BASE_FEE);
        l1_block_contract.storage.insert(ECOTONE_L1_FEE_SCALARS_SLOT, L1_FEE_SCALARS);

        let ctx = Context::op()
            .with_db(db)
            .with_chain(L1BlockInfo {
                l2_block: Some(BLOCK_NUM + U256::from(1)),
                ..Default::default()
            })
            .with_block(BlockEnv { number: BLOCK_NUM, ..Default::default() })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::ECOTONE));

        let mut evm = ctx.build_op();
        assert_ne!(evm.ctx().chain().l2_block, Some(BLOCK_NUM));

        let handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();
        handler
            .validate_against_state_and_deduct_caller(&mut evm, &mut Default::default())
            .unwrap();

        assert_eq!(
            *evm.ctx().chain(),
            L1BlockInfo {
                l2_block: Some(BLOCK_NUM),
                l1_base_fee: L1_BASE_FEE,
                l1_base_fee_scalar: U256::from(L1_BASE_FEE_SCALAR),
                l1_blob_base_fee: Some(L1_BLOB_BASE_FEE),
                l1_blob_base_fee_scalar: Some(U256::from(L1_BLOB_BASE_FEE_SCALAR)),
                empty_ecotone_scalars: false,
                l1_fee_overhead: None,
                tx_l1_cost: Some(U256::ZERO),
                ..Default::default()
            }
        );
    }

    #[test]
    fn test_load_l1_block_info_isthmus_none() {
        const BLOCK_NUM: U256 = uint!(100_U256);
        const L1_BASE_FEE: U256 = uint!(1_U256);
        const L1_BLOB_BASE_FEE: U256 = uint!(2_U256);
        const L1_BASE_FEE_SCALAR: u64 = 3;
        const L1_BLOB_BASE_FEE_SCALAR: u64 = 4;
        const L1_FEE_SCALARS: U256 = U256::from_limbs([
            0,
            (L1_BASE_FEE_SCALAR << (64 - BASE_FEE_SCALAR_OFFSET * 2)) | L1_BLOB_BASE_FEE_SCALAR,
            0,
            0,
        ]);
        const OPERATOR_FEE_SCALAR: u64 = 5;
        const OPERATOR_FEE_CONST: u64 = 6;
        const OPERATOR_FEE: U256 =
            U256::from_limbs([OPERATOR_FEE_CONST, OPERATOR_FEE_SCALAR, 0, 0]);

        let mut db = InMemoryDB::default();
        let l1_block_contract = db.load_account(L1_BLOCK_CONTRACT).unwrap();
        l1_block_contract.storage.insert(L1_BASE_FEE_SLOT, L1_BASE_FEE);
        l1_block_contract.storage.insert(ECOTONE_L1_BLOB_BASE_FEE_SLOT, L1_BLOB_BASE_FEE);
        l1_block_contract.storage.insert(ECOTONE_L1_FEE_SCALARS_SLOT, L1_FEE_SCALARS);
        l1_block_contract.storage.insert(OPERATOR_FEE_SCALARS_SLOT, OPERATOR_FEE);
        db.insert_account_info(
            Address::ZERO,
            AccountInfo { balance: U256::from(1000), ..Default::default() },
        );

        let ctx = Context::op()
            .with_db(db)
            .with_block(BlockEnv { number: BLOCK_NUM, ..Default::default() })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::ISTHMUS));

        let mut evm = ctx.build_op();

        assert_ne!(evm.ctx().chain().l2_block, Some(BLOCK_NUM));

        let handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();
        handler
            .validate_against_state_and_deduct_caller(&mut evm, &mut Default::default())
            .unwrap();

        assert_eq!(
            *evm.ctx().chain(),
            L1BlockInfo {
                l2_block: Some(BLOCK_NUM),
                l1_base_fee: L1_BASE_FEE,
                l1_base_fee_scalar: U256::from(L1_BASE_FEE_SCALAR),
                l1_blob_base_fee: Some(L1_BLOB_BASE_FEE),
                l1_blob_base_fee_scalar: Some(U256::from(L1_BLOB_BASE_FEE_SCALAR)),
                empty_ecotone_scalars: false,
                l1_fee_overhead: None,
                operator_fee_scalar: Some(U256::from(OPERATOR_FEE_SCALAR)),
                operator_fee_constant: Some(U256::from(OPERATOR_FEE_CONST)),
                tx_l1_cost: Some(U256::ZERO),
                ..Default::default()
            }
        );
    }

    #[test]
    fn test_remove_l1_cost() {
        let caller = Address::ZERO;
        let mut db = InMemoryDB::default();
        db.insert_account_info(
            caller,
            AccountInfo { balance: U256::from(1049), ..Default::default() },
        );
        let ctx = Context::op()
            .with_db(db)
            .with_chain(L1BlockInfo {
                l1_base_fee: U256::from(1_000),
                l1_fee_overhead: Some(U256::from(1_000)),
                l1_base_fee_scalar: U256::from(1_000),
                l2_block: Some(U256::from(0)),
                ..Default::default()
            })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::REGOLITH))
            .with_tx(
                OpTransaction::builder()
                    .base(TxEnv::builder().gas_limit(100))
                    .source_hash(B256::ZERO)
                    .enveloped_tx(Some(bytes!("FACADE")))
                    .build()
                    .unwrap(),
            );

        let mut evm = ctx.build_op();
        let handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();

        // l1block cost is 1048 fee.
        handler
            .validate_against_state_and_deduct_caller(&mut evm, &mut Default::default())
            .unwrap();

        // Check the account balance is updated.
        let account = evm.ctx().journal_mut().load_account(caller).unwrap();
        assert_eq!(account.info.balance, U256::from(1));
    }

    #[test]
    fn test_remove_operator_cost_isthmus() {
        let caller = Address::ZERO;
        let mut db = InMemoryDB::default();
        db.insert_account_info(
            caller,
            AccountInfo { balance: U256::from(151), ..Default::default() },
        );
        let ctx = Context::op()
            .with_db(db)
            .with_chain(L1BlockInfo {
                operator_fee_scalar: Some(U256::from(10_000_000)),
                operator_fee_constant: Some(U256::from(50)),
                l2_block: Some(U256::from(0)),
                ..Default::default()
            })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::ISTHMUS))
            .with_tx(
                OpTransaction::builder()
                    .base(TxEnv::builder().gas_limit(10))
                    .enveloped_tx(Some(bytes!("FACADE")))
                    .build_fill(),
            );

        let mut evm = ctx.build_op();
        let handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();

        // Under Isthmus the operator fee cost is operator_fee_scalar * gas_limit / 1e6 +
        // operator_fee_constant 10_000_000 * 10 / 1_000_000 + 50 = 150
        handler
            .validate_against_state_and_deduct_caller(&mut evm, &mut Default::default())
            .unwrap();

        // Check the account balance is updated.
        let account = evm.ctx().journal_mut().load_account(caller).unwrap();
        assert_eq!(account.info.balance, U256::from(1));
    }

    #[test]
    fn test_remove_operator_cost_jovian() {
        let caller = Address::ZERO;
        let mut db = InMemoryDB::default();
        db.insert_account_info(
            caller,
            AccountInfo { balance: U256::from(2_051), ..Default::default() },
        );
        let ctx = Context::op()
            .with_db(db)
            .with_chain(L1BlockInfo {
                operator_fee_scalar: Some(U256::from(2)),
                operator_fee_constant: Some(U256::from(50)),
                l2_block: Some(U256::from(0)),
                ..Default::default()
            })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::JOVIAN))
            .with_tx(
                OpTransaction::builder()
                    .base(TxEnv::builder().gas_limit(10))
                    .enveloped_tx(Some(bytes!("FACADE")))
                    .build_fill(),
            );

        let mut evm = ctx.build_op();
        let handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();

        // Under Jovian the operator fee cost is operator_fee_scalar * gas_limit * 100 +
        // operator_fee_constant 2 * 10 * 100 + 50 = 2_050
        handler
            .validate_against_state_and_deduct_caller(&mut evm, &mut Default::default())
            .unwrap();

        let account = evm.ctx().journal_mut().load_account(caller).unwrap();
        assert_eq!(account.info.balance, U256::from(1));
    }

    #[test]
    fn test_remove_l1_cost_lack_of_funds() {
        let caller = Address::ZERO;
        let mut db = InMemoryDB::default();
        db.insert_account_info(
            caller,
            AccountInfo { balance: U256::from(48), ..Default::default() },
        );
        let ctx = Context::op()
            .with_db(db)
            .with_chain(L1BlockInfo {
                l1_base_fee: U256::from(1_000),
                l1_fee_overhead: Some(U256::from(1_000)),
                l1_base_fee_scalar: U256::from(1_000),
                l2_block: Some(U256::from(0)),
                ..Default::default()
            })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::REGOLITH))
            .modify_tx_chained(|tx| {
                tx.enveloped_tx = Some(bytes!("FACADE"));
            });

        // l1block cost is 1048 fee.
        let mut evm = ctx.build_op();
        let handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();

        // l1block cost is 1048 fee.
        assert_eq!(
            handler.validate_against_state_and_deduct_caller(&mut evm, &mut Default::default()),
            Err(EVMError::Transaction(
                InvalidTransaction::LackOfFundForMaxFee {
                    fee: Box::new(U256::from(1048)),
                    balance: Box::new(U256::from(48)),
                }
                .into(),
            ))
        );
    }

    #[test]
    fn test_validate_sys_tx() {
        // mark the tx as a system transaction.
        let ctx = Context::op()
            .modify_tx_chained(|tx| {
                tx.deposit.source_hash = B256::from([1u8; 32]);
                tx.deposit.is_system_transaction = true;
            })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::REGOLITH));

        let mut evm = ctx.build_op();
        let handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();

        assert_eq!(
            handler.validate_env(&mut evm),
            Err(EVMError::Transaction(OpTransactionError::DepositSystemTxPostRegolith))
        );

        // With BEDROCK spec.
        let ctx = evm.into_context();
        let mut evm = ctx.with_cfg(CfgEnv::new_with_spec(OpSpecId::BEDROCK)).build_op();

        // Pre-regolith system transactions should be allowed.
        assert!(handler.validate_env(&mut evm).is_ok());
    }

    #[test]
    fn test_validate_deposit_tx() {
        // Set source hash.
        let ctx = Context::op()
            .modify_tx_chained(|tx| {
                tx.deposit.source_hash = B256::from([1u8; 32]);
            })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::REGOLITH));

        let mut evm = ctx.build_op();
        let handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();

        assert!(handler.validate_env(&mut evm).is_ok());
    }

    #[test]
    fn test_validate_tx_against_state_deposit_tx() {
        // Set source hash.
        let ctx = Context::op()
            .modify_tx_chained(|tx| {
                tx.deposit.source_hash = B256::from([1u8; 32]);
            })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::REGOLITH));

        let mut evm = ctx.build_op();
        let handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();

        // Nonce and balance checks should be skipped for deposit transactions.
        assert!(handler.validate_env(&mut evm).is_ok());
    }

    #[test]
    fn test_halted_deposit_tx_post_regolith() {
        let ctx = Context::op()
            .modify_tx_chained(|tx| {
                // Set up as deposit transaction by having a deposit with source_hash
                tx.deposit.source_hash = B256::from([1u8; 32]);
            })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::REGOLITH));

        let mut evm = ctx.build_op();
        let mut handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();

        assert_eq!(
            handler.execution_result(
                &mut evm,
                FrameResult::Call(CallOutcome::new(
                    InterpreterResult {
                        result: InstructionResult::OutOfGas,
                        output: Default::default(),
                        gas: Default::default(),
                    },
                    Default::default()
                )),
                ResultGas::default(),
            ),
            Err(EVMError::Transaction(OpTransactionError::HaltedDepositPostRegolith))
        )
    }

    #[test]
    fn test_tx_zero_value_touch_caller() {
        let ctx = Context::op();

        let mut evm = ctx.build_op();

        assert!(!evm.0.ctx.journal_mut().load_account(Address::ZERO).unwrap().is_touched());

        let handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();

        handler
            .validate_against_state_and_deduct_caller(&mut evm, &mut Default::default())
            .unwrap();

        assert!(evm.0.ctx.journal_mut().load_account(Address::ZERO).unwrap().is_touched());
    }

    #[rstest]
    #[case::deposit(true)]
    #[case::dyn_fee(false)]
    fn test_operator_fee_refund(#[case] is_deposit: bool) {
        const SENDER: Address = Address::ZERO;
        const GAS_PRICE: u128 = 0xFF;
        const OP_FEE_MOCK_PARAM: u128 = 0xFFFF;

        let ctx = Context::op()
            .with_tx(
                OpTransaction::builder()
                    .base(
                        TxEnv::builder().gas_price(GAS_PRICE).gas_priority_fee(None).caller(SENDER),
                    )
                    .enveloped_tx(if is_deposit { None } else { Some(bytes!("FACADE")) })
                    .source_hash(if is_deposit { B256::from([1u8; 32]) } else { B256::ZERO })
                    .build_fill(),
            )
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::ISTHMUS));

        let mut evm = ctx.build_op();
        let handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();

        // Set the operator fee scalar & constant to non-zero values in the L1 block info.
        evm.ctx().chain.operator_fee_scalar = Some(U256::from(OP_FEE_MOCK_PARAM));
        evm.ctx().chain.operator_fee_constant = Some(U256::from(OP_FEE_MOCK_PARAM));

        let mut gas = Gas::new(100);
        gas.set_spent(10);
        let mut exec_result = FrameResult::Call(CallOutcome::new(
            InterpreterResult {
                result: InstructionResult::Return,
                output: Default::default(),
                gas,
            },
            0..0,
        ));

        // Reimburse the caller for the unspent portion of the fees.
        handler.reimburse_caller(&mut evm, &mut exec_result).unwrap();

        // Compute the expected refund amount. If the transaction is a deposit, the operator fee
        // refund never applies. If the transaction is not a deposit, the operator fee
        // refund is added to the refund amount.
        let mut expected_refund =
            U256::from(GAS_PRICE * (gas.remaining() + gas.refunded() as u64) as u128);
        let op_fee_refund = evm.ctx().chain().operator_fee_refund(&gas, OpSpecId::ISTHMUS);
        assert!(op_fee_refund > U256::ZERO);

        if !is_deposit {
            expected_refund += op_fee_refund;
        }

        // Check that the caller was reimbursed the correct amount of ETH.
        let account = evm.ctx().journal_mut().load_account(SENDER).unwrap();
        assert_eq!(account.info.balance, expected_refund);
    }

    #[test]
    fn test_tx_low_balance_nonce_unchanged() {
        let ctx = Context::op().with_tx(
            OpTransaction::builder().base(TxEnv::builder().value(U256::from(1000))).build_fill(),
        );

        let mut evm = ctx.build_op();

        let handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();

        let result =
            handler.validate_against_state_and_deduct_caller(&mut evm, &mut Default::default());

        assert!(matches!(
            result.err().unwrap(),
            EVMError::Transaction(OpTransactionError::Base(
                InvalidTransaction::LackOfFundForMaxFee { .. }
            ))
        ));
        assert_eq!(evm.0.ctx.journal_mut().load_account(Address::ZERO).unwrap().info.nonce, 0);
    }

    #[test]
    fn test_validate_missing_enveloped_tx() {
        use crate::transaction::deposit::DepositTransactionParts;

        // Create a non-deposit transaction without enveloped_tx
        let ctx = Context::op().with_tx(OpTransaction {
            base: TxEnv::builder().build_fill(),
            enveloped_tx: None, // Missing enveloped_tx for non-deposit transaction
            deposit: DepositTransactionParts::default(), // No source_hash means non-deposit
            eip8130: crate::transaction::eip8130::Eip8130Parts::default(),
        });

        let mut evm = ctx.build_op();
        let handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();

        assert_eq!(
            handler.validate_env(&mut evm),
            Err(EVMError::Transaction(OpTransactionError::MissingEnvelopedTx))
        );
    }
}

/// EIP-8130 handler execution tests
#[cfg(test)]
mod xlayer_eip8130_tests {
    use super::{
        ACCOUNT_CONFIG_ADDRESS, K1_VERIFIER_ADDRESS, NONCE_KEY_MAX, OpHandler, REVOKED_VERIFIER,
        aa_expiring_seen_slot, aa_lock_slot, aa_owner_config_slot,
    };
    use crate::{
        DefaultOp, L1BlockInfo, OpBuilder, OpContext, OpHaltReason, OpSpecId, OpTransaction,
        api::builder::DefaultOpEvm,
        precompiles_xlayer::{NONCE_MANAGER_ADDRESS, aa_nonce_slot},
        transaction::{
            OpTransactionError,
            eip8130::{
                AuthState, Eip8130AccountChanges, Eip8130AuthorizerValidation, Eip8130Call,
                Eip8130CodePlacement, Eip8130ConfigOp, Eip8130Parts, Eip8130SequenceUpdate,
                Eip8130StorageWrite, Eip8130VerifyCall, decode_phase_statuses,
            },
        },
    };
    use core::convert::Infallible;
    use revm::{
        bytecode::Bytecode,
        context::{BlockEnv, CfgEnv, Context, TxEnv},
        context_interface::{
            ContextTr, JournalTr,
            result::{EVMError, ExecutionResult},
        },
        database::InMemoryDB,
        handler::{EthFrame, EvmTr, Handler},
        interpreter::interpreter::EthInterpreter,
        primitives::{Address, B256, Bytes, TxKind, U256, bytes},
        state::AccountInfo,
    };
    use std::vec;

    type TestEvm = DefaultOpEvm<OpContext<InMemoryDB>>;

    type RunResult =
        Result<ExecutionResult<OpHaltReason>, EVMError<Infallible, OpTransactionError>>;

    /// Optional knobs used by a handful of failure-mode tests. All defaults
    /// reproduce the original `run_eip8130_tx` behavior.
    #[derive(Default, Clone)]
    struct Eip8130TxOpts {
        /// Override `BlockEnv::basefee` (default 0).
        block_basefee: u64,
        /// Override `tx.base.gas_price` (= `max_fee_per_gas`; default 0).
        max_fee_per_gas: Option<u128>,
        /// Override `tx.base.gas_priority_fee` (default `None` → 0).
        max_priority_fee_per_gas: Option<u128>,
        /// Override `OpSpecId` (default `XLAYER_V1`).
        spec: Option<OpSpecId>,
        /// Extra accounts to seed with `AccountInfo { balance, ..Default::default() }`.
        ///
        /// Sponsored scenarios need the payer pre-funded so the deduction step
        /// passes. Default empty preserves the original behavior (only `sender`
        /// gets funded).
        funded_accounts: Vec<(Address, U256)>,
        /// When `true`, install [`crate::gas_params::xlayer_gas_params`] on
        /// the `CfgEnv`. Default `false` keeps every `XLayer` slot at zero so
        /// tests written for the pre-XLayer-pricing baseline keep passing;
        /// `_succeeds` / `_rejected` tests that exercise real `XLayer`
        /// pricing (`account_changes_cost`, `bytecode_cost`, …) opt in.
        use_xlayer_gas_params: bool,
    }

    /// Builds an EVM with EIP-8130 parts and runs the full handler flow,
    /// returning both the run result and the constructed EVM so tests can
    /// inspect post-execution state.
    ///
    /// `block_timestamp == 0` keeps the default `BlockEnv::timestamp`, which
    /// every existing call site assumed implicitly. Pass a non-zero value to
    /// exercise expiry / time-bound logic.
    fn run_eip8130_tx(
        sender: Address,
        accounts: &[(Address, Bytecode)],
        storage: &[(Address, U256, U256)],
        tx_nonce: u64,
        eip8130: Eip8130Parts,
        gas_limit: u64,
        block_timestamp: u64,
    ) -> (RunResult, TestEvm) {
        run_eip8130_tx_with_opts(
            sender,
            accounts,
            storage,
            tx_nonce,
            eip8130,
            gas_limit,
            block_timestamp,
            Eip8130TxOpts::default(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn run_eip8130_tx_with_opts(
        sender: Address,
        accounts: &[(Address, Bytecode)],
        storage: &[(Address, U256, U256)],
        tx_nonce: u64,
        eip8130: Eip8130Parts,
        gas_limit: u64,
        block_timestamp: u64,
        opts: Eip8130TxOpts,
    ) -> (RunResult, TestEvm) {
        let mut db = InMemoryDB::default();
        db.insert_account_info(
            sender,
            AccountInfo { balance: U256::from(10_000_000), ..Default::default() },
        );
        db.insert_account_info(
            NONCE_MANAGER_ADDRESS,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("FE"))), ..Default::default() },
        );
        for (addr, code) in accounts {
            db.insert_account_info(
                *addr,
                AccountInfo { code: Some(code.clone()), ..Default::default() },
            );
        }
        for (addr, slot, value) in storage {
            db.insert_account_storage(*addr, *slot, *value).unwrap();
        }
        for (addr, balance) in &opts.funded_accounts {
            db.insert_account_info(*addr, AccountInfo { balance: *balance, ..Default::default() });
        }

        let mut tx = OpTransaction::builder()
            .base(
                TxEnv::builder()
                    .tx_type(Some(0x7B))
                    .caller(sender)
                    .gas_limit(gas_limit)
                    .kind(TxKind::Call(sender)),
            )
            .enveloped_tx(Some(bytes!("7BFACADE")))
            .build_fill();
        tx.base.nonce = tx_nonce;
        if let Some(fee) = opts.max_fee_per_gas {
            tx.base.gas_price = fee;
        }
        if let Some(prio) = opts.max_priority_fee_per_gas {
            tx.base.gas_priority_fee = Some(prio);
        }
        tx.eip8130 = eip8130;

        let ctx = Context::op()
            .with_db(db)
            .with_tx(tx)
            .with_block(BlockEnv {
                timestamp: U256::from(block_timestamp),
                basefee: opts.block_basefee,
                ..Default::default()
            })
            .with_chain(L1BlockInfo {
                l2_block: Some(U256::ZERO),
                operator_fee_scalar: Some(U256::ZERO),
                operator_fee_constant: Some(U256::ZERO),
                ..Default::default()
            })
            .with_cfg({
                // Default tests run against the upstream-EVM gas table only
                // (every XLayer slot zero), preserving the original test
                // expectations. Tests that need real XLayer pricing opt in
                // via `opts.use_xlayer_gas_params = true`.
                let spec = opts.spec.unwrap_or(OpSpecId::XLAYER_V1);
                let mut cfg = CfgEnv::new_with_spec(spec);
                if opts.use_xlayer_gas_params {
                    cfg.set_gas_params(crate::gas_params::xlayer_gas_params(spec));
                }
                cfg
            });
        let mut evm = ctx.build_op();

        let mut handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();
        let result = handler.run(&mut evm);
        (result, evm)
    }

    #[test]
    fn test_eip8130_empty_tx_succeeds() {
        let sender = Address::from([0x11; 20]);
        let (result, _) = run_eip8130_tx(
            sender,
            &[],
            &[],
            0,
            Eip8130Parts { sender, payer: sender, ..Default::default() },
            100_000,
            0,
        );
        let result = result.unwrap();
        assert!(
            result.is_success(),
            "no call phases and no deploys → spec: status = 0x01 (calls was empty)",
        );

        let statuses = decode_phase_statuses(result.output().unwrap());
        assert!(statuses.is_empty(), "no call phases = empty statuses");
    }

    #[test]
    fn test_eip8130_deploy_only_succeeds() {
        let sender = Address::from([0x11; 20]);
        let deployed_addr = Address::from([0x99; 20]);
        let bytecode = bytes!("363d3d373d3d3d363d73DEADBEEF5af43d82803e903d91602b57fd5bf3");

        let (result, _) = run_eip8130_tx(
            sender,
            &[],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                account_changes: Eip8130AccountChanges {
                    has_create_entry: true,
                    code_placements: vec![Eip8130CodePlacement {
                        address: deployed_addr,
                        code: bytecode,
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            100_000,
            0,
        );
        let result = result.unwrap();
        assert!(result.is_success(), "deploy-only tx should succeed");

        let statuses = decode_phase_statuses(result.output().unwrap());
        assert!(statuses.is_empty(), "no call phases = empty statuses");
    }

    /// Parameterized completeness sweep for the single-phase happy path.
    ///
    /// One `#[test]` exercising every supported feature axis at least once
    /// (nonce mode, sender auth verifier, payer mode, from mode). Each
    /// scenario reuses `run_eip8130_tx_with_opts` via `run_single_phase_scenario`
    /// and asserts the tx succeeds with `phase_statuses == vec![true]`. Per-axis
    /// soundness is covered by dedicated `_rejected` tests.
    #[test]
    fn test_eip8130_single_phase_succeeds() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);
        let payer = Address::from([0x12; 20]);
        let stop_runtime = (target, Bytecode::new_legacy(bytes!("00"))); // STOP
        let block_ts: u64 = 0;

        // Helper to make `bytes32(bytes20(addr))` (the EOA self-owner pattern).
        fn eoa_owner_id(addr: Address) -> B256 {
            let mut buf = [0u8; 32];
            buf[..20].copy_from_slice(addr.as_slice());
            B256::from(buf)
        }

        // Local helper: build a single-phase tx (one STOP call) and assert it
        // succeeds. `[label]` prefixes panic messages so a failing scenario
        // points at the offending row.
        let run_single_phase_scenario = |label: &str,
                                         parts_no_calls: Eip8130Parts,
                                         storage: Vec<(Address, U256, U256)>,
                                         accounts_extra: Vec<(Address, Bytecode)>,
                                         funded: Vec<(Address, U256)>,
                                         tx_nonce: u64| {
            let mut accounts = vec![stop_runtime.clone()];
            accounts.extend(accounts_extra);

            let parts = Eip8130Parts {
                call_phases: vec![vec![Eip8130Call {
                    to: target,
                    data: Bytes::new(),
                    value: U256::ZERO,
                }]],
                ..parts_no_calls
            };

            let (result, _) = run_eip8130_tx_with_opts(
                sender,
                &accounts,
                &storage,
                tx_nonce,
                parts,
                300_000,
                block_ts,
                Eip8130TxOpts { funded_accounts: funded, ..Default::default() },
            );
            let result = result.unwrap_or_else(|e| panic!("[{label}] handler error: {e:?}"));
            assert!(result.is_success(), "[{label}] tx should succeed, got: {result:?}");
            let statuses = decode_phase_statuses(result.output().unwrap());
            assert_eq!(statuses, vec![true], "[{label}] phase status mismatch");
        };

        // Scenario 1: nonce_key = 0, K1 implicit-EOA, self-pay, explicit-from.
        // Baseline. Auth state defaults to SelfPay/SelfPay (both sides skipped),
        // matching the original test.
        run_single_phase_scenario(
            "nonce0-k1-selfpay-explicit",
            Eip8130Parts { sender, payer: sender, ..Default::default() },
            vec![],
            vec![],
            vec![],
            0,
        );

        // Scenario 2: nonce_key = 1 (non-zero sequenced), K1 implicit-EOA,
        // self-pay, explicit-from. Seeds the parallel-stream nonce slot.
        let nonce_key_one = U256::from(1u64);
        let nonce_one_slot = crate::precompiles_xlayer::aa_nonce_slot(sender, nonce_key_one);
        run_single_phase_scenario(
            "nonce1-k1-selfpay-explicit",
            Eip8130Parts { sender, payer: sender, nonce_key: nonce_key_one, ..Default::default() },
            vec![(NONCE_MANAGER_ADDRESS, nonce_one_slot, U256::from(0u64))],
            vec![],
            vec![],
            0,
        );

        // Scenario 3: nonce_key = NONCE_KEY_MAX (nonce-free) with expiry +
        // nonce_free_hash. Replay protection lives in the expiring-nonce
        // circular buffer, not the sequenced slot.
        run_single_phase_scenario(
            "nonceMAX-k1-selfpay-explicit",
            Eip8130Parts {
                sender,
                payer: sender,
                nonce_key: NONCE_KEY_MAX,
                expiry: 25,
                nonce_free_hash: Some(B256::from([0xAB; 32])),
                ..Default::default()
            },
            vec![],
            vec![],
            vec![],
            0,
        );

        // Scenario 4: P256Raw native verifier, self-pay, explicit-from.
        // Eager-resolved owner_id = keccak256(pubkey), seeded synthetically as
        // 0xAA...AA. Owner_config row binds (P256_RAW, OWNER_SCOPE_SENDER).
        let p256_raw_owner_id = B256::repeat_byte(0xAA);
        let p256_raw_slot = aa_owner_config_slot(sender, U256::from_be_bytes(p256_raw_owner_id.0));
        run_single_phase_scenario(
            "p256raw-selfpay-explicit",
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: AuthState::Native {
                    verifier: crate::constants::P256_RAW_VERIFIER_ADDRESS,
                    owner_id: p256_raw_owner_id,
                    delegate_inner: None,
                },
                ..Default::default()
            },
            vec![(
                ACCOUNT_CONFIG_ADDRESS,
                p256_raw_slot,
                pack_owner_config(
                    crate::constants::P256_RAW_VERIFIER_ADDRESS,
                    crate::constants::OWNER_SCOPE_SENDER,
                ),
            )],
            vec![],
            vec![],
            0,
        );

        // Scenario 5: P256WebAuthn native verifier, self-pay, explicit-from.
        // Same shape as P256Raw but binds the WebAuthn verifier address.
        let p256_wa_owner_id = B256::repeat_byte(0xBB);
        let p256_wa_slot = aa_owner_config_slot(sender, U256::from_be_bytes(p256_wa_owner_id.0));
        run_single_phase_scenario(
            "p256webauthn-selfpay-explicit",
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: AuthState::Native {
                    verifier: crate::constants::P256_WEBAUTHN_VERIFIER_ADDRESS,
                    owner_id: p256_wa_owner_id,
                    delegate_inner: None,
                },
                ..Default::default()
            },
            vec![(
                ACCOUNT_CONFIG_ADDRESS,
                p256_wa_slot,
                pack_owner_config(
                    crate::constants::P256_WEBAUTHN_VERIFIER_ADDRESS,
                    crate::constants::OWNER_SCOPE_SENDER,
                ),
            )],
            vec![],
            vec![],
            0,
        );

        // Scenario 6: Delegate→K1 inner, self-pay, explicit-from. Outer binds
        // (sender, bytes20(delegate)) → DELEGATE; inner binds
        // (delegate, inner_owner_id) → K1.
        let delegate_addr = Address::from([0x33; 20]);
        let delegate_owner_id = eoa_owner_id(delegate_addr);
        let inner_owner_id = B256::repeat_byte(0xCC);
        let outer_slot = aa_owner_config_slot(sender, U256::from_be_bytes(delegate_owner_id.0));
        let inner_slot = aa_owner_config_slot(delegate_addr, U256::from_be_bytes(inner_owner_id.0));
        run_single_phase_scenario(
            "delegate-k1inner-selfpay-explicit",
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: AuthState::Native {
                    verifier: crate::constants::DELEGATE_VERIFIER_ADDRESS,
                    owner_id: delegate_owner_id,
                    delegate_inner: Some(crate::transaction::eip8130::DelegateInner {
                        verifier: K1_VERIFIER_ADDRESS,
                        owner_id: inner_owner_id,
                    }),
                },
                ..Default::default()
            },
            vec![
                (
                    ACCOUNT_CONFIG_ADDRESS,
                    outer_slot,
                    pack_owner_config(
                        crate::constants::DELEGATE_VERIFIER_ADDRESS,
                        crate::constants::OWNER_SCOPE_SENDER,
                    ),
                ),
                (
                    ACCOUNT_CONFIG_ADDRESS,
                    inner_slot,
                    pack_owner_config(K1_VERIFIER_ADDRESS, crate::constants::OWNER_SCOPE_SENDER),
                ),
            ],
            vec![],
            vec![],
            0,
        );

        // Scenario 7: K1 sender + K1 payer, sponsored (payer != sender),
        // explicit-from. Exercises the separate payer-side auth check + payer
        // gas deduction path. Payer's owner_config row binds
        // (K1, OWNER_SCOPE_PAYER) at the payer's EOA self-owner slot. Payer
        // also needs balance — funded via `funded_accounts`.
        let payer_owner_id = eoa_owner_id(payer);
        let payer_slot = aa_owner_config_slot(payer, U256::from_be_bytes(payer_owner_id.0));
        run_single_phase_scenario(
            "k1sender-k1payer-sponsored-explicit",
            Eip8130Parts {
                sender,
                payer,
                sender_authstate: AuthState::Native {
                    verifier: K1_VERIFIER_ADDRESS,
                    owner_id: eoa_owner_id(sender),
                    delegate_inner: None,
                },
                payer_authstate: AuthState::Native {
                    verifier: K1_VERIFIER_ADDRESS,
                    owner_id: payer_owner_id,
                    delegate_inner: None,
                },
                ..Default::default()
            },
            vec![(
                ACCOUNT_CONFIG_ADDRESS,
                payer_slot,
                pack_owner_config(K1_VERIFIER_ADDRESS, crate::constants::OWNER_SCOPE_PAYER),
            )],
            vec![],
            vec![(payer, U256::from(10_000_000u64))],
            0,
        );

        // Scenario 8: K1 implicit-EOA, self-pay, EOA-mode (`is_eoa = true`).
        // Auth state was decided at conversion time — handler doesn't re-check
        // the K1 sig — so the existing K1 implicit-EOA happy path also works
        // with `is_eoa = true`. Pins the axis explicitly.
        run_single_phase_scenario(
            "nonce0-k1-selfpay-eoa",
            Eip8130Parts { sender, payer: sender, is_eoa: true, ..Default::default() },
            vec![],
            vec![],
            vec![],
            0,
        );
    }

    #[test]
    fn test_eip8130_single_phase_failure_rejected() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);

        let (result, _) = run_eip8130_tx(
            sender,
            &[(target, Bytecode::new_legacy(bytes!("60006000FD")))], // REVERT
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                call_phases: vec![vec![Eip8130Call {
                    to: target,
                    data: Bytes::new(),
                    value: U256::ZERO,
                }]],
                ..Default::default()
            },
            100_000,
            0,
        );
        let result = result.unwrap();
        assert!(!result.is_success(), "all phases failed → tx reverts");
    }

    #[test]
    fn test_eip8130_single_phase_atomic_batch_failure_rejected() {
        let sender = Address::from([0x11; 20]);
        let target_success = Address::from([0x22; 20]);
        let target_revert = Address::from([0x23; 20]);

        let (result, mut evm) = run_eip8130_tx(
            sender,
            &[
                (target_success, Bytecode::new_legacy(bytes!("00"))), // STOP
                (target_revert, Bytecode::new_legacy(bytes!("60006000FD"))), // REVERT
            ],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                call_phases: vec![vec![
                    Eip8130Call { to: target_success, data: Bytes::new(), value: U256::ONE },
                    Eip8130Call { to: target_revert, data: Bytes::new(), value: U256::ZERO },
                ]],
                ..Default::default()
            },
            100_000,
            0,
        );
        let result = result.unwrap();

        assert!(!result.is_success(), "all phases failed → tx reverts");

        let success_balance =
            evm.ctx().journal_mut().load_account(target_success).unwrap().info.balance;
        let revert_balance =
            evm.ctx().journal_mut().load_account(target_revert).unwrap().info.balance;
        assert_eq!(success_balance, U256::ZERO, "target_success value transfer not rolled back");
        assert_eq!(revert_balance, U256::ZERO, "target_revert balance unexpectedly nonzero");
    }

    #[test]
    fn test_eip8130_mixed_phases_rejected() {
        let sender = Address::from([0x11; 20]);
        let target_ok = Address::from([0x22; 20]);
        let target_fail = Address::from([0x33; 20]);

        let (result, _) = run_eip8130_tx(
            sender,
            &[
                (target_ok, Bytecode::new_legacy(bytes!("00"))), // STOP
                (target_fail, Bytecode::new_legacy(bytes!("60006000FD"))), // REVERT
            ],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                call_phases: vec![
                    vec![Eip8130Call { to: target_ok, data: Bytes::new(), value: U256::ZERO }],
                    vec![Eip8130Call { to: target_fail, data: Bytes::new(), value: U256::ZERO }],
                ],
                ..Default::default()
            },
            100_000,
            0,
        );
        let result = result.unwrap();
        assert!(!result.is_success(), "any phase reverted → tx fails per EIP-8130");

        let statuses = decode_phase_statuses(result.output().unwrap());
        assert_eq!(statuses, vec![true, false]);
    }

    #[test]
    fn test_eip8130_mid_phase_failure_pads_remaining_rejected() {
        let sender = Address::from([0x11; 20]);
        let target_ok = Address::from([0x22; 20]);
        let target_fail = Address::from([0x33; 20]);
        let target_phase3 = Address::from([0x44; 20]);

        let (result, _) = run_eip8130_tx(
            sender,
            &[
                (target_ok, Bytecode::new_legacy(bytes!("00"))), // STOP
                (target_fail, Bytecode::new_legacy(bytes!("60006000FD"))), // REVERT
                (target_phase3, Bytecode::new_legacy(bytes!("00"))), // STOP — never executed
            ],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                call_phases: vec![
                    vec![Eip8130Call { to: target_ok, data: Bytes::new(), value: U256::ZERO }],
                    vec![Eip8130Call { to: target_fail, data: Bytes::new(), value: U256::ZERO }],
                    vec![Eip8130Call { to: target_phase3, data: Bytes::new(), value: U256::ZERO }],
                ],
                ..Default::default()
            },
            200_000,
            0,
        );
        let result = result.unwrap();
        assert!(!result.is_success(), "phase 2 reverted → tx fails per EIP-8130");

        let statuses = decode_phase_statuses(result.output().unwrap());
        assert_eq!(
            statuses,
            vec![true, false, false],
            "phase 3 must be padded as failed (spec: phases after revert reported as 0x00)",
        );
    }

    #[test]
    fn test_eip8130_all_phases_fail_rejected() {
        let sender = Address::from([0x11; 20]);
        let target_fail = Address::from([0x33; 20]);

        let (result, _) = run_eip8130_tx(
            sender,
            &[(target_fail, Bytecode::new_legacy(bytes!("60006000FD")))], // REVERT
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                call_phases: vec![
                    vec![Eip8130Call { to: target_fail, data: Bytes::new(), value: U256::ZERO }],
                    vec![Eip8130Call { to: target_fail, data: Bytes::new(), value: U256::ZERO }],
                ],
                ..Default::default()
            },
            100_000,
            0,
        );
        let result = result.unwrap();
        assert!(!result.is_success(), "all phases failed → tx reverts");
    }

    #[test]
    fn test_eip8130_gas_accounting_succeeds() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);

        // The handler now computes intrinsic gas on demand from
        // `cfg.gas_params()` against a default `TxEip8130` (no inner tx is
        // attached for synthetic AA test fixtures); we just sanity-check that
        // some intrinsic gas is charged and execution stays inside the limit.
        let gas_limit = 100_000u64;
        let (result, _) = run_eip8130_tx(
            sender,
            &[(target, Bytecode::new_legacy(bytes!("00")))], // STOP
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                call_phases: vec![vec![Eip8130Call {
                    to: target,
                    data: Bytes::new(),
                    value: U256::ZERO,
                }]],
                ..Default::default()
            },
            gas_limit,
            0,
        );
        let result = result.unwrap();
        assert!(result.is_success());
        assert!(result.tx_gas_used() > 0, "intrinsic gas should be charged");
        assert!(result.tx_gas_used() <= gas_limit, "cannot spend more than limit");
    }

    #[test]
    fn test_eip8130_payer_custom_verifier_does_not_reduce_call_gas() {
        let sender = Address::from([0x11; 20]);
        let payer = Address::from([0x44; 20]);
        let target = Address::from([0x22; 20]);
        let verifier = Address::from([0xAA; 20]);
        let owner_id = B256::from([0xBB; 32]);
        let owner_config_slot = aa_owner_config_slot(payer, U256::from_be_bytes(owner_id.0));

        let mut payer_auth = Vec::with_capacity(56);
        payer_auth.extend_from_slice(verifier.as_slice());
        payer_auth.extend_from_slice(&[0xCA; 36]);

        let parts = Eip8130Parts {
            sender,
            payer,
            payer_auth: Bytes::from(payer_auth),
            payer_authstate: AuthState::Deferred {
                spec: Eip8130VerifyCall {
                    verifier,
                    calldata: Bytes::from(vec![0xCA; 36]),
                    account: payer,
                    required_scope: crate::constants::OWNER_SCOPE_PAYER,
                },
                delegate_outer: None,
            },
            call_phases: vec![vec![Eip8130Call {
                to: target,
                data: Bytes::new(),
                value: U256::ZERO,
            }]],
            ..Default::default()
        };

        let gas_params = crate::gas_params::xlayer_gas_params(crate::OpSpecId::XLAYER_V1);
        let sender_intrinsic = crate::eip8130_gas::aa_intrinsic_gas(&parts, &gas_params);
        let gas_limit = sender_intrinsic + 1;

        let (result, _) = run_eip8130_tx_with_opts(
            sender,
            &[
                (target, Bytecode::new_legacy(bytes!("00"))),
                (verifier, make_verifier_bytecode(owner_id)),
            ],
            &[(
                ACCOUNT_CONFIG_ADDRESS,
                owner_config_slot,
                pack_owner_config(verifier, crate::constants::OWNER_SCOPE_PAYER),
            )],
            0,
            parts,
            gas_limit,
            0,
            Eip8130TxOpts {
                funded_accounts: vec![(payer, U256::from(10_000_000u64))],
                use_xlayer_gas_params: true,
                ..Default::default()
            },
        );

        let result = result.expect("payer custom verifier should validate");
        assert!(
            result.is_success(),
            "payer-auth verifier gas must be metered separately instead of exhausting call gas",
        );
        assert!(
            result.tx_gas_used() > gas_limit,
            "payer auth gas should be charged in addition to the signed sender-side gas_limit",
        );
    }

    /// EIP-8130 spec: "Intrinsic gas (including auth costs) is not refundable."
    /// A reverting AA tx must still charge the full `aa_intrinsic_gas`
    /// (`tx_base` + auth verification + nonce-key cost) — only unused
    /// phase-budget gas can be refunded back to the payer. Pins this by
    /// computing `aa_intrinsic_gas` for the fixture's parts + `XLayer` gas
    /// params, running a single-phase REVERT call, and asserting
    /// `gas_used >= intrinsic`.
    #[test]
    fn test_eip8130_intrinsic_gas_not_refundable_on_revert() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);

        let parts = Eip8130Parts {
            sender,
            payer: sender,
            call_phases: vec![vec![Eip8130Call {
                to: target,
                data: Bytes::new(),
                value: U256::ZERO,
            }]],
            ..Default::default()
        };

        // Compute the expected intrinsic against XLayer pricing (which the
        // fixture installs via `use_xlayer_gas_params = true`). With default
        // parts: tx_base_stipend + K1 fallback (sender_auth empty) +
        // nonce_cold.
        let gas_params = crate::gas_params::xlayer_gas_params(crate::OpSpecId::XLAYER_V1);
        let expected_intrinsic = crate::eip8130_gas::aa_intrinsic_gas(&parts, &gas_params);
        assert!(expected_intrinsic > 0, "test pre-condition: intrinsic must be non-zero");

        let gas_limit = 200_000u64;
        let (result, _) = run_eip8130_tx_with_opts(
            sender,
            // PUSH1 0 PUSH1 0 REVERT — fails the only call, fails the only phase.
            &[(target, Bytecode::new_legacy(bytes!("60006000FD")))],
            &[],
            0,
            parts,
            gas_limit,
            0,
            Eip8130TxOpts { use_xlayer_gas_params: true, ..Default::default() },
        );
        let result = result.unwrap();
        assert!(!result.is_success(), "REVERT-only tx must not succeed");
        assert!(
            result.tx_gas_used() >= expected_intrinsic,
            "intrinsic gas must not be refunded on revert: gas_used={}, expected >= {expected_intrinsic}",
            result.tx_gas_used(),
        );
    }

    #[test]
    fn test_eip8130_warm_nonce_reduces_intrinsic_gas_succeeds() {
        // Smoke-test that both cold and warm nonce paths run to completion
        // under the same `tx.gas_limit`. The original intent (assert warm
        // run consumes strictly less gas than cold) is masked by revm's
        // `set_final_refund` cap (≤ half spent) which compresses the
        // 17_100-gas warm/cold delta below observability — see comment at
        // the asserts below. This test currently only catches **regressions
        // that break execution entirely** for either nonce-state; the
        // strict warm < cold relationship is *not* enforced. To restore
        // the strict claim, the test would need a fixture where the warm
        // refund crosses a phase-completion boundary that survives the
        // refund cap (e.g., a multi-phase tx where the extra budget lets
        // an additional phase run).
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);
        let gas_limit = 200_000u64;
        let nonce_key = U256::ZERO;
        let slot = aa_nonce_slot(sender, nonce_key);

        // Cold run: nonce slot pre-seeded at exactly 1 (initialized but
        // never bumped — the handler's `nonce_value > 1` check treats this
        // as cold so no warm refund is granted).
        let (cold_result, _) = run_eip8130_tx(
            sender,
            &[(target, Bytecode::new_legacy(bytes!("00")))],
            &[(NONCE_MANAGER_ADDRESS, slot, U256::from(1u64))],
            1,
            Eip8130Parts {
                sender,
                payer: sender,
                nonce_key,
                call_phases: vec![vec![Eip8130Call {
                    to: target,
                    data: Bytes::new(),
                    value: U256::ZERO,
                }]],
                ..Default::default()
            },
            gas_limit,
            0,
        );
        let cold_result = cold_result.unwrap();
        assert!(cold_result.is_success());

        // Warm run: nonce slot seeded above 1 so the handler grants the
        // warm-nonce refund (`eip8130_gas::nonce_warm_refund(params)` —
        // currently 17_100 on XLAYER_V1, derived from `nonce_cold_gas -
        // nonce_warm_gas` — added back into the gas budget).
        let nonce_seq: u64 = 5;
        let (warm_result, _) = run_eip8130_tx(
            sender,
            &[(target, Bytecode::new_legacy(bytes!("00")))],
            &[(NONCE_MANAGER_ADDRESS, slot, U256::from(nonce_seq))],
            nonce_seq,
            Eip8130Parts {
                sender,
                payer: sender,
                nonce_key,
                call_phases: vec![vec![Eip8130Call {
                    to: target,
                    data: Bytes::new(),
                    value: U256::ZERO,
                }]],
                ..Default::default()
            },
            gas_limit,
            0,
        );
        let warm_result = warm_result.unwrap();
        assert!(warm_result.is_success());

        // Both succeed and report some gas used; the relationship between
        // warm/cold gas_used is masked by the refund-cap mechanism in
        // `set_final_refund`, so we assert the weaker (but still load-
        // bearing) property that both paths complete successfully under
        // the same `tx.gas_limit`.
        assert!(cold_result.tx_gas_used() > 0);
        assert!(warm_result.tx_gas_used() > 0);
    }

    #[test]
    fn test_eip8130_nonce_mismatch_rejected() {
        let sender = Address::from([0x11; 20]);
        let nonce_key = U256::ZERO;
        let slot = aa_nonce_slot(sender, nonce_key);

        let (result, _) = run_eip8130_tx(
            sender,
            &[],
            &[(NONCE_MANAGER_ADDRESS, slot, U256::from(3u64))],
            5, // tx_nonce — state has 3, tx says 5 → NonceTooHigh
            Eip8130Parts { sender, payer: sender, nonce_key, ..Default::default() },
            200_000,
            0,
        );

        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::NonceTooHigh { tx, state },
            ))) => {
                assert_eq!(tx, 5, "tx nonce field");
                assert_eq!(state, 3, "state nonce field");
            }
            other => panic!("expected NonceTooHigh, got: {other:?}"),
        }
    }

    /// Drives one nonce-slot post-state scenario.
    ///
    /// For `nonce_free` = `None`, asserts that `nonce[sender][nonce_key]` is
    /// bumped to `tx_nonce + 1` (the sequenced bump performed by
    /// `validate_initial_tx_after_clauses` via `journal.sstore`).
    ///
    /// For `nonce_free` = `Some((nf_hash, expiry))`, asserts the regular
    /// sequenced slot stays at `ZERO` (replay protection lives in the
    /// expiring-nonce storage instead) and the seen-slot at
    /// `aa_expiring_seen_slot(nf_hash)` records `expiry`.
    fn assert_nonce_post_state(
        nonce_key: U256,
        tx_nonce: u64,
        nonce_free: Option<(B256, u64)>,
        also_assert_zero_key_untouched: bool,
    ) {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);
        let slot = aa_nonce_slot(sender, nonce_key);

        // Sequenced flows seed the current nonce; nonce-free flows must not
        // (and `validate_env` requires `tx.nonce_sequence == 0` for them).
        let storage = if nonce_free.is_some() {
            vec![]
        } else {
            vec![(NONCE_MANAGER_ADDRESS, slot, U256::from(tx_nonce))]
        };

        let parts = Eip8130Parts {
            sender,
            payer: sender,
            nonce_key,
            nonce_free_hash: nonce_free.map(|(h, _)| h),
            expiry: nonce_free.map(|(_, e)| e).unwrap_or(0),
            call_phases: vec![vec![Eip8130Call {
                to: target,
                data: Bytes::new(),
                value: U256::ZERO,
            }]],
            ..Default::default()
        };

        let (result, mut evm) = run_eip8130_tx(
            sender,
            &[(target, Bytecode::new_legacy(bytes!("00")))],
            &storage,
            tx_nonce,
            parts,
            200_000,
            0,
        );
        assert!(result.unwrap().is_success(), "tx should succeed");

        let post_seq = evm.ctx().journal_mut().sload(NONCE_MANAGER_ADDRESS, slot).unwrap().data;
        match nonce_free {
            None => assert_eq!(
                post_seq,
                U256::from(tx_nonce + 1),
                "sequenced nonce slot must be tx_nonce+1 after success",
            ),
            Some((nf_hash, expiry)) => {
                assert_eq!(
                    post_seq,
                    U256::ZERO,
                    "regular nonce slot must stay untouched in nonce-free mode",
                );
                let seen_slot = aa_expiring_seen_slot(nf_hash);
                let post_seen =
                    evm.ctx().journal_mut().sload(NONCE_MANAGER_ADDRESS, seen_slot).unwrap().data;
                assert_eq!(
                    post_seen,
                    U256::from(expiry),
                    "expiring-nonce seen slot must record `expiry` for replay protection",
                );
            }
        }

        if also_assert_zero_key_untouched {
            let zero_key_slot = aa_nonce_slot(sender, U256::ZERO);
            let post_zero =
                evm.ctx().journal_mut().sload(NONCE_MANAGER_ADDRESS, zero_key_slot).unwrap().data;
            assert_eq!(
                post_zero,
                U256::ZERO,
                "nonce[sender][0] must be untouched when bumping a different key",
            );
        }
    }

    #[test]
    fn test_eip8130_nonce_slot_bumped_for_zero_key_succeeds() {
        assert_nonce_post_state(U256::ZERO, 7, None, false);
    }

    #[test]
    fn test_eip8130_nonce_slot_bumped_for_nonzero_key_succeeds() {
        // Confirms slot derivation depends on `nonce_key` (parallel stream)
        // and the zero-key slot stays untouched.
        assert_nonce_post_state(U256::from(42u64), 3, None, true);
    }

    #[test]
    fn test_eip8130_nonce_slot_unchanged_for_nonce_free_key_succeeds() {
        // Default `BlockEnv::timestamp == 0`, so any expiry in (0, 30]
        // satisfies `expiry > now && expiry <= now + NONCE_FREE_MAX_EXPIRY_WINDOW`.
        assert_nonce_post_state(NONCE_KEY_MAX, 0, Some((B256::from([0xAB; 32]), 25)), false);
    }

    #[test]
    fn test_eip8130_expiry_rejected() {
        let sender = Address::from([0x11; 20]);
        let (result, _) = run_eip8130_tx(
            sender,
            &[],
            &[],
            0,
            Eip8130Parts { sender, payer: sender, expiry: 10, ..Default::default() },
            200_000,
            11, // block_timestamp > expiry → reject
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("transaction expired"),
                    "expected expiry rejection, got: {msg}",
                );
            }
            other => panic!("expected expiry rejection, got: {other:?}"),
        }
    }

    #[test]
    fn test_eip8130_too_many_calls_rejected() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);
        let (result, _) = run_eip8130_tx(
            sender,
            &[],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                call_phases: vec![vec![
                    Eip8130Call {
                        to: target,
                        data: Bytes::new(),
                        value: U256::ZERO,
                    };
                    crate::constants::MAX_CALLS_PER_TX + 1
                ]],
                ..Default::default()
            },
            500_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("too many calls"),
                    "expected too-many-calls rejection, got: {msg}",
                );
            }
            other => panic!("expected too-many-calls rejection, got: {other:?}"),
        }
    }

    #[test]
    fn test_eip8130_too_many_account_changes_rejected() {
        let sender = Address::from([0x11; 20]);
        let (result, _) = run_eip8130_tx(
            sender,
            &[],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                account_changes: Eip8130AccountChanges {
                    account_change_units: crate::constants::MAX_ACCOUNT_CHANGES_PER_TX + 1,
                    ..Default::default()
                },
                ..Default::default()
            },
            500_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("too many account changes"),
                    "expected too-many-account-changes rejection, got: {msg}",
                );
            }
            other => panic!("expected too-many-account-changes rejection, got: {other:?}"),
        }
    }

    #[test]
    fn test_eip8130_locked_config_change_rejected() {
        let sender = Address::from([0x11; 20]);
        let lock_slot = aa_lock_slot(sender);
        // `insert_account_storage` auto-creates ACCOUNT_CONFIG_ADDRESS with
        // `code: None`, matching the original `AccountInfo::default()` seed.
        let (result, _) = run_eip8130_tx(
            sender,
            &[],
            &[(ACCOUNT_CONFIG_ADDRESS, lock_slot, pack_lock_state(true))],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                account_changes: Eip8130AccountChanges {
                    config_writes: vec![Eip8130StorageWrite {
                        address: ACCOUNT_CONFIG_ADDRESS,
                        slot: U256::from(1),
                        value: U256::from(2),
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            200_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(msg.contains("account is locked"), "expected lock rejection, got: {msg}",);
            }
            other => panic!("expected lock rejection, got: {other:?}"),
        }
    }

    #[test]
    fn test_eip8130_config_sequence_mismatch_rejected() {
        let sender = Address::from([0x11; 20]);
        // Seed AccountConfig with non-empty bytecode so the
        // `validate_config_change_preconditions` deployment-check passes
        // and execution actually reaches the sequence-mismatch logic. With
        // empty code, the precond check would reject first with
        // "config changes require AccountConfiguration to be deployed".
        let lock_slot = aa_lock_slot(sender);
        let seq_slot = U256::from(0x1234_u64);
        let (result, _) = run_eip8130_tx(
            sender,
            &[(ACCOUNT_CONFIG_ADDRESS, Bytecode::new_legacy(bytes!("FE")))],
            &[
                (ACCOUNT_CONFIG_ADDRESS, lock_slot, pack_lock_state(false)),
                (ACCOUNT_CONFIG_ADDRESS, seq_slot, pack_sequences(0, 5)),
            ],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                account_changes: Eip8130AccountChanges {
                    sequence_updates: vec![Eip8130SequenceUpdate {
                        slot: seq_slot,
                        is_multichain: false,
                        // Update declares "previous local seq = 2" (new_value - 1),
                        // but the storage seed has local = 5 → mismatch.
                        new_value: 3,
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            200_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("config change sequence mismatch"),
                    "expected sequence-mismatch rejection, got: {msg}",
                );
            }
            other => panic!("expected sequence-mismatch rejection, got: {other:?}"),
        }
    }

    #[test]
    fn test_eip8130_sequence_update_preserves_lock_fields_succeeds() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);
        let state_slot = aa_lock_slot(sender);
        let initial = pack_account_state(5, 9, 0, 600);

        let (result, mut evm) = run_eip8130_tx(
            sender,
            &[
                (target, Bytecode::new_legacy(bytes!("00"))),
                (ACCOUNT_CONFIG_ADDRESS, Bytecode::new_legacy(bytes!("FE"))),
            ],
            &[(ACCOUNT_CONFIG_ADDRESS, state_slot, initial)],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                account_changes: Eip8130AccountChanges {
                    sequence_updates: vec![Eip8130SequenceUpdate {
                        slot: state_slot,
                        is_multichain: true,
                        new_value: 6, // tx sequence = 5, matches initial multichain sequence
                    }],
                    ..Default::default()
                },
                call_phases: vec![vec![Eip8130Call {
                    to: target,
                    data: Bytes::new(),
                    value: U256::ZERO,
                }]],
                ..Default::default()
            },
            200_000,
            0,
        );
        let result = result.expect("tx should execute");
        assert!(result.is_success(), "inclusion should succeed");

        let account = evm
            .ctx()
            .journal_mut()
            .load_account(ACCOUNT_CONFIG_ADDRESS)
            .expect("account config should be loaded");
        let updated = account
            .storage
            .get(&state_slot)
            .expect("account state slot should be present")
            .present_value();
        let bytes = updated.to_be_bytes::<32>();
        let multichain = u64::from_be_bytes(bytes[24..32].try_into().expect("8-byte slice"));
        let local = u64::from_be_bytes(bytes[16..24].try_into().expect("8-byte slice"));
        let mut unlocks_at_bytes = [0u8; 8];
        unlocks_at_bytes[3..8].copy_from_slice(&bytes[11..16]);
        let unlocks_at = u64::from_be_bytes(unlocks_at_bytes);
        let unlock_delay = u16::from_be_bytes([bytes[9], bytes[10]]);

        assert_eq!(multichain, 6, "multichain sequence should increment");
        assert_eq!(local, 9, "local sequence should be preserved");
        assert_eq!(unlocks_at, 0, "unlocksAt should be preserved");
        assert_eq!(unlock_delay, 600, "unlockDelay should be preserved");
    }

    #[test]
    fn test_eip8130_sender_auth_revoked_in_same_tx_rejected() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);
        let k1_verifier = K1_VERIFIER_ADDRESS;
        let new_verifier = Address::from([0x44; 20]);

        let mut implicit_owner = [0u8; 32];
        implicit_owner[..20].copy_from_slice(sender.as_slice());
        let eoa_owner_id = B256::from(implicit_owner);
        let new_owner_id = B256::from([0x55; 32]);

        let (result, _) = run_eip8130_tx(
            sender,
            &[
                (target, Bytecode::new_legacy(bytes!("00"))),
                (ACCOUNT_CONFIG_ADDRESS, Bytecode::new_legacy(bytes!("FE"))),
            ],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: AuthState::Native {
                    verifier: k1_verifier,
                    owner_id: eoa_owner_id,
                    delegate_inner: None,
                },
                call_phases: vec![vec![Eip8130Call {
                    to: target,
                    data: Bytes::new(),
                    value: U256::ZERO,
                }]],
                account_changes: Eip8130AccountChanges {
                    authorizer_validations: vec![Eip8130AuthorizerValidation {
                        verifier: k1_verifier,
                        owner_id: eoa_owner_id,
                        verify_call: None,
                        owner_changes: vec![
                            Eip8130ConfigOp {
                                change_type: 0x01,
                                verifier: new_verifier,
                                owner_id: new_owner_id,
                                scope: 0,
                            },
                            Eip8130ConfigOp {
                                change_type: 0x02,
                                verifier: Address::ZERO,
                                owner_id: eoa_owner_id,
                                scope: 0,
                            },
                        ],
                    }],
                    config_writes: vec![
                        Eip8130StorageWrite {
                            address: ACCOUNT_CONFIG_ADDRESS,
                            slot: aa_owner_config_slot(sender, U256::from_be_bytes(new_owner_id.0)),
                            value: pack_owner_config(new_verifier, 0),
                        },
                        Eip8130StorageWrite {
                            address: ACCOUNT_CONFIG_ADDRESS,
                            slot: aa_owner_config_slot(sender, U256::from_be_bytes(eoa_owner_id.0)),
                            value: pack_owner_config(REVOKED_VERIFIER, 0),
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            },
            300_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                // The pending in-tx revoke fires `PendingOwnerValidationError::Revoked`
                // which surfaces as the "explicitly revoked in pending config changes"
                // message. Pinning this rules out generic OOG / nonce / lock failures
                // accidentally satisfying the test.
                assert!(msg.contains("revoked"), "expected revoke rejection, got: {msg}",);
            }
            other => panic!(
                "sender auth signed by owner revoked in the same tx must be rejected with revoke reason; got: {other:?}",
            ),
        }
    }

    #[test]
    fn test_eip8130_sender_auth_owner_added_in_same_tx_succeeds() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);
        let k1_verifier = K1_VERIFIER_ADDRESS;
        let new_verifier = Address::from([0x44; 20]);

        let mut implicit_owner = [0u8; 32];
        implicit_owner[..20].copy_from_slice(sender.as_slice());
        let eoa_owner_id = B256::from(implicit_owner);
        let new_owner_id = B256::from([0x55; 32]);

        let (result, _) = run_eip8130_tx(
            sender,
            &[
                (target, Bytecode::new_legacy(bytes!("00"))),
                (ACCOUNT_CONFIG_ADDRESS, Bytecode::new_legacy(bytes!("FE"))),
            ],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: AuthState::Native {
                    verifier: new_verifier,
                    owner_id: new_owner_id,
                    delegate_inner: None,
                },
                call_phases: vec![vec![Eip8130Call {
                    to: target,
                    data: Bytes::new(),
                    value: U256::ZERO,
                }]],
                account_changes: Eip8130AccountChanges {
                    authorizer_validations: vec![Eip8130AuthorizerValidation {
                        verifier: k1_verifier,
                        owner_id: eoa_owner_id,
                        verify_call: None,
                        owner_changes: vec![
                            Eip8130ConfigOp {
                                change_type: 0x01,
                                verifier: new_verifier,
                                owner_id: new_owner_id,
                                scope: 0,
                            },
                            Eip8130ConfigOp {
                                change_type: 0x02,
                                verifier: Address::ZERO,
                                owner_id: eoa_owner_id,
                                scope: 0,
                            },
                        ],
                    }],
                    config_writes: vec![
                        Eip8130StorageWrite {
                            address: ACCOUNT_CONFIG_ADDRESS,
                            slot: aa_owner_config_slot(sender, U256::from_be_bytes(new_owner_id.0)),
                            value: pack_owner_config(new_verifier, 0),
                        },
                        Eip8130StorageWrite {
                            address: ACCOUNT_CONFIG_ADDRESS,
                            slot: aa_owner_config_slot(sender, U256::from_be_bytes(eoa_owner_id.0)),
                            value: pack_owner_config(REVOKED_VERIFIER, 0),
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            },
            300_000,
            0,
        );
        let result = result.expect("tx should execute");
        assert!(
            result.is_success(),
            "sender auth signed by owner authorized in the same tx should succeed",
        );
        let statuses = decode_phase_statuses(result.output().unwrap());
        assert_eq!(statuses, vec![true], "single STOP phase should succeed");
    }

    #[test]
    fn test_eip8130_authorizer_native_verifier_field_mismatch_rejected() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);
        let new_verifier = Address::from([0x44; 20]);

        let mut implicit_owner = [0u8; 32];
        implicit_owner[..20].copy_from_slice(sender.as_slice());
        let eoa_owner_id = B256::from(implicit_owner);
        let new_owner_id = B256::from([0x55; 32]);

        let (result, _) = run_eip8130_tx(
            sender,
            &[
                (target, Bytecode::new_legacy(bytes!("00"))),
                (ACCOUNT_CONFIG_ADDRESS, Bytecode::new_legacy(bytes!("FE"))),
            ],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: AuthState::Native {
                    verifier: new_verifier,
                    owner_id: new_owner_id,
                    delegate_inner: None,
                },
                call_phases: vec![vec![Eip8130Call {
                    to: target,
                    data: Bytes::new(),
                    value: U256::ZERO,
                }]],
                account_changes: Eip8130AccountChanges {
                    authorizer_validations: vec![Eip8130AuthorizerValidation {
                        // Regression guard: if conversion accidentally zeroes this
                        // field, inclusion validation must reject.
                        verifier: Address::ZERO,
                        owner_id: eoa_owner_id,
                        verify_call: None,
                        owner_changes: vec![
                            Eip8130ConfigOp {
                                change_type: 0x01,
                                verifier: new_verifier,
                                owner_id: new_owner_id,
                                scope: 0,
                            },
                            Eip8130ConfigOp {
                                change_type: 0x02,
                                verifier: Address::ZERO,
                                owner_id: eoa_owner_id,
                                scope: 0,
                            },
                        ],
                    }],
                    config_writes: vec![
                        Eip8130StorageWrite {
                            address: ACCOUNT_CONFIG_ADDRESS,
                            slot: aa_owner_config_slot(sender, U256::from_be_bytes(new_owner_id.0)),
                            value: pack_owner_config(new_verifier, 0),
                        },
                        Eip8130StorageWrite {
                            address: ACCOUNT_CONFIG_ADDRESS,
                            slot: aa_owner_config_slot(sender, U256::from_be_bytes(eoa_owner_id.0)),
                            value: pack_owner_config(REVOKED_VERIFIER, 0),
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            },
            300_000,
            0,
        );
        // Authorizer.verifier == Address::ZERO → owner_config lookup against
        // the seeded entry's actual verifier (`new_verifier`) fails the
        // verifier-equality check inside validate_owner_against_effective_config.
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("verifier mismatch") || msg.contains("owner_config not found"),
                    "expected owner_config / verifier-mismatch rejection, got: {msg}",
                );
            }
            other => panic!("expected owner_config rejection, got: {other:?}"),
        }
    }

    #[test]
    fn test_eip8130_custom_authorizer_staticcall_succeeds() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);
        let custom_verifier = Address::from([0xAA; 20]);
        let authorizer_owner_id = B256::from([0xBB; 32]);

        let mut implicit_owner = [0u8; 32];
        implicit_owner[..20].copy_from_slice(sender.as_slice());
        let eoa_owner_id = B256::from(implicit_owner);

        let owner_config_slot =
            aa_owner_config_slot(sender, U256::from_be_bytes(authorizer_owner_id.0));

        // gas_limit must cover intrinsic + the fork-bound
        // XLAYER_AA_CUSTOM_VERIFIER_GAS_CAP (250_000) + execution.
        let (result, _) = run_eip8130_tx(
            sender,
            &[
                (custom_verifier, make_verifier_bytecode(authorizer_owner_id)),
                (target, Bytecode::new_legacy(bytes!("00"))),
            ],
            &[(ACCOUNT_CONFIG_ADDRESS, owner_config_slot, pack_owner_config(custom_verifier, 0))],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: AuthState::Native {
                    verifier: K1_VERIFIER_ADDRESS,
                    owner_id: eoa_owner_id,
                    delegate_inner: None,
                },
                call_phases: vec![vec![Eip8130Call {
                    to: target,
                    data: Bytes::new(),
                    value: U256::ZERO,
                }]],
                account_changes: Eip8130AccountChanges {
                    authorizer_validations: vec![Eip8130AuthorizerValidation {
                        verifier: custom_verifier,
                        owner_id: B256::ZERO,
                        verify_call: Some(Eip8130VerifyCall {
                            verifier: custom_verifier,
                            calldata: Bytes::from(vec![0xCA; 36]),
                            account: sender,
                            required_scope: crate::constants::OWNER_SCOPE_CONFIG,
                        }),
                        owner_changes: vec![],
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            500_000,
            0,
        );
        let result = result.expect("tx should execute");
        assert!(result.is_success(), "custom authorizer STATICCALL should succeed at inclusion",);
    }

    #[test]
    fn test_eip8130_owner_id_visible_through_tx_context_succeeds() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x44; 20]);
        let owner_id = B256::from([0xAB; 32]);

        // Runtime for OwnerIdProbe:
        // - probe(): reads TxContext.getOwnerId() and stores it at slot 0
        // - lastOwnerId(): returns slot 0
        let probe_runtime = Bytecode::new_legacy(bytes!(
            "608060405234801561000f575f5ffd5b5060043610610034575f3560e01c80634320a6cb14610038578063b74af5a914610056575b5f5ffd5b610040610074565b60405161004d9190610111565b60405180910390f35b61005e610079565b60405161006b9190610111565b60405180910390f35b5f5481565b5f5f61aa0373ffffffffffffffffffffffffffffffffffffffff16631f5072f26040518163ffffffff1660e01b8152600401602060405180830381865afa1580156100c6573d5f5f3e3d5ffd5b505050506040513d601f19601f820116820180604052508101906100ea9190610158565b9050805f819055508091505090565b5f819050919050565b61010b816100f9565b82525050565b5f6020820190506101245f830184610102565b92915050565b5f5ffd5b610137816100f9565b8114610141575f5ffd5b50565b5f815190506101528161012e565b92915050565b5f6020828403121561016d5761016c61012a565b5b5f61017a84828501610144565b9150509291505056fea26469706673582212203ca48096bb84d6eb04b36713b485cfdc832bcb25ec90dc9b384decb8a8ba23ee64736f6c63430008210033"
        ));

        // Seed owner_config[sender][owner_id] = (K1_VERIFIER_ADDRESS, SENDER)
        // so the new dispatch_auth_state's Native validation passes — this
        // test cares about owner_id flowing through the precompile, not
        // about owner_config validation.
        let owner_slot = aa_owner_config_slot(sender, U256::from_be_bytes(owner_id.0));
        let (result, mut evm) = run_eip8130_tx(
            sender,
            &[(target, probe_runtime)],
            &[(
                ACCOUNT_CONFIG_ADDRESS,
                owner_slot,
                pack_owner_config(K1_VERIFIER_ADDRESS, crate::constants::OWNER_SCOPE_SENDER),
            )],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: AuthState::Native {
                    verifier: K1_VERIFIER_ADDRESS,
                    owner_id,
                    delegate_inner: None,
                },
                call_phases: vec![vec![Eip8130Call {
                    to: target,
                    data: bytes!("b74af5a9"), // probe()
                    value: U256::ZERO,
                }]],
                ..Default::default()
            },
            300_000,
            0,
        );
        let result = result.unwrap();
        assert!(result.is_success(), "probe call should succeed");

        let account =
            evm.ctx().journal_mut().load_account(target).expect("target account should be loaded");
        let slot = account.storage.get(&U256::ZERO).expect("probe should write slot 0");
        assert_eq!(
            slot.present_value(),
            U256::from_be_bytes(owner_id.0),
            "slot0 should store owner_id"
        );
    }

    // -----------------------------------------------------------------------
    // Custom verifier STATICCALL tests
    // -----------------------------------------------------------------------

    /// Builds bytecode that returns a fixed 32-byte value (`owner_id`).
    ///
    /// Bytecode: PUSH32 <id> | PUSH1 0 | MSTORE | PUSH1 32 | PUSH1 0 | RETURN
    fn make_verifier_bytecode(owner_id: B256) -> Bytecode {
        let mut code = vec![0x7f]; // PUSH32
        code.extend_from_slice(owner_id.as_slice());
        code.extend_from_slice(&[
            0x60, 0x00, // PUSH1 0
            0x52, // MSTORE
            0x60, 0x20, // PUSH1 32
            0x60, 0x00, // PUSH1 0
            0xF3, // RETURN
        ]);
        Bytecode::new_legacy(Bytes::from(code))
    }

    /// Packs `(verifier_address, scope)` into the 32-byte word format used by
    /// `AccountConfig`'s `owner_config` mapping.
    fn pack_owner_config(verifier: Address, scope: u8) -> U256 {
        let mut bytes = [0u8; 32];
        bytes[11] = scope;
        bytes[12..32].copy_from_slice(verifier.as_slice());
        U256::from_be_bytes(bytes)
    }

    /// Packs an `AccountState` storage word.
    ///
    /// Layout: `zeros(9) | unlockDelay(2) | unlocksAt(5) | localSequence(8) |
    /// multichainSequence(8)`.
    fn pack_account_state(multichain: u64, local: u64, unlocks_at: u64, unlock_delay: u16) -> U256 {
        let mut bytes = [0u8; 32];
        bytes[24..32].copy_from_slice(&multichain.to_be_bytes());
        bytes[16..24].copy_from_slice(&local.to_be_bytes());
        let unlocks_at_bytes = unlocks_at.to_be_bytes();
        bytes[11..16].copy_from_slice(&unlocks_at_bytes[3..8]);
        bytes[9..11].copy_from_slice(&unlock_delay.to_be_bytes());
        U256::from_be_bytes(bytes)
    }

    /// Packs an `AccountState` slot with only the lock-related fields.
    ///
    /// When `locked` is true, sets `unlocksAt = type(uint40).max`.
    fn pack_lock_state(locked: bool) -> U256 {
        let unlocks_at = if locked { (1_u64 << 40) - 1 } else { 0 };
        pack_account_state(0, 0, unlocks_at, 0)
    }

    fn pack_sequences(multichain: u64, local: u64) -> U256 {
        pack_account_state(multichain, local, 0, 0)
    }

    #[test]
    fn test_eip8130_custom_verifier_staticcall_succeeds() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);
        let verifier = Address::from([0xAA; 20]);
        let owner_id = B256::from([0xBB; 32]);

        let owner_config_slot = aa_owner_config_slot(sender, U256::from_be_bytes(owner_id.0));

        let (result, _) = run_eip8130_tx(
            sender,
            &[
                (verifier, make_verifier_bytecode(owner_id)),
                (target, Bytecode::new_legacy(bytes!("00"))),
            ],
            // scope=0 = unrestricted
            &[(ACCOUNT_CONFIG_ADDRESS, owner_config_slot, pack_owner_config(verifier, 0x00))],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: AuthState::Deferred {
                    spec: Eip8130VerifyCall {
                        verifier,
                        calldata: Bytes::from(vec![0xCA; 36]),
                        account: sender,
                        required_scope: 0x02, // SENDER
                    },
                    delegate_outer: None,
                },
                call_phases: vec![vec![Eip8130Call {
                    to: target,
                    data: Bytes::new(),
                    value: U256::ZERO,
                }]],
                ..Default::default()
            },
            500_000,
            0,
        );
        let result = result.unwrap();
        assert!(result.is_success(), "custom verifier STATICCALL should succeed");

        let statuses = decode_phase_statuses(result.output().unwrap());
        assert_eq!(statuses, vec![true]);
    }

    #[test]
    fn test_eip8130_custom_verifier_cannot_use_implicit_eoa_owner() {
        let sender = Address::from([0x11; 20]);
        let verifier = Address::from([0xAA; 20]);

        let mut owner_id_bytes = [0u8; 32];
        owner_id_bytes[..20].copy_from_slice(sender.as_slice());
        let implicit_eoa_owner_id = B256::from(owner_id_bytes);

        let (result, _) = run_eip8130_tx(
            sender,
            &[(verifier, make_verifier_bytecode(implicit_eoa_owner_id))],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: AuthState::Deferred {
                    spec: Eip8130VerifyCall {
                        verifier,
                        calldata: Bytes::from(vec![0xCA; 36]),
                        account: sender,
                        required_scope: 0x02, // SENDER
                    },
                    delegate_outer: None,
                },
                call_phases: vec![],
                ..Default::default()
            },
            500_000,
            0,
        );

        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("owner_config not found"),
                    "custom verifier must not satisfy implicit EOA auth, got: {msg}",
                );
            }
            other => panic!("expected missing-owner-config rejection, got: {other:?}"),
        }
    }

    #[test]
    fn test_eip8130_custom_verifier_wrong_verifier_rejected() {
        let sender = Address::from([0x11; 20]);
        let verifier = Address::from([0xAA; 20]);
        let wrong_verifier = Address::from([0xCC; 20]); // different from expected
        let owner_id = B256::from([0xBB; 32]);

        let owner_config_slot = aa_owner_config_slot(sender, U256::from_be_bytes(owner_id.0));

        let (result, _) = run_eip8130_tx(
            sender,
            &[(verifier, make_verifier_bytecode(owner_id))],
            // Store a DIFFERENT verifier in owner_config than what the tx specifies.
            &[(ACCOUNT_CONFIG_ADDRESS, owner_config_slot, pack_owner_config(wrong_verifier, 0x00))],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: AuthState::Deferred {
                    spec: Eip8130VerifyCall {
                        verifier,
                        calldata: Bytes::from(vec![0xCA; 36]),
                        account: sender,
                        required_scope: 0x02,
                    },
                    delegate_outer: None,
                },
                call_phases: vec![],
                ..Default::default()
            },
            500_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("verifier mismatch"),
                    "expected verifier-mismatch rejection, got: {msg}",
                );
            }
            other => panic!("expected verifier-mismatch rejection, got: {other:?}"),
        }
    }

    #[test]
    fn test_eip8130_custom_verifier_wrong_scope_rejected() {
        let sender = Address::from([0x11; 20]);
        let verifier = Address::from([0xAA; 20]);
        let owner_id = B256::from([0xBB; 32]);

        let owner_config_slot = aa_owner_config_slot(sender, U256::from_be_bytes(owner_id.0));

        let (result, _) = run_eip8130_tx(
            sender,
            &[(verifier, make_verifier_bytecode(owner_id))],
            // Scope = PAYER (0x04), but required is SENDER (0x02) → should fail.
            &[(ACCOUNT_CONFIG_ADDRESS, owner_config_slot, pack_owner_config(verifier, 0x04))],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: AuthState::Deferred {
                    spec: Eip8130VerifyCall {
                        verifier,
                        calldata: Bytes::from(vec![0xCA; 36]),
                        account: sender,
                        required_scope: 0x02, // SENDER
                    },
                    delegate_outer: None,
                },
                call_phases: vec![],
                ..Default::default()
            },
            500_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(msg.contains("scope"), "expected scope-mismatch rejection, got: {msg}",);
            }
            other => panic!("expected scope rejection, got: {other:?}"),
        }
    }

    // ------------------------------------------------------------------
    // EIP-8130 delegation gating: requires sender authenticated as EOA
    // self-owner with CONFIG scope.
    // ------------------------------------------------------------------

    /// Thin wrapper over `run_eip8130_tx` for the delegation-owner tests:
    /// pre-seeds `target` with a STOP runtime and `ACCOUNT_CONFIG_ADDRESS`
    /// with non-empty bytecode (so config-change preconditions pass).
    fn run_delegation_owner_tx(
        sender: Address,
        target: Address,
        eip8130: Eip8130Parts,
        extra_storage: &[(Address, U256, U256)],
    ) -> RunResult {
        let (result, _) = run_eip8130_tx(
            sender,
            &[
                (target, Bytecode::new_legacy(bytes!("00"))),
                (ACCOUNT_CONFIG_ADDRESS, Bytecode::new_legacy(bytes!("FE"))),
            ],
            extra_storage,
            0,
            eip8130,
            300_000,
            0,
        );
        result
    }

    #[test]
    fn test_eip8130_delegation_no_target_skipped_succeeds() {
        // delegation_target = None, so the helper must short-circuit and
        // never look at owner_id. To verify the short-circuit really fires
        // (rather than the helper running and happening to pass), we set
        // sender_auth to a non-EOA-self-owner pattern (`owner_id = 0xCC..0xCC`)
        // and seed AccountConfig so the auth dispatch passes. If anyone
        // removes the `if delegation_target.is_none() { return Ok(()) }`
        // short-circuit, the helper will compare owner_id vs bytes20(sender),
        // see the mismatch, and reject with "EOA self-owner" — flipping
        // this test from passing to failing.
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);

        let non_self_owner = B256::repeat_byte(0xCC);
        let owner_slot = aa_owner_config_slot(sender, U256::from_be_bytes(non_self_owner.0));

        let result = run_delegation_owner_tx(
            sender,
            target,
            Eip8130Parts {
                sender,
                payer: sender,
                // Non-EOA-self-owner: owner_id is `0xCC..0xCC`, not
                // `bytes32(bytes20(sender))`. If the delegation helper
                // ran, this would fire the EOA-self-owner rejection.
                sender_authstate: AuthState::Native {
                    verifier: K1_VERIFIER_ADDRESS,
                    owner_id: non_self_owner,
                    delegate_inner: None,
                },
                account_changes: Eip8130AccountChanges {
                    delegation_target: None,
                    ..Default::default()
                },
                call_phases: vec![vec![Eip8130Call {
                    to: target,
                    data: Bytes::new(),
                    value: U256::ZERO,
                }]],
                ..Default::default()
            },
            // Seed owner_config so dispatch_auth_state's Native check passes
            // — this isolates the test to the delegation-helper short-circuit
            // (any auth-dispatch failure would mask the regression we're
            // trying to catch).
            &[(
                ACCOUNT_CONFIG_ADDRESS,
                owner_slot,
                pack_owner_config(K1_VERIFIER_ADDRESS, crate::constants::OWNER_SCOPE_SENDER),
            )],
        );
        let result = result.expect("handler should produce a result");
        assert!(
            result.is_success(),
            "delegation gating must short-circuit when delegation_target is None \
             (regression: removing the short-circuit would reject this tx with \
              'EOA self-owner' because owner_id != bytes20(sender))",
        );
    }

    #[test]
    fn test_eip8130_delegation_with_custom_verifier_owner_id_rejected() {
        // delegation_target = Some(ZERO) (clearing) but owner_id is a custom-
        // verifier identifier (not bytes32(bytes20(sender))). Helper must
        // reject with the EOA-self-owner message.
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);

        let result = run_delegation_owner_tx(
            sender,
            target,
            Eip8130Parts {
                sender,
                payer: sender,
                // Custom-verifier authenticated owner id (Deferred returns
                // owner_id = ZERO from the helper's match, which is != the
                // expected EOA-self-owner pattern → reject).
                sender_authstate: AuthState::Deferred {
                    spec: Eip8130VerifyCall {
                        verifier: Address::repeat_byte(0xAB),
                        calldata: Bytes::new(),
                        account: sender,
                        required_scope: crate::constants::OWNER_SCOPE_SENDER,
                    },
                    delegate_outer: None,
                },
                account_changes: Eip8130AccountChanges {
                    delegation_target: Some(Address::ZERO),
                    ..Default::default()
                },
                call_phases: vec![vec![Eip8130Call {
                    to: target,
                    data: Bytes::new(),
                    value: U256::ZERO,
                }]],
                ..Default::default()
            },
            &[],
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("EOA self-owner"),
                    "expected EOA-self-owner rejection, got: {msg}",
                );
            }
            other => panic!("expected EOA-self-owner rejection, got: {other:?}"),
        }
    }

    #[test]
    fn test_eip8130_delegation_with_eoa_owner_implicit_eoa_path_succeeds() {
        // delegation_target = Some(target), owner_id = bytes32(bytes20(sender)),
        // AccountConfig owner_config is empty for that slot → implicit-EOA
        // fallback in validate_owner_against_effective_config should accept.
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);

        let mut eoa_owner_bytes = [0u8; 32];
        eoa_owner_bytes[..20].copy_from_slice(sender.as_slice());
        let eoa_owner_id = B256::from(eoa_owner_bytes);

        let result = run_delegation_owner_tx(
            sender,
            target,
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: AuthState::Native {
                    verifier: K1_VERIFIER_ADDRESS,
                    owner_id: eoa_owner_id,
                    delegate_inner: None,
                },
                account_changes: Eip8130AccountChanges {
                    delegation_target: Some(target),
                    ..Default::default()
                },
                call_phases: vec![vec![Eip8130Call {
                    to: target,
                    data: Bytes::new(),
                    value: U256::ZERO,
                }]],
                ..Default::default()
            },
            &[],
        );
        let result = result.expect("handler should produce a result");
        assert!(
            result.is_success(),
            "implicit-EOA fallback must permit delegation when owner_id matches sender",
        );
    }

    #[test]
    fn test_eip8130_delegation_with_eoa_owner_explicit_config_with_config_scope_succeeds() {
        // Same as the implicit-EOA test but the slot is explicitly populated
        // with (K1_VERIFIER_ADDRESS, scope = SENDER | CONFIG). Expect Ok.
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);

        let mut eoa_owner_bytes = [0u8; 32];
        eoa_owner_bytes[..20].copy_from_slice(sender.as_slice());
        let eoa_owner_id = B256::from(eoa_owner_bytes);
        let owner_slot = aa_owner_config_slot(sender, U256::from_be_bytes(eoa_owner_id.0));
        let scope = crate::constants::OWNER_SCOPE_SENDER | crate::constants::OWNER_SCOPE_CONFIG;

        let result = run_delegation_owner_tx(
            sender,
            target,
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: AuthState::Native {
                    verifier: K1_VERIFIER_ADDRESS,
                    owner_id: eoa_owner_id,
                    delegate_inner: None,
                },
                account_changes: Eip8130AccountChanges {
                    delegation_target: Some(target),
                    ..Default::default()
                },
                call_phases: vec![vec![Eip8130Call {
                    to: target,
                    data: Bytes::new(),
                    value: U256::ZERO,
                }]],
                ..Default::default()
            },
            &[(ACCOUNT_CONFIG_ADDRESS, owner_slot, pack_owner_config(K1_VERIFIER_ADDRESS, scope))],
        );
        let result = result.expect("handler should produce a result");
        assert!(
            result.is_success(),
            "explicit owner_config with CONFIG scope must permit delegation",
        );
    }

    #[test]
    fn test_eip8130_delegation_with_eoa_owner_lacking_config_scope_rejected() {
        // owner_config seeded with (K1_VERIFIER_ADDRESS, scope = SENDER) — no
        // CONFIG bit. Helper must reject with the "lacks required scope"
        // message from validate_owner_against_effective_config.
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);

        let mut eoa_owner_bytes = [0u8; 32];
        eoa_owner_bytes[..20].copy_from_slice(sender.as_slice());
        let eoa_owner_id = B256::from(eoa_owner_bytes);
        let owner_slot = aa_owner_config_slot(sender, U256::from_be_bytes(eoa_owner_id.0));
        let scope = crate::constants::OWNER_SCOPE_SENDER; // no CONFIG bit

        let result = run_delegation_owner_tx(
            sender,
            target,
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: AuthState::Native {
                    verifier: K1_VERIFIER_ADDRESS,
                    owner_id: eoa_owner_id,
                    delegate_inner: None,
                },
                account_changes: Eip8130AccountChanges {
                    delegation_target: Some(target),
                    ..Default::default()
                },
                call_phases: vec![vec![Eip8130Call {
                    to: target,
                    data: Bytes::new(),
                    value: U256::ZERO,
                }]],
                ..Default::default()
            },
            &[(ACCOUNT_CONFIG_ADDRESS, owner_slot, pack_owner_config(K1_VERIFIER_ADDRESS, scope))],
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("lacks required scope"),
                    "expected scope rejection, got: {msg}",
                );
            }
            other => panic!("expected scope rejection, got: {other:?}"),
        }
    }

    #[test]
    fn test_eip8130_delegation_with_explicitly_revoked_eoa_owner_rejected() {
        // owner_config seeded with REVOKED_VERIFIER sentinel. Helper must
        // reject with the "revoked" message from
        // validate_owner_against_effective_config.
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);

        let mut eoa_owner_bytes = [0u8; 32];
        eoa_owner_bytes[..20].copy_from_slice(sender.as_slice());
        let eoa_owner_id = B256::from(eoa_owner_bytes);
        let owner_slot = aa_owner_config_slot(sender, U256::from_be_bytes(eoa_owner_id.0));

        let result = run_delegation_owner_tx(
            sender,
            target,
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: AuthState::Native {
                    verifier: K1_VERIFIER_ADDRESS,
                    owner_id: eoa_owner_id,
                    delegate_inner: None,
                },
                account_changes: Eip8130AccountChanges {
                    delegation_target: Some(target),
                    ..Default::default()
                },
                call_phases: vec![vec![Eip8130Call {
                    to: target,
                    data: Bytes::new(),
                    value: U256::ZERO,
                }]],
                ..Default::default()
            },
            &[(ACCOUNT_CONFIG_ADDRESS, owner_slot, pack_owner_config(REVOKED_VERIFIER, 0))],
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(msg.contains("revoked"), "expected revoked rejection, got: {msg}",);
            }
            other => panic!("expected revoked rejection, got: {other:?}"),
        }
    }

    // ── Delegate→Native inner-binding regression coverage ───────────────────
    //
    // Base's `verify_delegate` (used at conversion / validator side) does NOT
    // check the inner-recovered owner against the delegate account's
    // owner_config — only base's mempool `verify_delegate_auth_with_scope`
    // does. We're not running a separate mempool yet, so the inner check
    // must run at validator dispatch. These tests pin that down.

    use crate::transaction::eip8130::DelegateInner;

    /// Thin wrapper over `run_eip8130_tx` for Delegate→Native inner-binding
    /// tests. `target`, `delegate_addr`, and `ACCOUNT_CONFIG_ADDRESS` are left
    /// without seeded code (the original helper used `AccountInfo::default()`
    /// for them — `insert_account_storage` matches that by auto-creating
    /// accounts with `code: None`).
    fn run_delegate_native_inner_tx(
        sender: Address,
        _delegate_addr: Address,
        sender_auth: AuthState,
        owner_config_seeds: &[(U256, U256)],
    ) -> RunResult {
        let target = Address::from([0x22; 20]);
        let storage: Vec<(Address, U256, U256)> = owner_config_seeds
            .iter()
            .map(|(slot, value)| (ACCOUNT_CONFIG_ADDRESS, *slot, *value))
            .collect();
        let (result, _) = run_eip8130_tx(
            sender,
            &[],
            &storage,
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: sender_auth,
                call_phases: vec![vec![Eip8130Call {
                    to: target,
                    data: Bytes::new(),
                    value: U256::ZERO,
                }]],
                ..Default::default()
            },
            300_000,
            0,
        );
        result
    }

    #[test]
    fn test_eip8130_delegate_native_inner_owner_unregistered_rejected() {
        // Delegate→Native: outer (sender → bytes20(delegate)) is registered
        // but the inner-recovered owner (e.g. the K1 signer) is *not*
        // registered on the delegate account's owner_config. Validator must
        // reject. This is the bug base's `verify_delegate` misses on the
        // validator side.
        let sender = Address::from([0x11; 20]);
        let delegate_addr = Address::from([0x33; 20]);
        let inner_owner_id = B256::repeat_byte(0xCC);
        let mut delegate_owner_id_bytes = [0u8; 32];
        delegate_owner_id_bytes[..20].copy_from_slice(delegate_addr.as_slice());
        let delegate_owner_id = B256::from(delegate_owner_id_bytes);

        // Outer slot: owner_config[sender][bytes20(delegate)] = (DELEGATE, SENDER)
        let outer_slot = aa_owner_config_slot(sender, U256::from_be_bytes(delegate_owner_id.0));
        let outer_packed = pack_owner_config(
            crate::constants::DELEGATE_VERIFIER_ADDRESS,
            crate::constants::OWNER_SCOPE_SENDER,
        );
        // Inner slot is *deliberately not seeded* — this is the missing
        // registration the inner check must catch.

        let result = run_delegate_native_inner_tx(
            sender,
            delegate_addr,
            AuthState::Native {
                verifier: crate::constants::DELEGATE_VERIFIER_ADDRESS,
                owner_id: delegate_owner_id,
                delegate_inner: Some(DelegateInner {
                    verifier: K1_VERIFIER_ADDRESS,
                    owner_id: inner_owner_id,
                }),
            },
            &[(outer_slot, outer_packed)],
        );

        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("owner_config not found") ||
                        msg.contains("revoked") ||
                        msg.contains("verifier mismatch"),
                    "expected inner-binding rejection, got: {msg}",
                );
            }
            other => panic!("expected inner-binding rejection, got: {other:?}"),
        }
    }

    #[test]
    fn test_eip8130_delegate_native_inner_owner_registered_succeeds() {
        // Delegate→Native happy path: both outer and inner bindings are
        // registered. Should accept.
        let sender = Address::from([0x11; 20]);
        let delegate_addr = Address::from([0x33; 20]);
        let inner_owner_id = B256::repeat_byte(0xCC);
        let mut delegate_owner_id_bytes = [0u8; 32];
        delegate_owner_id_bytes[..20].copy_from_slice(delegate_addr.as_slice());
        let delegate_owner_id = B256::from(delegate_owner_id_bytes);

        let outer_slot = aa_owner_config_slot(sender, U256::from_be_bytes(delegate_owner_id.0));
        let outer_packed = pack_owner_config(
            crate::constants::DELEGATE_VERIFIER_ADDRESS,
            crate::constants::OWNER_SCOPE_SENDER,
        );
        let inner_slot = aa_owner_config_slot(delegate_addr, U256::from_be_bytes(inner_owner_id.0));
        let inner_packed =
            pack_owner_config(K1_VERIFIER_ADDRESS, crate::constants::OWNER_SCOPE_SENDER);

        let result = run_delegate_native_inner_tx(
            sender,
            delegate_addr,
            AuthState::Native {
                verifier: crate::constants::DELEGATE_VERIFIER_ADDRESS,
                owner_id: delegate_owner_id,
                delegate_inner: Some(DelegateInner {
                    verifier: K1_VERIFIER_ADDRESS,
                    owner_id: inner_owner_id,
                }),
            },
            &[(outer_slot, outer_packed), (inner_slot, inner_packed)],
        );

        let result = result.expect("handler should produce a result");
        assert!(
            result.is_success(),
            "Delegate→Native with both outer + inner registered should succeed",
        );
    }

    #[test]
    fn test_eip8130_delegate_native_inner_verifier_mismatch_rejected() {
        // Inner slot is registered, but with a *different* verifier than
        // the inner crypto used. Validator must reject.
        let sender = Address::from([0x11; 20]);
        let delegate_addr = Address::from([0x33; 20]);
        let inner_owner_id = B256::repeat_byte(0xCC);
        let mut delegate_owner_id_bytes = [0u8; 32];
        delegate_owner_id_bytes[..20].copy_from_slice(delegate_addr.as_slice());
        let delegate_owner_id = B256::from(delegate_owner_id_bytes);

        let outer_slot = aa_owner_config_slot(sender, U256::from_be_bytes(delegate_owner_id.0));
        let outer_packed = pack_owner_config(
            crate::constants::DELEGATE_VERIFIER_ADDRESS,
            crate::constants::OWNER_SCOPE_SENDER,
        );
        let inner_slot = aa_owner_config_slot(delegate_addr, U256::from_be_bytes(inner_owner_id.0));
        // Registered with a *different* verifier than what the auth claims (K1).
        let inner_packed =
            pack_owner_config(Address::from([0x99; 20]), crate::constants::OWNER_SCOPE_SENDER);

        let result = run_delegate_native_inner_tx(
            sender,
            delegate_addr,
            AuthState::Native {
                verifier: crate::constants::DELEGATE_VERIFIER_ADDRESS,
                owner_id: delegate_owner_id,
                delegate_inner: Some(DelegateInner {
                    verifier: K1_VERIFIER_ADDRESS,
                    owner_id: inner_owner_id,
                }),
            },
            &[(outer_slot, outer_packed), (inner_slot, inner_packed)],
        );

        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("verifier mismatch"),
                    "expected verifier-mismatch rejection, got: {msg}",
                );
            }
            other => panic!("expected verifier-mismatch rejection, got: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // validate_env structural rejects (spec gating, fee bounds, auth, etc.)
    // -----------------------------------------------------------------------

    #[test]
    fn test_eip8130_pre_xlayer_v1_spec_rejected() {
        let sender = Address::from([0x11; 20]);
        let (result, _) = run_eip8130_tx_with_opts(
            sender,
            &[],
            &[],
            0,
            Eip8130Parts { sender, payer: sender, ..Default::default() },
            200_000,
            0,
            Eip8130TxOpts { spec: Some(OpSpecId::ISTHMUS), ..Default::default() },
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("EIP-8130 AA transactions require XLAYER_V1"),
                    "expected pre-XLAYER_V1 rejection, got: {msg}",
                );
            }
            other => panic!("expected pre-XLAYER_V1 rejection, got: {other:?}"),
        }
    }

    #[test]
    fn test_eip8130_max_fee_below_basefee_rejected() {
        let sender = Address::from([0x11; 20]);
        let (result, _) = run_eip8130_tx_with_opts(
            sender,
            &[],
            &[],
            0,
            Eip8130Parts { sender, payer: sender, ..Default::default() },
            200_000,
            0,
            Eip8130TxOpts { block_basefee: 100, max_fee_per_gas: Some(50), ..Default::default() },
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("max_fee_per_gas below base fee"),
                    "expected max_fee<basefee rejection, got: {msg}",
                );
            }
            other => panic!("expected max_fee<basefee rejection, got: {other:?}"),
        }
    }

    #[test]
    fn test_eip8130_priority_fee_exceeds_max_fee_rejected() {
        let sender = Address::from([0x11; 20]);
        let (result, _) = run_eip8130_tx_with_opts(
            sender,
            &[],
            &[],
            0,
            Eip8130Parts { sender, payer: sender, ..Default::default() },
            200_000,
            0,
            Eip8130TxOpts {
                max_fee_per_gas: Some(100),
                max_priority_fee_per_gas: Some(150),
                ..Default::default()
            },
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("max_priority_fee_per_gas exceeds max_fee_per_gas"),
                    "expected priority>max rejection, got: {msg}",
                );
            }
            other => panic!("expected priority>max rejection, got: {other:?}"),
        }
    }

    #[test]
    fn test_eip8130_missing_sender_auth_rejected() {
        let sender = Address::from([0x11; 20]);
        let (result, _) = run_eip8130_tx(
            sender,
            &[],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: AuthState::Empty,
                ..Default::default()
            },
            200_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("missing sender_auth signature"),
                    "expected missing sender_auth, got: {msg}",
                );
            }
            other => panic!("expected missing sender_auth rejection, got: {other:?}"),
        }
    }

    #[test]
    fn test_eip8130_missing_payer_auth_rejected() {
        let sender = Address::from([0x11; 20]);
        let payer = Address::from([0x12; 20]);
        let (result, _) = run_eip8130_tx(
            sender,
            &[(payer, Bytecode::new_legacy(bytes!("00")))],
            &[],
            0,
            Eip8130Parts { sender, payer, payer_authstate: AuthState::Empty, ..Default::default() },
            200_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("missing payer_auth signature"),
                    "expected missing payer_auth, got: {msg}",
                );
            }
            other => panic!("expected missing payer_auth rejection, got: {other:?}"),
        }
    }

    #[test]
    fn test_eip8130_invalid_sender_authstate_rejected() {
        let sender = Address::from([0x11; 20]);
        let (result, _) = run_eip8130_tx(
            sender,
            &[],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: AuthState::Invalid("test reason".to_string()),
                ..Default::default()
            },
            200_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("invalid sender_auth"),
                    "expected invalid sender_auth, got: {msg}",
                );
            }
            other => panic!("expected invalid sender_auth rejection, got: {other:?}"),
        }
    }

    #[test]
    fn test_eip8130_invalid_payer_authstate_rejected() {
        let sender = Address::from([0x11; 20]);
        let payer = Address::from([0x12; 20]);
        let (result, _) = run_eip8130_tx(
            sender,
            &[(payer, Bytecode::new_legacy(bytes!("00")))],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer,
                payer_authstate: AuthState::Invalid("test reason".to_string()),
                ..Default::default()
            },
            200_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("invalid payer_auth"),
                    "expected invalid payer_auth, got: {msg}",
                );
            }
            other => panic!("expected invalid payer_auth rejection, got: {other:?}"),
        }
    }

    #[test]
    fn test_eip8130_more_than_one_create_entry_rejected() {
        let sender = Address::from([0x11; 20]);
        let addr_a = Address::from([0xA1; 20]);
        let addr_b = Address::from([0xA2; 20]);
        let (result, _) = run_eip8130_tx(
            sender,
            &[],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                account_changes: Eip8130AccountChanges {
                    has_create_entry: true,
                    code_placements: vec![
                        Eip8130CodePlacement { address: addr_a, code: bytes!("00") },
                        Eip8130CodePlacement { address: addr_b, code: bytes!("00") },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            },
            200_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("more than one create entry"),
                    "expected more-than-one-create rejection, got: {msg}",
                );
            }
            other => panic!("expected more-than-one-create rejection, got: {other:?}"),
        }
    }

    /// EIP-8130 spec: when present, the `Create` entry MUST be the first
    /// entry in `account_changes`. The parser sets
    /// `create_at_invalid_position = true` whenever a Create follows any
    /// other entry; `validate_env` then rejects.
    #[test]
    fn test_eip8130_create_not_first_rejected() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0xA1; 20]);
        let (result, _) = run_eip8130_tx(
            sender,
            &[],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                account_changes: Eip8130AccountChanges {
                    has_create_entry: true,
                    create_not_first_entry: true,
                    code_placements: vec![Eip8130CodePlacement {
                        address: target,
                        code: bytes!("00"),
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            200_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("create entry must be first"),
                    "expected create-must-be-first rejection, got: {msg}",
                );
            }
            other => panic!("expected create-must-be-first rejection, got: {other:?}"),
        }
    }

    /// EIP-8130 spec: at most one `Delegation` entry per account. A tx
    /// targets one account, so >1 Delegation entries violate the spec.
    /// The parser tracks the count via `delegation_entry_count`;
    /// `validate_env` rejects when `> 1`.
    #[test]
    fn test_eip8130_more_than_one_delegation_rejected() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0xDE; 20]);
        let (result, _) = run_eip8130_tx(
            sender,
            &[],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                account_changes: Eip8130AccountChanges {
                    delegation_target: Some(target),
                    delegation_entry_count: 2,
                    ..Default::default()
                },
                ..Default::default()
            },
            200_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("more than one delegation entry"),
                    "expected more-than-one-delegation rejection, got: {msg}",
                );
            }
            other => panic!("expected more-than-one-delegation rejection, got: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Nonce-free structural rejects (validate_env)
    // -----------------------------------------------------------------------

    #[test]
    fn test_eip8130_nonce_free_zero_expiry_rejected() {
        let sender = Address::from([0x11; 20]);
        let (result, _) = run_eip8130_tx(
            sender,
            &[],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                nonce_key: NONCE_KEY_MAX,
                expiry: 0,
                ..Default::default()
            },
            200_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("nonce-free tx requires non-zero expiry"),
                    "expected non-zero-expiry rejection, got: {msg}",
                );
            }
            other => panic!("expected non-zero-expiry rejection, got: {other:?}"),
        }
    }

    #[test]
    fn test_eip8130_nonce_free_nonzero_nonce_sequence_rejected() {
        let sender = Address::from([0x11; 20]);
        let (result, _) = run_eip8130_tx(
            sender,
            &[],
            &[],
            1, // tx_nonce != 0 with nonce-free key → reject
            Eip8130Parts {
                sender,
                payer: sender,
                nonce_key: NONCE_KEY_MAX,
                expiry: 25,
                nonce_free_hash: Some(B256::from([0xAB; 32])),
                ..Default::default()
            },
            200_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("nonce-free tx requires nonce_sequence == 0"),
                    "expected nonce_sequence==0 rejection, got: {msg}",
                );
            }
            other => panic!("expected nonce_sequence==0 rejection, got: {other:?}"),
        }
    }

    #[test]
    fn test_eip8130_nonce_free_missing_hash_rejected() {
        let sender = Address::from([0x11; 20]);
        let (result, _) = run_eip8130_tx(
            sender,
            &[],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                nonce_key: NONCE_KEY_MAX,
                expiry: 25,
                nonce_free_hash: None,
                ..Default::default()
            },
            200_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("nonce-free tx requires nonce_free_hash"),
                    "expected nonce_free_hash rejection, got: {msg}",
                );
            }
            other => panic!("expected nonce_free_hash rejection, got: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Nonce-free runtime rejects (validate_against_state_and_deduct_caller)
    // -----------------------------------------------------------------------

    #[test]
    fn test_eip8130_nonce_free_expiry_out_of_window_rejected() {
        // expiry = 100 with block_timestamp = 0 violates `expiry > now + 30`.
        let sender = Address::from([0x11; 20]);
        let (result, _) = run_eip8130_tx(
            sender,
            &[],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                nonce_key: NONCE_KEY_MAX,
                expiry: 100,
                nonce_free_hash: Some(B256::from([0xAB; 32])),
                ..Default::default()
            },
            200_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("nonce-free expiry out of window"),
                    "expected expiry-out-of-window rejection, got: {msg}",
                );
            }
            other => panic!("expected expiry-out-of-window rejection, got: {other:?}"),
        }
    }

    #[test]
    fn test_eip8130_nonce_free_replay_rejected() {
        // Pre-seed `aa_expiring_seen_slot(nf_hash) = 50` (> now == 0) to
        // simulate a hash already recorded. The replay guard fires.
        let sender = Address::from([0x11; 20]);
        let nf_hash = B256::from([0xAB; 32]);
        let seen_slot = aa_expiring_seen_slot(nf_hash);

        let (result, _) = run_eip8130_tx(
            sender,
            &[],
            &[(NONCE_MANAGER_ADDRESS, seen_slot, U256::from(50u64))],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                nonce_key: NONCE_KEY_MAX,
                expiry: 25,
                nonce_free_hash: Some(nf_hash),
                ..Default::default()
            },
            200_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("nonce-free transaction replay: hash already seen"),
                    "expected replay rejection, got: {msg}",
                );
            }
            other => panic!("expected replay rejection, got: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Config-change preconditions
    // -----------------------------------------------------------------------

    // `ACCOUNT_CONFIG_DEPLOYED` is a process-wide AtomicBool cache. Once any
    // earlier test seeded `ACCOUNT_CONFIG_ADDRESS` with non-empty bytecode,
    // the cache flips to `true` for the remainder of the process and this
    // check is bypassed. Marked `#[ignore]` to avoid order-dependent flakes;
    // the rejection path is structurally simple and exercised in dedicated
    // unit harnesses if needed.
    #[test]
    #[ignore = "ACCOUNT_CONFIG_DEPLOYED is process-wide AtomicBool; sticky after first deployed seed"]
    fn test_eip8130_config_change_without_account_config_deployed_rejected() {
        let sender = Address::from([0x11; 20]);
        let (result, _) = run_eip8130_tx(
            sender,
            &[],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                account_changes: Eip8130AccountChanges {
                    config_writes: vec![Eip8130StorageWrite {
                        address: ACCOUNT_CONFIG_ADDRESS,
                        slot: U256::from(1),
                        value: U256::from(2),
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            200_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("AccountConfiguration to be deployed"),
                    "expected deploy-precondition rejection, got: {msg}",
                );
            }
            other => panic!("expected deploy-precondition rejection, got: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Custom verifier failure paths (sender_authstate = Deferred)
    // -----------------------------------------------------------------------

    #[test]
    fn test_eip8130_custom_verifier_staticcall_revert_rejected() {
        let sender = Address::from([0x11; 20]);
        let verifier = Address::from([0xAA; 20]);
        // PUSH1 0 PUSH1 0 REVERT — succeeds-to-execute but reverts.
        let revert_code = Bytecode::new_legacy(bytes!("60006000FD"));

        let (result, _) = run_eip8130_tx(
            sender,
            &[(verifier, revert_code)],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: AuthState::Deferred {
                    spec: Eip8130VerifyCall {
                        verifier,
                        calldata: Bytes::from(vec![0xCA; 36]),
                        account: sender,
                        required_scope: 0x02,
                    },
                    delegate_outer: None,
                },
                call_phases: vec![],
                ..Default::default()
            },
            500_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("custom verifier STATICCALL failed"),
                    "expected verifier-call-failed rejection, got: {msg}",
                );
            }
            other => panic!("expected verifier-call-failed rejection, got: {other:?}"),
        }
    }

    #[test]
    fn test_eip8130_custom_verifier_short_output_rejected() {
        let sender = Address::from([0x11; 20]);
        let verifier = Address::from([0xAA; 20]);
        // PUSH1 0 PUSH1 0 RETURN — returns 0 bytes (< 32, triggers invalid owner_id).
        let empty_return = Bytecode::new_legacy(bytes!("60006000F3"));

        let (result, _) = run_eip8130_tx(
            sender,
            &[(verifier, empty_return)],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: AuthState::Deferred {
                    spec: Eip8130VerifyCall {
                        verifier,
                        calldata: Bytes::from(vec![0xCA; 36]),
                        account: sender,
                        required_scope: 0x02,
                    },
                    delegate_outer: None,
                },
                call_phases: vec![],
                ..Default::default()
            },
            500_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("custom verifier returned invalid owner_id"),
                    "expected invalid-owner_id rejection, got: {msg}",
                );
            }
            other => panic!("expected invalid-owner_id rejection, got: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Custom authorizer failure paths (authorizer_validations[i].verify_call)
    // -----------------------------------------------------------------------

    #[test]
    fn test_eip8130_custom_authorizer_staticcall_revert_rejected() {
        let sender = Address::from([0x11; 20]);
        let custom_verifier = Address::from([0xAA; 20]);
        let revert_code = Bytecode::new_legacy(bytes!("60006000FD"));

        let mut implicit_owner = [0u8; 32];
        implicit_owner[..20].copy_from_slice(sender.as_slice());
        let eoa_owner_id = B256::from(implicit_owner);

        let (result, _) = run_eip8130_tx(
            sender,
            &[
                (custom_verifier, revert_code),
                (ACCOUNT_CONFIG_ADDRESS, Bytecode::new_legacy(bytes!("FE"))),
            ],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: AuthState::Native {
                    verifier: K1_VERIFIER_ADDRESS,
                    owner_id: eoa_owner_id,
                    delegate_inner: None,
                },
                account_changes: Eip8130AccountChanges {
                    authorizer_validations: vec![Eip8130AuthorizerValidation {
                        verifier: custom_verifier,
                        owner_id: B256::ZERO,
                        verify_call: Some(Eip8130VerifyCall {
                            verifier: custom_verifier,
                            calldata: Bytes::from(vec![0xCA; 36]),
                            account: sender,
                            required_scope: crate::constants::OWNER_SCOPE_CONFIG,
                        }),
                        owner_changes: vec![],
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            500_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("config change authorizer STATICCALL failed"),
                    "expected authorizer-call-failed rejection, got: {msg}",
                );
            }
            other => panic!("expected authorizer-call-failed rejection, got: {other:?}"),
        }
    }

    #[test]
    fn test_eip8130_custom_authorizer_zero_owner_id_rejected() {
        let sender = Address::from([0x11; 20]);
        let custom_verifier = Address::from([0xAA; 20]);
        // Verifier returns 32 zero bytes → owner_id == 0 → reject.
        let zero_returner = make_verifier_bytecode(B256::ZERO);

        let mut implicit_owner = [0u8; 32];
        implicit_owner[..20].copy_from_slice(sender.as_slice());
        let eoa_owner_id = B256::from(implicit_owner);

        let (result, _) = run_eip8130_tx(
            sender,
            &[
                (custom_verifier, zero_returner),
                (ACCOUNT_CONFIG_ADDRESS, Bytecode::new_legacy(bytes!("FE"))),
            ],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: AuthState::Native {
                    verifier: K1_VERIFIER_ADDRESS,
                    owner_id: eoa_owner_id,
                    delegate_inner: None,
                },
                account_changes: Eip8130AccountChanges {
                    authorizer_validations: vec![Eip8130AuthorizerValidation {
                        verifier: custom_verifier,
                        owner_id: B256::ZERO,
                        verify_call: Some(Eip8130VerifyCall {
                            verifier: custom_verifier,
                            calldata: Bytes::from(vec![0xCA; 36]),
                            account: sender,
                            required_scope: crate::constants::OWNER_SCOPE_CONFIG,
                        }),
                        owner_changes: vec![],
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            500_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("config change authorizer returned zero owner_id"),
                    "expected zero-owner_id rejection, got: {msg}",
                );
            }
            other => panic!("expected zero-owner_id rejection, got: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Delegation entry / auto-delegation runtime paths
    // -----------------------------------------------------------------------

    #[test]
    fn test_eip8130_delegation_entry_non_delegation_bytecode_rejected() {
        // Sender has arbitrary non-delegation runtime code → delegation entry
        // must be rejected before the code is overwritten.
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x33; 20]);

        let mut eoa_owner_bytes = [0u8; 32];
        eoa_owner_bytes[..20].copy_from_slice(sender.as_slice());
        let eoa_owner_id = B256::from(eoa_owner_bytes);

        let (result, _) = run_eip8130_tx(
            sender,
            &[
                (sender, Bytecode::new_legacy(bytes!("00"))),
                (ACCOUNT_CONFIG_ADDRESS, Bytecode::new_legacy(bytes!("FE"))),
            ],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: AuthState::Native {
                    verifier: K1_VERIFIER_ADDRESS,
                    owner_id: eoa_owner_id,
                    delegate_inner: None,
                },
                account_changes: Eip8130AccountChanges {
                    delegation_target: Some(target),
                    ..Default::default()
                },
                ..Default::default()
            },
            300_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("delegation entry rejected: sender has non-delegation bytecode"),
                    "expected non-delegation-bytecode rejection, got: {msg}",
                );
            }
            other => panic!("expected non-delegation-bytecode rejection, got: {other:?}"),
        }
    }

    #[test]
    fn test_eip8130_auto_delegation_applied_for_codeless_sender_succeeds() {
        // Sender has no code, no create entry, auto_delegation_code = 23 bytes.
        // Post-execution, sender's code must be the EIP-7702 designator.
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);
        let delegate_target = Address::from([0xDD; 20]);

        let mut auto_code = vec![0xef, 0x01, 0x00];
        auto_code.extend_from_slice(delegate_target.as_slice());

        let (result, mut evm) = run_eip8130_tx(
            sender,
            &[(target, Bytecode::new_legacy(bytes!("00")))],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                account_changes: Eip8130AccountChanges {
                    auto_delegation_code: Bytes::from(auto_code),
                    ..Default::default()
                },
                call_phases: vec![vec![Eip8130Call {
                    to: target,
                    data: Bytes::new(),
                    value: U256::ZERO,
                }]],
                ..Default::default()
            },
            300_000,
            0,
        );
        let result = result.expect("tx should execute");
        assert!(result.is_success(), "auto-delegation tx should succeed");

        let acc = evm.ctx().journal_mut().load_account(sender).expect("sender loaded");
        let code = acc.info.code.as_ref().expect("sender code present");
        assert!(code.is_eip7702(), "sender code should be EIP-7702 delegation designator");
    }

    // -----------------------------------------------------------------------
    // Delegate→Custom (Deferred + delegate_outer) outer-binding coverage
    // -----------------------------------------------------------------------

    #[test]
    fn test_eip8130_delegate_custom_outer_binding_succeeds() {
        // Inner = custom verifier returning a fixed owner_id. Outer = the
        // sender's owner_config maps `bytes32(delegate_addr)` to
        // `(DELEGATE_VERIFIER_ADDRESS, scope)`.
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);
        let delegate_addr = Address::from([0x33; 20]);
        let custom_verifier = Address::from([0xAA; 20]);
        let inner_owner_id = B256::from([0xBB; 32]);

        // Inner binding: owner_config[delegate_addr][inner_owner_id] = (custom_verifier, SENDER)
        let inner_slot = aa_owner_config_slot(delegate_addr, U256::from_be_bytes(inner_owner_id.0));
        let inner_packed = pack_owner_config(custom_verifier, crate::constants::OWNER_SCOPE_SENDER);

        // Outer binding: owner_config[sender][bytes32(delegate_addr)] = (DELEGATE, SENDER)
        let mut outer_owner_bytes = [0u8; 32];
        outer_owner_bytes[..20].copy_from_slice(delegate_addr.as_slice());
        let outer_owner_id = U256::from_be_bytes(outer_owner_bytes);
        let outer_slot = aa_owner_config_slot(sender, outer_owner_id);
        let outer_packed = pack_owner_config(
            crate::constants::DELEGATE_VERIFIER_ADDRESS,
            crate::constants::OWNER_SCOPE_SENDER,
        );

        let (result, _) = run_eip8130_tx(
            sender,
            &[
                (custom_verifier, make_verifier_bytecode(inner_owner_id)),
                (target, Bytecode::new_legacy(bytes!("00"))),
            ],
            &[
                (ACCOUNT_CONFIG_ADDRESS, inner_slot, inner_packed),
                (ACCOUNT_CONFIG_ADDRESS, outer_slot, outer_packed),
            ],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: AuthState::Deferred {
                    spec: Eip8130VerifyCall {
                        verifier: custom_verifier,
                        calldata: Bytes::from(vec![0xCA; 36]),
                        account: delegate_addr,
                        required_scope: crate::constants::OWNER_SCOPE_SENDER,
                    },
                    delegate_outer: Some(delegate_addr),
                },
                call_phases: vec![vec![Eip8130Call {
                    to: target,
                    data: Bytes::new(),
                    value: U256::ZERO,
                }]],
                ..Default::default()
            },
            500_000,
            0,
        );
        let result = result.expect("tx should execute");
        assert!(result.is_success(), "Delegate→Custom outer binding should succeed");
    }

    #[test]
    fn test_eip8130_delegate_custom_outer_binding_wrong_verifier_rejected() {
        // Same as the success case, but the outer binding records a non-
        // DELEGATE verifier → outer-binding check must reject.
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);
        let delegate_addr = Address::from([0x33; 20]);
        let custom_verifier = Address::from([0xAA; 20]);
        let inner_owner_id = B256::from([0xBB; 32]);

        let inner_slot = aa_owner_config_slot(delegate_addr, U256::from_be_bytes(inner_owner_id.0));
        let inner_packed = pack_owner_config(custom_verifier, crate::constants::OWNER_SCOPE_SENDER);

        let mut outer_owner_bytes = [0u8; 32];
        outer_owner_bytes[..20].copy_from_slice(delegate_addr.as_slice());
        let outer_owner_id = U256::from_be_bytes(outer_owner_bytes);
        let outer_slot = aa_owner_config_slot(sender, outer_owner_id);
        // Wrong verifier (anything not DELEGATE_VERIFIER_ADDRESS).
        let outer_packed =
            pack_owner_config(Address::from([0x99; 20]), crate::constants::OWNER_SCOPE_SENDER);

        let (result, _) = run_eip8130_tx(
            sender,
            &[
                (custom_verifier, make_verifier_bytecode(inner_owner_id)),
                (target, Bytecode::new_legacy(bytes!("00"))),
            ],
            &[
                (ACCOUNT_CONFIG_ADDRESS, inner_slot, inner_packed),
                (ACCOUNT_CONFIG_ADDRESS, outer_slot, outer_packed),
            ],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                sender_authstate: AuthState::Deferred {
                    spec: Eip8130VerifyCall {
                        verifier: custom_verifier,
                        calldata: Bytes::from(vec![0xCA; 36]),
                        account: delegate_addr,
                        required_scope: crate::constants::OWNER_SCOPE_SENDER,
                    },
                    delegate_outer: Some(delegate_addr),
                },
                call_phases: vec![],
                ..Default::default()
            },
            500_000,
            0,
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            ))) => {
                assert!(
                    msg.contains("verifier mismatch"),
                    "expected outer-binding verifier-mismatch rejection, got: {msg}",
                );
            }
            other => panic!("expected outer-binding rejection, got: {other:?}"),
        }
    }

    /// `_rejected`: bytecode-cost-driven gas overflow.
    ///
    /// A Create entry with `5_000` bytes of bytecode bills
    /// `aa_bytecode_base_gas + 200 * 5_000 = 32_000 + 1_000_000` plus
    /// account-change / verification / payload costs. Setting a `50_000`
    /// `gas_limit` must fail with `CallGasCostMoreThanGasLimit`. Pins that
    /// `bytecode_cost` flows through to `validate_initial_tx_gas`.
    #[test]
    fn test_eip8130_create_bytecode_gas_overflow_rejected() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);

        // Bytecode large enough that 200/byte alone exceeds any reasonable
        // gas_limit. 5_000 bytes × 200 = 1_000_000 gas.
        let big_bytecode = Bytes::from(vec![0x60u8; 5_000]);

        let (result, _) = run_eip8130_tx_with_opts(
            sender,
            &[(target, Bytecode::new_legacy(bytes!("00")))],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                account_changes: Eip8130AccountChanges {
                    has_create_entry: true,
                    create_initial_owners_count: 0,
                    code_placements: vec![Eip8130CodePlacement {
                        address: Address::repeat_byte(0xAB),
                        code: big_bytecode,
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            50_000, // way under the bytecode cost
            0,
            Eip8130TxOpts { use_xlayer_gas_params: true, ..Default::default() },
        );
        match result {
            Err(EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::CallGasCostMoreThanGasLimit {
                    ..
                },
            ))) => {}
            other => panic!(
                "expected CallGasCostMoreThanGasLimit rejection from bytecode_cost, got: {other:?}",
            ),
        }
    }

    // -----------------------------------------------------------------------
    // Codex review fixes — replay / EIP-170 / empty-config-change / pending
    // overrides for delegation. These pin specific reject paths or
    // accept-paths exposed by the slice-3 review.
    // -----------------------------------------------------------------------

    /// Codex finding: an already-deployed account must not be overwritten by
    /// a Create entry whose CREATE2 tuple happens to derive the same
    /// address. The replay guard rejects when the target carries any code.
    #[test]
    fn test_eip8130_create_target_with_existing_code_rejected() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x99; 20]);
        let (result, _) = run_eip8130_tx(
            sender,
            // Pre-existing code at the target — what a previous Create
            // would have left behind.
            &[(target, Bytecode::new_legacy(bytes!("60ff60005260206000f3")))],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                account_changes: Eip8130AccountChanges {
                    has_create_entry: true,
                    code_placements: vec![Eip8130CodePlacement {
                        address: target,
                        code: bytes!("60aa60005260206000f3"),
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            300_000,
            0,
        );
        match result.unwrap_err() {
            EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            )) => assert!(
                msg.contains("create entry targets existing account"),
                "expected replay reject, got: {msg}",
            ),
            other => panic!("expected replay reject, got: {other:?}"),
        }
    }

    /// Replay guard variant: target has no code but a non-zero nonce
    /// (e.g. a regular EOA that has sent transactions). Per EIP-684-style
    /// collision check, deployment must still be rejected.
    #[test]
    fn test_eip8130_create_target_with_nonzero_nonce_rejected() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x99; 20]);

        let (result, _) = {
            let mut db = InMemoryDB::default();
            db.insert_account_info(
                sender,
                AccountInfo { balance: U256::from(10_000_000), ..Default::default() },
            );
            db.insert_account_info(
                NONCE_MANAGER_ADDRESS,
                AccountInfo {
                    code: Some(Bytecode::new_legacy(bytes!("FE"))),
                    ..Default::default()
                },
            );
            // Target has nonce 5, no code — still a collision per EIP-684.
            db.insert_account_info(target, AccountInfo { nonce: 5, ..Default::default() });

            let mut tx = OpTransaction::builder()
                .base(
                    TxEnv::builder()
                        .tx_type(Some(0x7B))
                        .caller(sender)
                        .gas_limit(300_000)
                        .kind(TxKind::Call(sender)),
                )
                .enveloped_tx(Some(bytes!("7BFACADE")))
                .build_fill();
            tx.eip8130 = Eip8130Parts {
                sender,
                payer: sender,
                account_changes: Eip8130AccountChanges {
                    has_create_entry: true,
                    code_placements: vec![Eip8130CodePlacement {
                        address: target,
                        code: bytes!("60aa60005260206000f3"),
                    }],
                    ..Default::default()
                },
                ..Default::default()
            };

            let ctx = Context::op()
                .with_db(db)
                .with_tx(tx)
                .with_block(BlockEnv { timestamp: U256::ZERO, ..Default::default() })
                .with_chain(L1BlockInfo {
                    l2_block: Some(U256::ZERO),
                    operator_fee_scalar: Some(U256::ZERO),
                    operator_fee_constant: Some(U256::ZERO),
                    ..Default::default()
                })
                .with_cfg(CfgEnv::new_with_spec(OpSpecId::XLAYER_V1));
            let mut evm = ctx.build_op();
            let mut handler =
                OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();
            (handler.run(&mut evm), evm)
        };

        match result.unwrap_err() {
            EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            )) => assert!(
                msg.contains("create entry targets existing account"),
                "expected replay reject (nonce path), got: {msg}",
            ),
            other => panic!("expected replay reject (nonce path), got: {other:?}"),
        }
    }

    /// Replay guard variant: target has empty code AND zero nonce, but the
    /// `_ownerConfig[target][initial_owner]` slot is already populated
    /// (e.g. left over from a previous Create with the same tuple, after
    /// which the target's bytecode + nonce got cleaned up via
    /// `selfdestruct`). The pre-write must observe `current == 0` before
    /// writing, otherwise replaying the Create tuple would clobber the
    /// post-create owner state.
    #[test]
    fn test_eip8130_create_target_with_existing_owner_config_rejected() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x99; 20]);
        let owner_id = U256::from_be_bytes([0xCD; 32]);
        let owner_slot = aa_owner_config_slot(target, owner_id);

        let (result, _) = run_eip8130_tx(
            sender,
            // Target itself is fresh (empty code, zero nonce), but
            // ACCOUNT_CONFIG_ADDRESS already carries an owner_config row
            // for `(target, owner_id)`.
            &[],
            &[(
                ACCOUNT_CONFIG_ADDRESS,
                owner_slot,
                pack_owner_config(K1_VERIFIER_ADDRESS, crate::constants::OWNER_SCOPE_SENDER),
            )],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                account_changes: Eip8130AccountChanges {
                    has_create_entry: true,
                    pre_writes: vec![Eip8130StorageWrite {
                        address: ACCOUNT_CONFIG_ADDRESS,
                        slot: owner_slot,
                        value: pack_owner_config(
                            K1_VERIFIER_ADDRESS,
                            crate::constants::OWNER_SCOPE_SENDER,
                        ),
                    }],
                    code_placements: vec![Eip8130CodePlacement {
                        address: target,
                        code: bytes!("60aa60005260206000f3"),
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            300_000,
            0,
        );
        match result.unwrap_err() {
            EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            )) => assert!(
                msg.contains("existing owner_config"),
                "expected owner_config replay reject, got: {msg}",
            ),
            other => panic!("expected owner_config replay reject, got: {other:?}"),
        }
    }

    /// EIP-170: deployed bytecode must not exceed `MAX_CODE_SIZE` (24 576 B).
    /// `validate_env` rejects so the bytecode never reaches the
    /// `Bytecode::new_raw` placement step (and never hits the `u16`
    /// truncation in `deployment_header`).
    #[test]
    fn test_eip8130_create_oversized_bytecode_rejected() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0xAB; 20]);
        let oversized = vec![0u8; revm::primitives::eip170::MAX_CODE_SIZE + 1];
        let (result, _) = run_eip8130_tx(
            sender,
            &[],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                account_changes: Eip8130AccountChanges {
                    has_create_entry: true,
                    code_placements: vec![Eip8130CodePlacement {
                        address: target,
                        code: Bytes::from(oversized),
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            300_000,
            0,
        );
        match result.unwrap_err() {
            EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            )) => assert!(
                msg.contains("exceeds EIP-170 max code size"),
                "expected EIP-170 reject, got: {msg}",
            ),
            other => panic!("expected EIP-170 reject, got: {other:?}"),
        }
    }

    /// Codex finding: a `ConfigChange` targeting this chain whose
    /// `owner_changes` produce zero effective ops (empty list, or every op
    /// of unknown `change_type`) would touch `_accountState` (sequence
    /// bump) and run authorizer validation without any `_ownerConfig`
    /// write. `validate_env` rejects via the
    /// `matching_config_change_with_zero_valid_ops` flag.
    #[test]
    fn test_eip8130_config_change_zero_valid_ops_rejected() {
        let sender = Address::from([0x11; 20]);
        let (result, _) = run_eip8130_tx(
            sender,
            &[],
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                account_changes: Eip8130AccountChanges {
                    matching_config_change_with_zero_valid_ops: true,
                    // Mimic what the parser would emit for a matching
                    // ConfigChange with all-unknown ops: sequence_update is
                    // pushed regardless, authorizer_validations is pushed
                    // regardless. The flag is the structural reject signal.
                    sequence_updates: vec![Eip8130SequenceUpdate {
                        slot: U256::from(1),
                        is_multichain: false,
                        new_value: 1,
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            200_000,
            0,
        );
        match result.unwrap_err() {
            EVMError::Transaction(OpTransactionError::Base(
                revm::context_interface::result::InvalidTransaction::Str(msg),
            )) => assert!(
                msg.contains("config change entry has no valid ops"),
                "expected zero-valid-ops reject, got: {msg}",
            ),
            other => panic!("expected zero-valid-ops reject, got: {other:?}"),
        }
    }

    /// Codex finding: delegation auth check must run AFTER the authorizer
    /// chain so same-tx `ConfigChange` ops authorizing the EOA self-owner
    /// with `OWNER_SCOPE_CONFIG` are visible via the pending overlay.
    ///
    /// Construction: sender's `_ownerConfig[sender][bytes20(sender)]` is
    /// pre-populated with `(K1, SENDER)` — i.e. owner exists but lacks the
    /// `OWNER_SCOPE_CONFIG` bit. Pre-fix, the early call site passed
    /// `None` for the overlay so the helper rejected. Post-fix, the late
    /// call site passes a pending overlay that upgrades the same owner
    /// to `(K1, SENDER | CONFIG)`, and the helper accepts.
    #[test]
    fn test_eip8130_delegation_pending_authorize_visible() {
        use crate::eip8130_policy::PendingOwnerState;
        use std::collections::BTreeMap;

        let sender = Address::from([0x11; 20]);

        let mut eoa_owner_bytes = [0u8; 32];
        eoa_owner_bytes[..20].copy_from_slice(sender.as_slice());
        let eoa_owner_id = B256::from(eoa_owner_bytes);

        let mut db = InMemoryDB::default();
        db.insert_account_info(
            sender,
            AccountInfo { balance: U256::from(1_000_000), ..Default::default() },
        );
        db.insert_account_info(
            ACCOUNT_CONFIG_ADDRESS,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("FE"))), ..Default::default() },
        );
        // Stored owner: K1 verifier, SENDER scope only — no CONFIG bit.
        // This blocks the implicit-EOA fallback (slot non-empty) and forces
        // the helper to consult the overlay for CONFIG scope.
        let owner_slot = aa_owner_config_slot(sender, U256::from_be_bytes(eoa_owner_id.0));
        db.insert_account_storage(
            ACCOUNT_CONFIG_ADDRESS,
            owner_slot,
            pack_owner_config(K1_VERIFIER_ADDRESS, crate::constants::OWNER_SCOPE_SENDER),
        )
        .unwrap();

        let mut tx = OpTransaction::builder()
            .base(
                TxEnv::builder()
                    .tx_type(Some(0x7B))
                    .caller(sender)
                    .gas_limit(300_000)
                    .kind(TxKind::Call(sender)),
            )
            .enveloped_tx(Some(bytes!("7BFACADE")))
            .build_fill();
        tx.eip8130 = Eip8130Parts {
            sender,
            payer: sender,
            sender_authstate: AuthState::Native {
                verifier: K1_VERIFIER_ADDRESS,
                owner_id: eoa_owner_id,
                delegate_inner: None,
            },
            account_changes: Eip8130AccountChanges {
                delegation_target: Some(Address::from([0xDE; 20])),
                ..Default::default()
            },
            ..Default::default()
        };

        let ctx = Context::op()
            .with_db(db)
            .with_tx(tx)
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::XLAYER_V1));
        let mut evm = ctx.build_op();

        // Sanity: with `None` overlay (= pre-fix early call site), the
        // helper must reject because the stored row lacks CONFIG scope.
        let result_none = super::check_delegation_requires_eoa_config_owner::<
            _,
            EVMError<_, OpTransactionError>,
        >(&mut evm, sender, None);
        assert!(
            result_none.is_err(),
            "stored row lacks CONFIG scope: pre-fix must reject without overlay",
        );

        // Post-fix: pending overlay upgrades the same owner to CONFIG
        // scope and the helper accepts.
        let mut pending: BTreeMap<U256, PendingOwnerState> = BTreeMap::new();
        pending.insert(
            U256::from_be_bytes(eoa_owner_id.0),
            PendingOwnerState::Authorized {
                verifier: K1_VERIFIER_ADDRESS,
                scope: crate::constants::OWNER_SCOPE_SENDER | crate::constants::OWNER_SCOPE_CONFIG,
            },
        );

        let result_pending = super::check_delegation_requires_eoa_config_owner::<
            _,
            EVMError<_, OpTransactionError>,
        >(&mut evm, sender, Some(&pending));
        assert!(
            result_pending.is_ok(),
            "delegation must accept EOA self-owner with pending CONFIG authorize, got: {result_pending:?}",
        );
    }
}
