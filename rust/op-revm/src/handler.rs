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
            Eip8130Parts, Eip8130PhaseResult, config_log_to_system_log, encode_phase_statuses,
            phase_statuses_system_log,
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
    primitives::{Address, B256, Bytes, U256, address, hardfork::SpecId, keccak256, uint},
};
use std::{boxed::Box, collections::BTreeMap, vec::Vec};

/// EIP-8130 AA transaction type byte.
const EIP8130_TX_TYPE: u8 = 0x7B;

/// Estimated calldata gas for a K1 auth blob missing during gas estimation.
const ESTIMATION_AUTH_CALLDATA_GAS: u64 = 1_100;

/// Gas delta between cold and warm nonce key SSTORE costs.
const NONCE_COLD_WARM_DELTA: u64 = 17_100;

/// AccountConfiguration deployed contract address.
/// Must match the CREATE2 address from the Solidity deployment script (salt = 0).
const ACCOUNT_CONFIG_ADDRESS: Address = address!("0x4F20618CF5c160e7AA385268721dA968F86F0e61");

/// Explicit native K1/ecrecover verifier sentinel (`address(1)`).
const K1_VERIFIER_ADDRESS: Address = address!("0x0000000000000000000000000000000000000001");

/// Sentinel verifier written when the implicit EOA owner is explicitly revoked.
const REVOKED_VERIFIER: Address = address!("0xffffffffffffffffffffffffffffffffffffffff");

/// Monotonic cache: once the AccountConfiguration contract is detected, we
/// skip the code-existence check on all subsequent calls.
static ACCOUNT_CONFIG_DEPLOYED: AtomicBool = AtomicBool::new(false);

/// Base storage slot for the packed `_accountState` mapping in AccountConfig (slot index 1).
const LOCK_BASE_SLOT: U256 = uint!(1_U256);

/// Sentinel nonce key that activates nonce-free mode.
const NONCE_KEY_MAX: U256 = U256::MAX;

/// Base storage slot for the expiring-seen mapping in NonceManager.
const EXPIRING_SEEN_BASE_SLOT: U256 = uint!(2_U256);
/// Base storage slot for the expiring-ring mapping in NonceManager.
const EXPIRING_RING_BASE_SLOT: U256 = uint!(3_U256);
/// Storage slot for the expiring-ring pointer in NonceManager.
const EXPIRING_RING_PTR_SLOT: U256 = uint!(4_U256);
/// Capacity of the expiring-nonce ring buffer.
const EXPIRING_NONCE_SET_CAPACITY: u32 = 300_000;
/// Maximum allowed expiry-window length for nonce-free transactions.
const NONCE_FREE_MAX_EXPIRY_WINDOW: u64 = 30;

/// Computes the storage slot for `expiringNonceSeen[txHash]`.
fn aa_expiring_seen_slot(tx_hash: B256) -> U256 {
    use alloy_sol_types::SolValue;
    U256::from_be_bytes(keccak256((tx_hash, EXPIRING_SEEN_BASE_SLOT).abi_encode()).0)
}

/// Computes the storage slot for `expiringNonceRing[index]`.
fn aa_expiring_ring_slot(index: u32) -> U256 {
    use alloy_sol_types::SolValue;
    U256::from_be_bytes(keccak256((U256::from(index), EXPIRING_RING_BASE_SLOT).abi_encode()).0)
}

/// Computes the AccountConfig storage slot for `lock_state(account)`.
fn aa_lock_slot(account: Address) -> U256 {
    use alloy_sol_types::SolValue;
    U256::from_be_bytes(keccak256((account, LOCK_BASE_SLOT).abi_encode()).0)
}

/// Owner config base storage slot in AccountConfig (slot index 0).
const OWNER_CONFIG_BASE_SLOT: U256 = U256::ZERO;

/// Computes the AccountConfig storage slot for `owner_config(account, owner_id)`.
fn aa_owner_config_slot(account: Address, owner_id: U256) -> U256 {
    use alloy_sol_types::SolValue;
    let inner = keccak256((owner_id, OWNER_CONFIG_BASE_SLOT).abi_encode());
    U256::from_be_bytes(keccak256((account, inner).abi_encode()).0)
}

/// Parses a packed owner_config word into `(verifier_address, scope)`.
fn parse_owner_config_word(word: U256) -> (Address, u8) {
    let bytes = word.to_be_bytes::<32>();
    let scope = bytes[11];
    let verifier = Address::from_slice(&bytes[12..32]);
    (verifier, scope)
}

/// Reads one sequence value from packed `AccountState`.
fn read_packed_sequence(slot_value: U256, is_multichain: bool) -> u64 {
    if is_multichain { slot_value.as_limbs()[0] } else { (slot_value >> 64_u8).as_limbs()[0] }
}

/// Extra gas to reserve during `eth_estimateGas` for missing auth blobs.
fn estimation_calldata_overhead(parts: &Eip8130Parts) -> u64 {
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

/// Validates that `owner_id` is registered in AccountConfig.
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

/// Re-validates a native verifier's owner_config at inclusion time.
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
        let inner_slot = aa_owner_config_slot(account, owner_id_uint);
        let inner_word = evm.ctx().journal_mut().sload(ACCOUNT_CONFIG_ADDRESS, inner_slot)?.data;
        let (inner_verifier, inner_scope) = parse_owner_config_word(inner_word);

        if inner_verifier == Address::ZERO {
            return Err(eip8130_invalid_tx::<ERROR>("delegate inner verifier owner revoked"));
        }
        if inner_scope != 0 && (inner_scope & required_scope) == 0 {
            return Err(eip8130_invalid_tx::<ERROR>("delegate inner owner lacks required scope"));
        }
    }

    Ok(())
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
    let lock_bytes = lock_word.to_be_bytes::<32>();
    let mut ua = [0u8; 8];
    ua[3..8].copy_from_slice(&lock_bytes[11..16]);
    Ok(u64::from_be_bytes(ua))
}

/// Pre-deduction lock check covering both delegation entries and config changes.
///
/// Per xlayer-aa.md (2026-04-21 lesson): runs BEFORE gas deduction to ensure
/// locked accounts don't pay for failed inclusion.
fn check_account_lock<EVM, ERROR>(
    evm: &mut EVM,
    sender: Address,
    eip8130: &Eip8130Parts,
) -> Result<(), ERROR>
where
    EVM: EvmTr<Context: OpContextTr>,
    ERROR: EvmTrError<EVM> + From<OpTransactionError>,
{
    let needs_check = eip8130.delegation_target.is_some() ||
        !eip8130.config_writes.is_empty() ||
        !eip8130.sequence_updates.is_empty();
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

/// Re-validates config-change preconditions at inclusion time.
fn validate_config_change_preconditions<EVM, ERROR>(
    evm: &mut EVM,
    _sender: Address,
    eip8130: &Eip8130Parts,
) -> Result<(), ERROR>
where
    EVM: EvmTr<Context: OpContextTr>,
    ERROR: EvmTrError<EVM> + From<OpTransactionError>,
{
    if eip8130.sequence_updates.is_empty() && eip8130.config_writes.is_empty() {
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

    if eip8130.sequence_updates.is_empty() {
        return Ok(());
    }

    evm.ctx().journal_mut().load_account(ACCOUNT_CONFIG_ADDRESS)?;
    // Sequence check with in-tx chaining.
    let seq_slot = eip8130.sequence_updates[0].slot;
    let packed = evm.ctx().journal_mut().sload(ACCOUNT_CONFIG_ADDRESS, seq_slot)?.data;
    let mut expected_multichain = read_packed_sequence(packed, true);
    let mut expected_local = read_packed_sequence(packed, false);

    for upd in &eip8130.sequence_updates {
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

/// Runs a custom verifier STATICCALL and decodes the returned owner_id.
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
    evm.ctx().journal_mut().load_account(verifier)?;

    let call_gas = verification_gas_cap.saturating_sub(*verification_gas_used);
    let call_inputs = CallInputs {
        input: CallInput::Bytes(calldata.clone()),
        return_memory_offset: 0..0,
        gas_limit: call_gas,
        bytecode_address: verifier,
        known_bytecode: None,
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
fn validate_authorizer_chain<EVM, ERROR, FRAME>(
    mainnet: &mut MainnetHandler<EVM, ERROR, FRAME>,
    evm: &mut EVM,
    sender: Address,
    eip8130: &Eip8130Parts,
    verification_gas_used: &mut u64,
) -> Result<BTreeMap<U256, PendingOwnerState>, ERROR>
where
    EVM: EvmTr<Context: OpContextTr, Frame = FRAME>,
    ERROR: EvmTrError<EVM> + From<OpTransactionError>,
    FRAME: FrameTr<FrameResult = FrameResult, FrameInit = FrameInit>,
{
    let mut pending_owners: BTreeMap<U256, PendingOwnerState> = BTreeMap::new();
    if eip8130.authorizer_validations.is_empty() {
        return Ok(pending_owners);
    }

    for validation in &eip8130.authorizer_validations {
        if validation.verify_call.is_none() &&
            validation.owner_id == B256::ZERO &&
            validation.owner_changes.is_empty()
        {
            continue;
        }

        let owner_id = if let Some(verify_call) = &validation.verify_call {
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
            U256::from_be_bytes(validation.owner_id.0)
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
            validation.verifier,
            crate::constants::OWNER_SCOPE_CONFIG,
            true,
            Some(&pending_owners),
        )?;

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
            if !spec.is_enabled_in(OpSpecId::XLAYER_AA) {
                return Err(eip8130_invalid_tx::<Self::Error>(
                    "EIP-8130 AA transactions require XLAYER_AA",
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

            if parts.account_change_units > crate::constants::MAX_ACCOUNT_CHANGES_PER_TX {
                return Err(eip8130_invalid_tx::<Self::Error>(
                    "EIP-8130: too many account changes",
                ));
            }

            // EIP-8130 invariant: at most one create entry per tx.
            if parts.code_placements.len() > 1 {
                return Err(eip8130_invalid_tx::<Self::Error>(
                    "EIP-8130: more than one create entry",
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
            let aa_gas = parts.aa_intrinsic_gas;
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
            let eip8130 = tx.eip8130_parts().clone();

            // Lock check before any state mutation.
            check_account_lock::<Self::Evm, Self::Error>(evm, sender, &eip8130)?;

            let (block, tx, cfg, journal, chain, _) = evm.ctx().all_mut();
            let spec = cfg.spec();

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

            let balance = calculate_caller_fee(balance, tx, block, cfg)?;
            payer_account.set_balance(balance);
            drop(payer_account);

            // Check if sender is a bare EOA (no code) for auto-delegation.
            let sender_account = journal.load_account_with_code_mut(sender)?.data;
            let sender_has_code = sender_account.account().info.code_hash != keccak256([]);
            drop(sender_account);

            // --- Nonce validation and increment in NonceManager ---
            let nonce_key = eip8130.nonce_key;
            if nonce_key != NONCE_KEY_MAX {
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
            } else {
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
            }

            // --- Delegation: explicit entry takes priority, otherwise auto. ---
            if let Some(target) = eip8130.delegation_target {
                let acc = journal.load_account_with_code_mut(sender)?.data;
                let current_code = acc.account().info.code.as_ref();
                let is_empty = current_code.map_or(true, |c| c.is_empty());
                let is_delegation = current_code.map_or(false, |c| c.is_eip7702());
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
                let mut acc = journal.load_account_with_code_mut(sender)?.data;
                acc.set_code_and_hash_slow(code);
                drop(acc);
            } else if !sender_has_code &&
                !eip8130.has_create_entry &&
                eip8130.auto_delegation_code.len() == 23
            {
                let target = Address::from_slice(&eip8130.auto_delegation_code[3..]);
                let code = revm::bytecode::Bytecode::new_eip7702(target);
                let mut acc = journal.load_account_with_code_mut(sender)?.data;
                acc.set_code_and_hash_slow(code);
                drop(acc);
            }

            // --- Apply pre-execution storage writes (account creation only) ---
            for w in &eip8130.pre_writes {
                journal.load_account(w.address)?;
                journal.sstore(w.address, w.slot, w.value)?;
            }

            // --- Account creation (place runtime bytecode at CREATE2-derived addresses) ---
            for placement in &eip8130.code_placements {
                let code = revm::bytecode::Bytecode::new_raw(placement.code.clone());
                let mut acc = journal.load_account_with_code_mut(placement.address)?.data;
                acc.set_code_and_hash_slow(code);
                drop(acc);
            }

            // --- Emit AccountConfiguration events for account creation ---
            for event in &eip8130.account_creation_logs {
                journal.log(config_log_to_system_log(ACCOUNT_CONFIG_ADDRESS, event));
            }

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

        let eip8130 = evm.ctx().tx().eip8130_parts().clone();
        let sender = evm.ctx().tx().caller();

        let is_estimation = evm.ctx().cfg().is_base_fee_check_disabled();

        let nonce_warm_adjustment = if !is_estimation && eip8130.nonce_key != NONCE_KEY_MAX {
            let nonce_slot = aa_nonce_slot(sender, eip8130.nonce_key);
            let nonce_value =
                evm.ctx().journal_mut().sload(NONCE_MANAGER_ADDRESS, nonce_slot)?.data;
            if nonce_value > U256::from(1) { NONCE_COLD_WARM_DELTA } else { 0 }
        } else {
            0
        };

        let overhead = if is_estimation { estimation_calldata_overhead(&eip8130) } else { 0 };
        let gas_limit = evm
            .ctx()
            .tx()
            .gas_limit()
            .saturating_sub(eip8130.aa_intrinsic_gas + eip8130.custom_verifier_gas_cap + overhead)
            .saturating_add(nonce_warm_adjustment);

        let mut gas_remaining = gas_limit;
        let mut phase_results = Vec::with_capacity(eip8130.call_phases.len());

        evm.ctx().journal_mut().load_account(sender)?;

        let mut verification_gas_used: u64 = 0;

        if !is_estimation {
            validate_config_change_preconditions::<Self::Evm, Self::Error>(evm, sender, &eip8130)?;

            let pending_sender_owner_overrides =
                validate_authorizer_chain::<Self::Evm, Self::Error, FRAME>(
                    &mut self.mainnet,
                    evm,
                    sender,
                    &eip8130,
                    &mut verification_gas_used,
                )?;

            let verify_calls =
                [eip8130.sender_verify_call.as_ref(), eip8130.payer_verify_call.as_ref()];
            for verify_call in verify_calls.into_iter().flatten() {
                let owner_id = run_custom_verifier_staticcall::<Self::Evm, Self::Error, FRAME>(
                    &mut self.mainnet,
                    evm,
                    verify_call.verifier,
                    &verify_call.calldata,
                    sender,
                    eip8130.custom_verifier_gas_cap,
                    &mut verification_gas_used,
                    "custom verifier STATICCALL failed",
                    "custom verifier returned invalid owner_id",
                )?;

                let pending_overrides = if verify_call.account == sender {
                    Some(&pending_sender_owner_overrides)
                } else {
                    None
                };
                validate_owner_config::<Self::Evm, Self::Error>(
                    evm,
                    verify_call.account,
                    owner_id,
                    verify_call.verifier,
                    verify_call.required_scope,
                    pending_overrides,
                )?;
            }

            if eip8130.sender_verify_call.is_some() &&
                eip8130.sender_verifier == crate::constants::DELEGATE_VERIFIER_ADDRESS &&
                eip8130.owner_id != B256::ZERO
            {
                validate_owner_config::<Self::Evm, Self::Error>(
                    evm,
                    sender,
                    U256::from_be_bytes(eip8130.owner_id.0),
                    crate::constants::DELEGATE_VERIFIER_ADDRESS,
                    crate::constants::OWNER_SCOPE_SENDER,
                    Some(&pending_sender_owner_overrides),
                )?;
            }
            if eip8130.payer_verify_call.is_some() &&
                eip8130.payer_verifier == crate::constants::DELEGATE_VERIFIER_ADDRESS &&
                eip8130.payer_owner_id != B256::ZERO &&
                eip8130.payer != eip8130.sender
            {
                let payer_pending_overrides = if eip8130.payer == sender {
                    Some(&pending_sender_owner_overrides)
                } else {
                    None
                };
                validate_owner_config::<Self::Evm, Self::Error>(
                    evm,
                    eip8130.payer,
                    U256::from_be_bytes(eip8130.payer_owner_id.0),
                    crate::constants::DELEGATE_VERIFIER_ADDRESS,
                    crate::constants::OWNER_SCOPE_PAYER,
                    payer_pending_overrides,
                )?;
            }

            if eip8130.sender_verify_call.is_none() && eip8130.sender_verifier != Address::ZERO {
                validate_native_verifier_owner::<Self::Evm, Self::Error>(
                    evm,
                    sender,
                    eip8130.sender_verifier,
                    eip8130.owner_id,
                    crate::constants::OWNER_SCOPE_SENDER,
                    Some(&pending_sender_owner_overrides),
                )?;
            }
            if eip8130.payer_verify_call.is_none() &&
                eip8130.payer_verifier != Address::ZERO &&
                eip8130.payer != eip8130.sender
            {
                let payer_pending_overrides = if eip8130.payer == sender {
                    Some(&pending_sender_owner_overrides)
                } else {
                    None
                };
                validate_native_verifier_owner::<Self::Evm, Self::Error>(
                    evm,
                    eip8130.payer,
                    eip8130.payer_verifier,
                    eip8130.payer_owner_id,
                    crate::constants::OWNER_SCOPE_PAYER,
                    payer_pending_overrides,
                )?;
            }
        }

        // --- Apply config change writes + sequence bumps ---
        if !eip8130.config_writes.is_empty() {
            for w in &eip8130.config_writes {
                evm.ctx().journal_mut().load_account(w.address)?;
                evm.ctx().journal_mut().sstore(w.address, w.slot, w.value)?;
            }
        }
        if !eip8130.sequence_updates.is_empty() {
            evm.ctx().journal_mut().load_account(ACCOUNT_CONFIG_ADDRESS)?;
            for upd in &eip8130.sequence_updates {
                let current = evm.ctx().journal_mut().sload(ACCOUNT_CONFIG_ADDRESS, upd.slot)?.data;
                let new_packed = upd.apply(current);
                evm.ctx().journal_mut().sstore(ACCOUNT_CONFIG_ADDRESS, upd.slot, new_packed)?;
            }
        }

        for event in &eip8130.config_change_logs {
            evm.ctx().journal_mut().log(config_log_to_system_log(ACCOUNT_CONFIG_ADDRESS, event));
        }

        let unused_verification_gas =
            eip8130.custom_verifier_gas_cap.saturating_sub(verification_gas_used);

        let mut accumulated_refunds: i64 = 0;

        for phase in &eip8130.call_phases {
            let checkpoint = evm.ctx().journal_mut().checkpoint();
            let mut phase_ok = true;
            let phase_gas_start = gas_remaining;
            let mut phase_refunds: i64 = 0;

            for call in phase {
                if gas_remaining == 0 {
                    phase_ok = false;
                    break;
                }

                evm.ctx().journal_mut().load_account(call.to)?;

                let call_gas = gas_remaining;
                let call_inputs = CallInputs {
                    input: CallInput::Bytes(call.data.clone()),
                    return_memory_offset: 0..0,
                    gas_limit: call_gas,
                    bytecode_address: call.to,
                    known_bytecode: None,
                    target_address: call.to,
                    caller: sender,
                    value: CallValue::Transfer(call.value),
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

            // EIP-8130 atomic-per-phase: halt remaining phases on first failure
            // (xlayer-aa.md 2026-04-21 lesson).
            if !phase_ok {
                break;
            }
        }

        let any_phase_succeeded = phase_results.iter().any(|r| r.success);
        let deploy_only_success = phase_results.is_empty() && !eip8130.code_placements.is_empty();
        let tx_succeeded = is_estimation || any_phase_succeeded || deploy_only_success;

        if !phase_results.is_empty() {
            evm.ctx()
                .journal_mut()
                .log(phase_statuses_system_log(TX_CONTEXT_ADDRESS, &phase_results));
        }

        let mut result_gas = Gas::new_spent(evm.ctx().tx().gas_limit());
        result_gas.erase_cost(gas_remaining + unused_verification_gas);
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

        // Spend the gas limit. Gas is reimbursed when the tx returns successfully.
        *gas = Gas::new_spent(tx_gas_limit);

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
        ACCOUNT_CONFIG_ADDRESS, K1_VERIFIER_ADDRESS, NONCE_COLD_WARM_DELTA, OpHandler,
        REVOKED_VERIFIER, aa_lock_slot, aa_owner_config_slot,
    };
    use crate::{
        DefaultOp, L1BlockInfo, OpBuilder, OpContext, OpHaltReason, OpSpecId, OpTransaction,
        api::builder::DefaultOpEvm,
        precompiles_xlayer::{NONCE_MANAGER_ADDRESS, aa_nonce_slot},
        transaction::{
            OpTransactionError,
            eip8130::{
                Eip8130AuthorizerValidation, Eip8130Call, Eip8130CodePlacement, Eip8130ConfigOp,
                Eip8130Parts, Eip8130SequenceUpdate, Eip8130StorageWrite, Eip8130VerifyCall,
                decode_phase_statuses,
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

    /// Builds an EVM with EIP-8130 parts and runs the full handler flow,
    /// returning both the run result and the constructed EVM so tests can
    /// inspect post-execution state.
    fn run_eip8130_tx(
        sender: Address,
        accounts: &[(Address, Bytecode)],
        storage: &[(Address, U256, U256)],
        tx_nonce: u64,
        eip8130: Eip8130Parts,
        gas_limit: u64,
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
        tx.eip8130 = eip8130;

        let ctx = Context::op()
            .with_db(db)
            .with_tx(tx)
            .with_chain(L1BlockInfo {
                l2_block: Some(U256::ZERO),
                operator_fee_scalar: Some(U256::ZERO),
                operator_fee_constant: Some(U256::ZERO),
                ..Default::default()
            })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::XLAYER_AA));
        let mut evm = ctx.build_op();

        let mut handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();
        let result = handler.run(&mut evm);
        (result, evm)
    }

    #[test]
    fn test_eip8130_empty_phases_reverts() {
        let sender = Address::from([0x11; 20]);
        let (result, _) = run_eip8130_tx(
            sender,
            &[],
            &[],
            0,
            Eip8130Parts { sender, payer: sender, ..Default::default() },
            100_000,
        );
        let result = result.unwrap();
        assert!(!result.is_success(), "empty phases = no successes = tx reverts");
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
                has_create_entry: true,
                code_placements: vec![Eip8130CodePlacement {
                    address: deployed_addr,
                    code: bytecode,
                }],
                ..Default::default()
            },
            100_000,
        );
        let result = result.unwrap();
        assert!(result.is_success(), "deploy-only tx should succeed");

        let statuses = decode_phase_statuses(result.output().unwrap());
        assert!(statuses.is_empty(), "no call phases = empty statuses");
    }

    #[test]
    fn test_eip8130_single_phase_success() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);

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
            100_000,
        );
        let result = result.unwrap();
        assert!(result.is_success(), "single STOP call should succeed");

        let statuses = decode_phase_statuses(result.output().unwrap());
        assert_eq!(statuses, vec![true]);
    }

    #[test]
    fn test_eip8130_single_phase_failure_reverts_tx() {
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
        );
        let result = result.unwrap();
        assert!(!result.is_success(), "all phases failed → tx reverts");
    }

    #[test]
    fn test_eip8130_single_phase_atomic_batch_failure_reverts_tx() {
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
    fn test_eip8130_mixed_phases_succeeds() {
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
        );
        let result = result.unwrap();
        assert!(result.is_success(), "at least one phase succeeded → tx succeeds");

        let statuses = decode_phase_statuses(result.output().unwrap());
        assert_eq!(statuses, vec![true, false]);
    }

    #[test]
    fn test_eip8130_all_phases_fail_reverts_tx() {
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
        );
        let result = result.unwrap();
        assert!(!result.is_success(), "all phases failed → tx reverts");
    }

    #[test]
    fn test_eip8130_gas_accounting() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);

        let aa_intrinsic = 25_000u64;
        let gas_limit = 100_000u64;
        let (result, _) = run_eip8130_tx(
            sender,
            &[(target, Bytecode::new_legacy(bytes!("00")))], // STOP
            &[],
            0,
            Eip8130Parts {
                sender,
                payer: sender,
                aa_intrinsic_gas: aa_intrinsic,
                call_phases: vec![vec![Eip8130Call {
                    to: target,
                    data: Bytes::new(),
                    value: U256::ZERO,
                }]],
                ..Default::default()
            },
            gas_limit,
        );
        let result = result.unwrap();
        assert!(result.is_success());
        assert!(result.gas_used() >= aa_intrinsic, "at least intrinsic gas should be charged");
        assert!(result.gas_used() <= gas_limit, "cannot spend more than limit");
    }

    #[test]
    fn test_eip8130_warm_nonce_reduces_intrinsic_gas() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);
        let nonce_seq: u64 = 5;
        let aa_intrinsic_cold = 40_000u64;
        let gas_limit = 200_000u64;
        let nonce_key = U256::ZERO;
        let slot = aa_nonce_slot(sender, nonce_key);

        let (result, _) = run_eip8130_tx(
            sender,
            &[(target, Bytecode::new_legacy(bytes!("00")))],
            &[(NONCE_MANAGER_ADDRESS, slot, U256::from(nonce_seq))],
            nonce_seq,
            Eip8130Parts {
                sender,
                payer: sender,
                nonce_key,
                aa_intrinsic_gas: aa_intrinsic_cold,
                call_phases: vec![vec![Eip8130Call {
                    to: target,
                    data: Bytes::new(),
                    value: U256::ZERO,
                }]],
                ..Default::default()
            },
            gas_limit,
        );
        let result = result.unwrap();

        assert!(result.is_success());
        let warm_intrinsic = aa_intrinsic_cold - NONCE_COLD_WARM_DELTA;
        assert!(
            result.gas_used() >= warm_intrinsic,
            "gas_used ({}) >= warm intrinsic gas ({})",
            result.gas_used(),
            warm_intrinsic,
        );
        assert!(
            result.gas_used() < aa_intrinsic_cold,
            "warm nonce should use less gas ({}) than cold ({})",
            result.gas_used(),
            aa_intrinsic_cold,
        );
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
            Eip8130Parts {
                sender,
                payer: sender,
                nonce_key,
                aa_intrinsic_gas: 25_000,
                ..Default::default()
            },
            200_000,
        );

        assert!(result.is_err(), "mismatched nonce should reject the transaction");
    }

    #[test]
    fn test_eip8130_expiry_rejected_at_inclusion() {
        let sender = Address::from([0x11; 20]);
        let mut db = InMemoryDB::default();
        db.insert_account_info(
            sender,
            AccountInfo { balance: U256::from(10_000_000), ..Default::default() },
        );
        db.insert_account_info(
            NONCE_MANAGER_ADDRESS,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("FE"))), ..Default::default() },
        );

        let mut tx = OpTransaction::builder()
            .base(
                TxEnv::builder()
                    .tx_type(Some(0x7B))
                    .caller(sender)
                    .gas_limit(200_000)
                    .kind(TxKind::Call(sender)),
            )
            .enveloped_tx(Some(bytes!("7BFACADE")))
            .build_fill();
        tx.eip8130 = Eip8130Parts {
            sender,
            payer: sender,
            expiry: 10,
            aa_intrinsic_gas: 25_000,
            ..Default::default()
        };

        let ctx = Context::op()
            .with_db(db)
            .with_tx(tx)
            .with_block(BlockEnv { timestamp: U256::from(11), ..Default::default() })
            .with_chain(L1BlockInfo {
                l2_block: Some(U256::ZERO),
                operator_fee_scalar: Some(U256::ZERO),
                operator_fee_constant: Some(U256::ZERO),
                ..Default::default()
            })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::XLAYER_AA));
        let mut evm = ctx.build_op();
        let mut handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();
        let result = handler.run(&mut evm);
        assert!(result.is_err(), "expired tx should be rejected at inclusion");
    }

    #[test]
    fn test_eip8130_too_many_calls_rejected_at_inclusion() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);

        let mut db = InMemoryDB::default();
        db.insert_account_info(
            sender,
            AccountInfo { balance: U256::from(10_000_000), ..Default::default() },
        );
        db.insert_account_info(
            NONCE_MANAGER_ADDRESS,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("FE"))), ..Default::default() },
        );

        let mut tx = OpTransaction::builder()
            .base(
                TxEnv::builder()
                    .tx_type(Some(0x7B))
                    .caller(sender)
                    .gas_limit(500_000)
                    .kind(TxKind::Call(sender)),
            )
            .enveloped_tx(Some(bytes!("7BFACADE")))
            .build_fill();
        tx.eip8130 = Eip8130Parts {
            sender,
            payer: sender,
            aa_intrinsic_gas: 25_000,
            call_phases: vec![vec![
                Eip8130Call {
                    to: target,
                    data: Bytes::new(),
                    value: U256::ZERO,
                };
                crate::constants::MAX_CALLS_PER_TX + 1
            ]],
            ..Default::default()
        };

        let ctx = Context::op()
            .with_db(db)
            .with_tx(tx)
            .with_chain(L1BlockInfo {
                l2_block: Some(U256::ZERO),
                operator_fee_scalar: Some(U256::ZERO),
                operator_fee_constant: Some(U256::ZERO),
                ..Default::default()
            })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::XLAYER_AA));
        let mut evm = ctx.build_op();
        let mut handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();
        let result = handler.run(&mut evm);
        assert!(result.is_err(), "tx with >100 calls should be rejected at inclusion");
    }

    #[test]
    fn test_eip8130_too_many_account_changes_rejected_at_inclusion() {
        let sender = Address::from([0x11; 20]);

        let mut db = InMemoryDB::default();
        db.insert_account_info(
            sender,
            AccountInfo { balance: U256::from(10_000_000), ..Default::default() },
        );
        db.insert_account_info(
            NONCE_MANAGER_ADDRESS,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("FE"))), ..Default::default() },
        );

        let mut tx = OpTransaction::builder()
            .base(
                TxEnv::builder()
                    .tx_type(Some(0x7B))
                    .caller(sender)
                    .gas_limit(500_000)
                    .kind(TxKind::Call(sender)),
            )
            .enveloped_tx(Some(bytes!("7BFACADE")))
            .build_fill();
        tx.eip8130 = Eip8130Parts {
            sender,
            payer: sender,
            aa_intrinsic_gas: 25_000,
            account_change_units: crate::constants::MAX_ACCOUNT_CHANGES_PER_TX + 1,
            ..Default::default()
        };

        let ctx = Context::op()
            .with_db(db)
            .with_tx(tx)
            .with_chain(L1BlockInfo {
                l2_block: Some(U256::ZERO),
                operator_fee_scalar: Some(U256::ZERO),
                operator_fee_constant: Some(U256::ZERO),
                ..Default::default()
            })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::XLAYER_AA));
        let mut evm = ctx.build_op();
        let mut handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();
        let result = handler.run(&mut evm);
        assert!(result.is_err(), "tx with >10 account changes should be rejected at inclusion");
    }

    #[test]
    fn test_eip8130_locked_config_change_rejected_at_inclusion() {
        let sender = Address::from([0x11; 20]);
        let mut db = InMemoryDB::default();
        db.insert_account_info(
            sender,
            AccountInfo { balance: U256::from(10_000_000), ..Default::default() },
        );
        db.insert_account_info(
            NONCE_MANAGER_ADDRESS,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("FE"))), ..Default::default() },
        );
        db.insert_account_info(ACCOUNT_CONFIG_ADDRESS, AccountInfo::default());

        let lock_slot = aa_lock_slot(sender);
        let account_cfg = db.load_account(ACCOUNT_CONFIG_ADDRESS).unwrap();
        account_cfg.storage.insert(lock_slot, pack_lock_state(true));

        let mut tx = OpTransaction::builder()
            .base(
                TxEnv::builder()
                    .tx_type(Some(0x7B))
                    .caller(sender)
                    .gas_limit(200_000)
                    .kind(TxKind::Call(sender)),
            )
            .enveloped_tx(Some(bytes!("7BFACADE")))
            .build_fill();
        tx.eip8130 = Eip8130Parts {
            sender,
            payer: sender,
            aa_intrinsic_gas: 25_000,
            config_writes: vec![Eip8130StorageWrite {
                address: ACCOUNT_CONFIG_ADDRESS,
                slot: U256::from(1),
                value: U256::from(2),
            }],
            ..Default::default()
        };

        let ctx = Context::op()
            .with_db(db)
            .with_tx(tx)
            .with_chain(L1BlockInfo {
                l2_block: Some(U256::ZERO),
                operator_fee_scalar: Some(U256::ZERO),
                operator_fee_constant: Some(U256::ZERO),
                ..Default::default()
            })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::XLAYER_AA));
        let mut evm = ctx.build_op();
        let mut handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();
        let result = handler.run(&mut evm);
        assert!(result.is_err(), "locked account should reject config change inclusion");
    }

    #[test]
    fn test_eip8130_config_sequence_mismatch_rejected_at_inclusion() {
        let sender = Address::from([0x11; 20]);
        let mut db = InMemoryDB::default();
        db.insert_account_info(
            sender,
            AccountInfo { balance: U256::from(10_000_000), ..Default::default() },
        );
        db.insert_account_info(
            NONCE_MANAGER_ADDRESS,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("FE"))), ..Default::default() },
        );
        db.insert_account_info(ACCOUNT_CONFIG_ADDRESS, AccountInfo::default());

        let lock_slot = aa_lock_slot(sender);
        let seq_slot = U256::from(0x1234_u64);
        let account_cfg = db.load_account(ACCOUNT_CONFIG_ADDRESS).unwrap();
        account_cfg.storage.insert(lock_slot, pack_lock_state(false));
        account_cfg.storage.insert(seq_slot, pack_sequences(0, 5));

        let mut tx = OpTransaction::builder()
            .base(
                TxEnv::builder()
                    .tx_type(Some(0x7B))
                    .caller(sender)
                    .gas_limit(200_000)
                    .kind(TxKind::Call(sender)),
            )
            .enveloped_tx(Some(bytes!("7BFACADE")))
            .build_fill();
        tx.eip8130 = Eip8130Parts {
            sender,
            payer: sender,
            aa_intrinsic_gas: 25_000,
            sequence_updates: vec![Eip8130SequenceUpdate {
                slot: seq_slot,
                is_multichain: false,
                new_value: 3, // tx sequence = 2, but expected local is 5
            }],
            ..Default::default()
        };

        let ctx = Context::op()
            .with_db(db)
            .with_tx(tx)
            .with_chain(L1BlockInfo {
                l2_block: Some(U256::ZERO),
                operator_fee_scalar: Some(U256::ZERO),
                operator_fee_constant: Some(U256::ZERO),
                ..Default::default()
            })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::XLAYER_AA));
        let mut evm = ctx.build_op();
        let mut handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();
        let result = handler.run(&mut evm);
        assert!(result.is_err(), "sequence mismatch should reject config change inclusion");
    }

    #[test]
    fn test_eip8130_sequence_update_preserves_lock_fields_at_inclusion() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);
        let mut db = InMemoryDB::default();
        db.insert_account_info(
            sender,
            AccountInfo { balance: U256::from(10_000_000), ..Default::default() },
        );
        db.insert_account_info(
            NONCE_MANAGER_ADDRESS,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("FE"))), ..Default::default() },
        );
        db.insert_account_info(
            target,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("00"))), ..Default::default() },
        );
        db.insert_account_info(
            ACCOUNT_CONFIG_ADDRESS,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("FE"))), ..Default::default() },
        );

        let state_slot = aa_lock_slot(sender);
        let initial = pack_account_state(5, 9, 0, 600);
        let account_cfg = db.load_account(ACCOUNT_CONFIG_ADDRESS).unwrap();
        account_cfg.storage.insert(state_slot, initial);

        let mut tx = OpTransaction::builder()
            .base(
                TxEnv::builder()
                    .tx_type(Some(0x7B))
                    .caller(sender)
                    .gas_limit(200_000)
                    .kind(TxKind::Call(sender)),
            )
            .enveloped_tx(Some(bytes!("7BFACADE")))
            .build_fill();
        tx.eip8130 = Eip8130Parts {
            sender,
            payer: sender,
            aa_intrinsic_gas: 25_000,
            sequence_updates: vec![Eip8130SequenceUpdate {
                slot: state_slot,
                is_multichain: true,
                new_value: 6, // tx sequence = 5, matches initial multichain sequence
            }],
            call_phases: vec![vec![Eip8130Call {
                to: target,
                data: Bytes::new(),
                value: U256::ZERO,
            }]],
            ..Default::default()
        };

        let ctx = Context::op()
            .with_db(db)
            .with_tx(tx)
            .with_chain(L1BlockInfo {
                l2_block: Some(U256::ZERO),
                operator_fee_scalar: Some(U256::ZERO),
                operator_fee_constant: Some(U256::ZERO),
                ..Default::default()
            })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::XLAYER_AA));
        let mut evm = ctx.build_op();
        let mut handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();
        let result = handler.run(&mut evm).expect("tx should execute");
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
    fn test_eip8130_sender_auth_rejected_when_revoked_in_same_tx() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);
        let k1_verifier = K1_VERIFIER_ADDRESS;
        let new_verifier = Address::from([0x44; 20]);
        let revoked_verifier = REVOKED_VERIFIER;

        let mut implicit_owner = [0u8; 32];
        implicit_owner[..20].copy_from_slice(sender.as_slice());
        let eoa_owner_id = B256::from(implicit_owner);
        let new_owner_id = B256::from([0x55; 32]);

        let eoa_owner_slot = aa_owner_config_slot(sender, U256::from_be_bytes(eoa_owner_id.0));
        let new_owner_slot = aa_owner_config_slot(sender, U256::from_be_bytes(new_owner_id.0));

        let mut db = InMemoryDB::default();
        db.insert_account_info(
            sender,
            AccountInfo { balance: U256::from(10_000_000), ..Default::default() },
        );
        db.insert_account_info(
            NONCE_MANAGER_ADDRESS,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("FE"))), ..Default::default() },
        );
        db.insert_account_info(
            target,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("00"))), ..Default::default() },
        );
        db.insert_account_info(
            ACCOUNT_CONFIG_ADDRESS,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("FE"))), ..Default::default() },
        );

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
            sender_verifier: k1_verifier,
            owner_id: eoa_owner_id,
            call_phases: vec![vec![Eip8130Call {
                to: target,
                data: Bytes::new(),
                value: U256::ZERO,
            }]],
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
                    slot: new_owner_slot,
                    value: pack_owner_config(new_verifier, 0),
                },
                Eip8130StorageWrite {
                    address: ACCOUNT_CONFIG_ADDRESS,
                    slot: eoa_owner_slot,
                    value: pack_owner_config(revoked_verifier, 0),
                },
            ],
            ..Default::default()
        };

        let ctx = Context::op()
            .with_db(db)
            .with_tx(tx)
            .with_chain(L1BlockInfo {
                l2_block: Some(U256::ZERO),
                operator_fee_scalar: Some(U256::ZERO),
                operator_fee_constant: Some(U256::ZERO),
                ..Default::default()
            })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::XLAYER_AA));
        let mut evm = ctx.build_op();

        let mut handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();
        let result = handler.run(&mut evm);
        assert!(
            result.is_err(),
            "sender auth signed by owner revoked in the same tx must be rejected",
        );
    }

    #[test]
    fn test_eip8130_sender_auth_accepts_owner_added_in_same_tx() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x22; 20]);
        let k1_verifier = K1_VERIFIER_ADDRESS;
        let new_verifier = Address::from([0x44; 20]);
        let revoked_verifier = REVOKED_VERIFIER;

        let mut implicit_owner = [0u8; 32];
        implicit_owner[..20].copy_from_slice(sender.as_slice());
        let eoa_owner_id = B256::from(implicit_owner);
        let new_owner_id = B256::from([0x55; 32]);

        let eoa_owner_slot = aa_owner_config_slot(sender, U256::from_be_bytes(eoa_owner_id.0));
        let new_owner_slot = aa_owner_config_slot(sender, U256::from_be_bytes(new_owner_id.0));

        let mut db = InMemoryDB::default();
        db.insert_account_info(
            sender,
            AccountInfo { balance: U256::from(10_000_000), ..Default::default() },
        );
        db.insert_account_info(
            NONCE_MANAGER_ADDRESS,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("FE"))), ..Default::default() },
        );
        db.insert_account_info(
            target,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("00"))), ..Default::default() },
        );
        db.insert_account_info(
            ACCOUNT_CONFIG_ADDRESS,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("FE"))), ..Default::default() },
        );

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
            sender_verifier: new_verifier,
            owner_id: new_owner_id,
            call_phases: vec![vec![Eip8130Call {
                to: target,
                data: Bytes::new(),
                value: U256::ZERO,
            }]],
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
                    slot: new_owner_slot,
                    value: pack_owner_config(new_verifier, 0),
                },
                Eip8130StorageWrite {
                    address: ACCOUNT_CONFIG_ADDRESS,
                    slot: eoa_owner_slot,
                    value: pack_owner_config(revoked_verifier, 0),
                },
            ],
            ..Default::default()
        };

        let ctx = Context::op()
            .with_db(db)
            .with_tx(tx)
            .with_chain(L1BlockInfo {
                l2_block: Some(U256::ZERO),
                operator_fee_scalar: Some(U256::ZERO),
                operator_fee_constant: Some(U256::ZERO),
                ..Default::default()
            })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::XLAYER_AA));
        let mut evm = ctx.build_op();

        let mut handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();
        let result = handler.run(&mut evm).expect("tx should execute");
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
        let revoked_verifier = REVOKED_VERIFIER;

        let mut implicit_owner = [0u8; 32];
        implicit_owner[..20].copy_from_slice(sender.as_slice());
        let eoa_owner_id = B256::from(implicit_owner);
        let new_owner_id = B256::from([0x55; 32]);

        let eoa_owner_slot = aa_owner_config_slot(sender, U256::from_be_bytes(eoa_owner_id.0));
        let new_owner_slot = aa_owner_config_slot(sender, U256::from_be_bytes(new_owner_id.0));

        let mut db = InMemoryDB::default();
        db.insert_account_info(
            sender,
            AccountInfo { balance: U256::from(10_000_000), ..Default::default() },
        );
        db.insert_account_info(
            NONCE_MANAGER_ADDRESS,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("FE"))), ..Default::default() },
        );
        db.insert_account_info(
            target,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("00"))), ..Default::default() },
        );
        db.insert_account_info(
            ACCOUNT_CONFIG_ADDRESS,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("FE"))), ..Default::default() },
        );

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
            sender_verifier: new_verifier,
            owner_id: new_owner_id,
            call_phases: vec![vec![Eip8130Call {
                to: target,
                data: Bytes::new(),
                value: U256::ZERO,
            }]],
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
                    slot: new_owner_slot,
                    value: pack_owner_config(new_verifier, 0),
                },
                Eip8130StorageWrite {
                    address: ACCOUNT_CONFIG_ADDRESS,
                    slot: eoa_owner_slot,
                    value: pack_owner_config(revoked_verifier, 0),
                },
            ],
            ..Default::default()
        };

        let ctx = Context::op()
            .with_db(db)
            .with_tx(tx)
            .with_chain(L1BlockInfo {
                l2_block: Some(U256::ZERO),
                operator_fee_scalar: Some(U256::ZERO),
                operator_fee_constant: Some(U256::ZERO),
                ..Default::default()
            })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::XLAYER_AA));
        let mut evm = ctx.build_op();

        let mut handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();
        let result = handler.run(&mut evm);
        assert!(result.is_err(), "authorizer verifier field mismatch must reject inclusion",);
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

        let mut db = InMemoryDB::default();
        db.insert_account_info(
            sender,
            AccountInfo { balance: U256::from(10_000_000), ..Default::default() },
        );
        db.insert_account_info(
            NONCE_MANAGER_ADDRESS,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("FE"))), ..Default::default() },
        );
        db.insert_account_info(
            custom_verifier,
            AccountInfo {
                code: Some(make_verifier_bytecode(authorizer_owner_id)),
                ..Default::default()
            },
        );
        db.insert_account_info(
            target,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("00"))), ..Default::default() },
        );
        db.insert_account_info(ACCOUNT_CONFIG_ADDRESS, AccountInfo::default());
        let acfg = db.load_account(ACCOUNT_CONFIG_ADDRESS).unwrap();
        acfg.storage.insert(owner_config_slot, pack_owner_config(custom_verifier, 0));

        let mut tx = OpTransaction::builder()
            .base(
                TxEnv::builder()
                    .tx_type(Some(0x7B))
                    .caller(sender)
                    .gas_limit(250_000)
                    .kind(TxKind::Call(sender)),
            )
            .enveloped_tx(Some(bytes!("7BFACADE")))
            .build_fill();
        tx.eip8130 = Eip8130Parts {
            sender,
            payer: sender,
            sender_verifier: K1_VERIFIER_ADDRESS,
            owner_id: eoa_owner_id,
            custom_verifier_gas_cap: 100_000,
            call_phases: vec![vec![Eip8130Call {
                to: target,
                data: Bytes::new(),
                value: U256::ZERO,
            }]],
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
        };

        let ctx = Context::op()
            .with_db(db)
            .with_tx(tx)
            .with_chain(L1BlockInfo {
                l2_block: Some(U256::ZERO),
                operator_fee_scalar: Some(U256::ZERO),
                operator_fee_constant: Some(U256::ZERO),
                ..Default::default()
            })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::XLAYER_AA));
        let mut evm = ctx.build_op();

        let mut handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();
        let result = handler.run(&mut evm).expect("tx should execute");
        assert!(result.is_success(), "custom authorizer STATICCALL should succeed at inclusion",);
    }

    #[test]
    fn test_eip8130_owner_id_visible_through_tx_context() {
        let sender = Address::from([0x11; 20]);
        let target = Address::from([0x44; 20]);
        let owner_id = B256::from([0xAB; 32]);

        // Runtime for OwnerIdProbe:
        // - probe(): reads TxContext.getOwnerId() and stores it at slot 0
        // - lastOwnerId(): returns slot 0
        let probe_runtime = Bytecode::new_legacy(bytes!(
            "608060405234801561000f575f5ffd5b5060043610610034575f3560e01c80634320a6cb14610038578063b74af5a914610056575b5f5ffd5b610040610074565b60405161004d9190610111565b60405180910390f35b61005e610079565b60405161006b9190610111565b60405180910390f35b5f5481565b5f5f61aa0373ffffffffffffffffffffffffffffffffffffffff16631f5072f26040518163ffffffff1660e01b8152600401602060405180830381865afa1580156100c6573d5f5f3e3d5ffd5b505050506040513d601f19601f820116820180604052508101906100ea9190610158565b9050805f819055508091505090565b5f819050919050565b61010b816100f9565b82525050565b5f6020820190506101245f830184610102565b92915050565b5f5ffd5b610137816100f9565b8114610141575f5ffd5b50565b5f815190506101528161012e565b92915050565b5f6020828403121561016d5761016c61012a565b5b5f61017a84828501610144565b9150509291505056fea26469706673582212203ca48096bb84d6eb04b36713b485cfdc832bcb25ec90dc9b384decb8a8ba23ee64736f6c63430008210033"
        ));

        let mut db = InMemoryDB::default();
        db.insert_account_info(
            sender,
            AccountInfo { balance: U256::from(10_000_000), ..Default::default() },
        );
        db.insert_account_info(
            NONCE_MANAGER_ADDRESS,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("FE"))), ..Default::default() },
        );
        db.insert_account_info(
            target,
            AccountInfo { code: Some(probe_runtime), ..Default::default() },
        );

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
            owner_id,
            call_phases: vec![vec![Eip8130Call {
                to: target,
                data: bytes!("b74af5a9"), // probe()
                value: U256::ZERO,
            }]],
            ..Default::default()
        };

        let ctx = Context::op()
            .with_db(db)
            .with_tx(tx)
            .with_chain(L1BlockInfo {
                l2_block: Some(U256::ZERO),
                operator_fee_scalar: Some(U256::ZERO),
                operator_fee_constant: Some(U256::ZERO),
                ..Default::default()
            })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::XLAYER_AA));
        let mut evm = ctx.build_op();

        let mut handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();
        let result = handler.run(&mut evm).unwrap();
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

    /// Builds bytecode that returns a fixed 32-byte value (owner_id).
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
    /// AccountConfig's owner_config mapping.
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
        let packed_config = pack_owner_config(verifier, 0x00); // scope=0 = unrestricted

        let mut db = InMemoryDB::default();
        db.insert_account_info(
            sender,
            AccountInfo { balance: U256::from(10_000_000), ..Default::default() },
        );
        db.insert_account_info(
            NONCE_MANAGER_ADDRESS,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("FE"))), ..Default::default() },
        );
        db.insert_account_info(
            verifier,
            AccountInfo { code: Some(make_verifier_bytecode(owner_id)), ..Default::default() },
        );
        db.insert_account_info(
            target,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("00"))), ..Default::default() },
        );
        db.insert_account_info(ACCOUNT_CONFIG_ADDRESS, AccountInfo::default());
        let acfg = db.load_account(ACCOUNT_CONFIG_ADDRESS).unwrap();
        acfg.storage.insert(owner_config_slot, packed_config);

        let calldata = Bytes::from(vec![0xCA; 36]); // dummy calldata

        let mut tx = OpTransaction::builder()
            .base(
                TxEnv::builder()
                    .tx_type(Some(0x7B))
                    .caller(sender)
                    .gas_limit(200_000)
                    .kind(TxKind::Call(sender)),
            )
            .enveloped_tx(Some(bytes!("7BFACADE")))
            .build_fill();
        tx.eip8130 = Eip8130Parts {
            sender,
            payer: sender,
            custom_verifier_gas_cap: 100_000,
            sender_verifier: Address::ZERO, // custom
            call_phases: vec![vec![Eip8130Call {
                to: target,
                data: Bytes::new(),
                value: U256::ZERO,
            }]],
            sender_verify_call: Some(Eip8130VerifyCall {
                verifier,
                calldata,
                account: sender,
                required_scope: 0x02, // SENDER
            }),
            ..Default::default()
        };

        let ctx = Context::op()
            .with_db(db)
            .with_tx(tx)
            .with_chain(L1BlockInfo {
                l2_block: Some(U256::ZERO),
                operator_fee_scalar: Some(U256::ZERO),
                operator_fee_constant: Some(U256::ZERO),
                ..Default::default()
            })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::XLAYER_AA));
        let mut evm = ctx.build_op();

        let mut handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();
        let result = handler.run(&mut evm).unwrap();
        assert!(result.is_success(), "custom verifier STATICCALL should succeed");

        let statuses = decode_phase_statuses(result.output().unwrap());
        assert_eq!(statuses, vec![true]);
    }

    #[test]
    fn test_eip8130_custom_verifier_wrong_verifier_fails() {
        let sender = Address::from([0x11; 20]);
        let verifier = Address::from([0xAA; 20]);
        let wrong_verifier = Address::from([0xCC; 20]); // different from expected
        let owner_id = B256::from([0xBB; 32]);

        let owner_config_slot = aa_owner_config_slot(sender, U256::from_be_bytes(owner_id.0));
        // Store a DIFFERENT verifier in owner_config than what the tx specifies
        let packed_config = pack_owner_config(wrong_verifier, 0x00);

        let mut db = InMemoryDB::default();
        db.insert_account_info(
            sender,
            AccountInfo { balance: U256::from(10_000_000), ..Default::default() },
        );
        db.insert_account_info(
            NONCE_MANAGER_ADDRESS,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("FE"))), ..Default::default() },
        );
        db.insert_account_info(
            verifier,
            AccountInfo { code: Some(make_verifier_bytecode(owner_id)), ..Default::default() },
        );
        db.insert_account_info(ACCOUNT_CONFIG_ADDRESS, AccountInfo::default());
        let acfg = db.load_account(ACCOUNT_CONFIG_ADDRESS).unwrap();
        acfg.storage.insert(owner_config_slot, packed_config);

        let calldata = Bytes::from(vec![0xCA; 36]);

        let mut tx = OpTransaction::builder()
            .base(
                TxEnv::builder()
                    .tx_type(Some(0x7B))
                    .caller(sender)
                    .gas_limit(200_000)
                    .kind(TxKind::Call(sender)),
            )
            .enveloped_tx(Some(bytes!("7BFACADE")))
            .build_fill();
        tx.eip8130 = Eip8130Parts {
            sender,
            payer: sender,
            custom_verifier_gas_cap: 100_000,
            sender_verifier: Address::ZERO,
            call_phases: vec![],
            sender_verify_call: Some(Eip8130VerifyCall {
                verifier,
                calldata,
                account: sender,
                required_scope: 0x02,
            }),
            ..Default::default()
        };

        let ctx = Context::op()
            .with_db(db)
            .with_tx(tx)
            .with_chain(L1BlockInfo {
                l2_block: Some(U256::ZERO),
                operator_fee_scalar: Some(U256::ZERO),
                operator_fee_constant: Some(U256::ZERO),
                ..Default::default()
            })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::XLAYER_AA));
        let mut evm = ctx.build_op();

        let mut handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();
        let result = handler.run(&mut evm);
        assert!(result.is_err(), "mismatched verifier should cause an error");
    }

    #[test]
    fn test_eip8130_custom_verifier_wrong_scope_fails() {
        let sender = Address::from([0x11; 20]);
        let verifier = Address::from([0xAA; 20]);
        let owner_id = B256::from([0xBB; 32]);

        let owner_config_slot = aa_owner_config_slot(sender, U256::from_be_bytes(owner_id.0));
        // Scope = PAYER (0x04), but required is SENDER (0x02) → should fail
        let packed_config = pack_owner_config(verifier, 0x04);

        let mut db = InMemoryDB::default();
        db.insert_account_info(
            sender,
            AccountInfo { balance: U256::from(10_000_000), ..Default::default() },
        );
        db.insert_account_info(
            NONCE_MANAGER_ADDRESS,
            AccountInfo { code: Some(Bytecode::new_legacy(bytes!("FE"))), ..Default::default() },
        );
        db.insert_account_info(
            verifier,
            AccountInfo { code: Some(make_verifier_bytecode(owner_id)), ..Default::default() },
        );
        db.insert_account_info(ACCOUNT_CONFIG_ADDRESS, AccountInfo::default());
        let acfg = db.load_account(ACCOUNT_CONFIG_ADDRESS).unwrap();
        acfg.storage.insert(owner_config_slot, packed_config);

        let calldata = Bytes::from(vec![0xCA; 36]);

        let mut tx = OpTransaction::builder()
            .base(
                TxEnv::builder()
                    .tx_type(Some(0x7B))
                    .caller(sender)
                    .gas_limit(200_000)
                    .kind(TxKind::Call(sender)),
            )
            .enveloped_tx(Some(bytes!("7BFACADE")))
            .build_fill();
        tx.eip8130 = Eip8130Parts {
            sender,
            payer: sender,
            custom_verifier_gas_cap: 100_000,
            sender_verifier: Address::ZERO,
            call_phases: vec![],
            sender_verify_call: Some(Eip8130VerifyCall {
                verifier,
                calldata,
                account: sender,
                required_scope: 0x02, // SENDER
            }),
            ..Default::default()
        };

        let ctx = Context::op()
            .with_db(db)
            .with_tx(tx)
            .with_chain(L1BlockInfo {
                l2_block: Some(U256::ZERO),
                operator_fee_scalar: Some(U256::ZERO),
                operator_fee_constant: Some(U256::ZERO),
                ..Default::default()
            })
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::XLAYER_AA));
        let mut evm = ctx.build_op();

        let mut handler =
            OpHandler::<_, EVMError<_, OpTransactionError>, EthFrame<EthInterpreter>>::new();
        let result = handler.run(&mut evm);
        assert!(result.is_err(), "wrong scope should cause an error");
    }
}
