//!Handler related to Optimism chain
use crate::{
    L1BlockInfo, OpHaltReason, OpSpecId,
    api::exec::OpContextTr,
    constants::{BASE_FEE_RECIPIENT, L1_FEE_RECIPIENT, OPERATOR_FEE_RECIPIENT},
    transaction::{OpTransactionError, OpTxTr, deposit::DEPOSIT_TRANSACTION_TYPE},
};
use revm::{
    context::{
        LocalContextTr,
        journaled_state::{JournalCheckpoint, account::JournaledAccountTr},
        result::InvalidTransaction,
    },
    context_interface::{
        Block, Cfg, ContextTr, JournalTr, Transaction,
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
        Gas, InitialAndFloorGas, interpreter::EthInterpreter, interpreter_action::FrameInit,
    },
    primitives::{U256, hardfork::SpecId},
};
use std::{boxed::Box, vec::Vec};

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

        // AA transactions require NATIVE_AA. Reject if the spec is not active.
        if tx_type == crate::handler_aa_helpers::EIP8130_TX_TYPE {
            if !evm.ctx().cfg().spec().is_enabled_in(OpSpecId::NATIVE_AA) {
                return Err(OpTransactionError::Base(InvalidTransaction::Str(
                    "EIP-8130 AA transactions require NATIVE_AA".into(),
                ))
                .into());
            }

            let ctx = evm.ctx();

            if !ctx.cfg().is_base_fee_check_disabled() {
                let basefee = ctx.block().basefee() as u128;
                let max_fee = ctx.tx().max_fee_per_gas();
                let max_priority = ctx.tx().max_priority_fee_per_gas().unwrap_or(0);

                if max_fee < basefee {
                    return Err(OpTransactionError::Base(InvalidTransaction::Str(
                        "EIP-8130: max_fee_per_gas below base fee".into(),
                    ))
                    .into());
                }
                if max_priority > max_fee {
                    return Err(OpTransactionError::Base(InvalidTransaction::Str(
                        "EIP-8130: max_priority_fee_per_gas exceeds max_fee_per_gas".into(),
                    ))
                    .into());
                }
            }

            // Inclusion-time expiry check (defense-in-depth against bypassing
            // mempool admission).
            let expiry = ctx.tx().eip8130_parts().expiry;
            if expiry != 0 {
                let block_ts = ctx.block().timestamp().saturating_to::<u64>();
                if block_ts > expiry {
                    return Err(OpTransactionError::Base(InvalidTransaction::Str(
                        format!(
                            "EIP-8130: transaction expired (expiry={expiry}, current={block_ts})"
                        )
                        .into(),
                    ))
                    .into());
                }
            }

            // Inclusion-time structural guard for phased calls.
            let total_calls: usize =
                ctx.tx().eip8130_parts().call_phases.iter().map(Vec::len).sum();
            if total_calls > crate::constants::MAX_CALLS_PER_TX {
                return Err(OpTransactionError::Base(InvalidTransaction::Str(
                    format!(
                        "EIP-8130: too many calls ({total_calls} > {})",
                        crate::constants::MAX_CALLS_PER_TX
                    )
                    .into(),
                ))
                .into());
            }

            let total_account_changes = ctx.tx().eip8130_parts().account_change_units;
            if total_account_changes > crate::constants::MAX_ACCOUNT_CHANGES_PER_TX {
                return Err(OpTransactionError::Base(InvalidTransaction::Str(
                    format!(
                        "EIP-8130: too many account changes ({total_account_changes} > {})",
                        crate::constants::MAX_ACCOUNT_CHANGES_PER_TX
                    )
                    .into(),
                ))
                .into());
            }

            return Ok(());
        }

        self.mainnet.validate_env(evm)
    }

    fn validate_initial_tx_gas(
        &self,
        evm: &mut Self::Evm,
    ) -> Result<InitialAndFloorGas, Self::Error> {
        if evm.ctx().tx().tx_type() == crate::handler_aa_helpers::EIP8130_TX_TYPE {
            let ctx = evm.ctx();
            let parts = ctx.tx().eip8130_parts();
            let aa_gas = parts.aa_intrinsic_gas;
            let calldata_overhead =
                crate::handler_aa_helpers::estimation_calldata_overhead(parts);
            let is_estimation = ctx.cfg().is_base_fee_check_disabled();
            let gas_limit = ctx.tx().gas_limit();

            let effective_gas =
                if is_estimation { aa_gas + calldata_overhead } else { aa_gas };

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

        // Clear any stale EIP-8130 thread-local context BEFORE any tx-type branch.
        // Without this, a deposit (or any non-AA tx) following an AA tx would see
        // the previous AA tx's sender / owner_id / max_cost / call_phases via the
        // TxContext precompile (0x...aa03), even though the precompile is gated by
        // `aa_context = (tx.tx_type() == EIP8130_TX_TYPE)` in OpPrecompiles::run.
        // We still defend in depth here so the thread-local cannot leak across
        // transaction boundaries.
        crate::precompiles::clear_eip8130_tx_context();

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

        // AA transactions: deduct gas from payer, increment NonceManager nonce,
        // auto-delegate bare EOAs, and apply pre-execution storage writes.
        if tx.tx_type() == crate::handler_aa_helpers::EIP8130_TX_TYPE {
            let sender = tx.caller();
            let nonce_sequence = tx.nonce();
            let eip8130 = tx.eip8130_parts().clone();

            {
                let execution_gas_limit = tx.gas_limit().saturating_sub(eip8130.aa_intrinsic_gas);
                let known_intrinsic =
                    eip8130.aa_intrinsic_gas.saturating_sub(eip8130.payer_intrinsic_gas);
                crate::precompiles::set_eip8130_tx_context(
                    crate::precompiles::Eip8130TxContext::new(
                        &eip8130,
                        execution_gas_limit,
                        known_intrinsic,
                        U256::from(tx.max_fee_per_gas()),
                    ),
                );
            }

            // --- Gas deduction from payer ---
            let payer = eip8130.payer;
            let mut payer_account = journal.load_account_with_code_mut(payer)?.data;
            let mut balance = payer_account.account().info.balance;

            if !cfg.is_fee_charge_disabled() {
                let Some(additional_cost) = chain.tx_cost_with_tx(tx, spec) else {
                    return Err(ERROR::from_string(
                        "[OPTIMISM] Failed to load enveloped transaction.".into(),
                    ));
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
            let sender_has_code =
                sender_account.account().info.code_hash != revm::primitives::keccak256([]);
            drop(sender_account);

            // --- Nonce validation and increment in NonceManager ---
            let nonce_key = eip8130.nonce_key;
            if nonce_key != crate::handler_aa_helpers::NONCE_KEY_MAX {
                let slot = crate::handler_aa_helpers::aa_nonce_slot(sender, nonce_key);

                journal.load_account(crate::precompiles::NONCE_MANAGER_ADDRESS)?;
                let current_seq =
                    journal.sload(crate::precompiles::NONCE_MANAGER_ADDRESS, slot)?.data;

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
                journal.sstore(crate::precompiles::NONCE_MANAGER_ADDRESS, slot, next_seq)?;
            } else {
                // --- Expiring-nonce circular buffer (nonce-free mode) ---
                //
                // On-chain replay protection: a fixed-capacity ring buffer
                // stores recent nonce-free tx hashes keyed by expiry. Before
                // including a new nonce-free tx the handler checks that no
                // active (unexpired) entry with the same hash exists, evicts
                // the oldest entry if it has expired, and inserts the new one.
                let now: u64 = block.timestamp().to();
                let expiry = eip8130.expiry;
                let skip_checks = cfg.is_nonce_check_disabled() || cfg.is_base_fee_check_disabled();

                if !skip_checks {
                    if expiry <= now
                        || expiry > now + crate::handler_aa_helpers::NONCE_FREE_MAX_EXPIRY_WINDOW
                    {
                        return Err(ERROR::from_string(format!(
                            "nonce-free expiry out of window: expiry={expiry}, now={now}"
                        )));
                    }
                }

                let nf_hash = eip8130.nonce_free_hash.unwrap_or_default();
                journal.load_account(crate::precompiles::NONCE_MANAGER_ADDRESS)?;

                // Replay check
                let seen_slot = crate::handler_aa_helpers::aa_expiring_seen_slot(nf_hash);
                let seen_expiry = journal
                    .sload(crate::precompiles::NONCE_MANAGER_ADDRESS, seen_slot)?
                    .data;
                if !skip_checks && seen_expiry != U256::ZERO && seen_expiry > U256::from(now) {
                    return Err(ERROR::from_string(
                        "nonce-free transaction replay: hash already seen".into(),
                    ));
                }

                // Read ring buffer pointer
                let ptr_raw = journal
                    .sload(
                        crate::precompiles::NONCE_MANAGER_ADDRESS,
                        crate::handler_aa_helpers::EXPIRING_RING_PTR_SLOT,
                    )?
                    .data;
                let idx = ptr_raw.as_limbs()[0] as u32
                    % crate::handler_aa_helpers::EXPIRING_NONCE_SET_CAPACITY;

                // Read existing entry at this ring position
                let ring_slot = crate::handler_aa_helpers::aa_expiring_ring_slot(idx);
                let old_hash_raw = journal
                    .sload(crate::precompiles::NONCE_MANAGER_ADDRESS, ring_slot)?
                    .data;

                // Evict old entry if present (must be expired)
                if old_hash_raw != U256::ZERO {
                    let old_hash = revm::primitives::B256::from(old_hash_raw.to_be_bytes::<32>());
                    let old_seen_slot = crate::handler_aa_helpers::aa_expiring_seen_slot(old_hash);
                    let old_expiry = journal
                        .sload(crate::precompiles::NONCE_MANAGER_ADDRESS, old_seen_slot)?
                        .data;
                    if !skip_checks
                        && old_expiry != U256::ZERO
                        && old_expiry > U256::from(now)
                    {
                        return Err(ERROR::from_string(
                            "nonce-free buffer full: cannot evict unexpired entry".into(),
                        ));
                    }
                    journal.sstore(
                        crate::precompiles::NONCE_MANAGER_ADDRESS,
                        old_seen_slot,
                        U256::ZERO,
                    )?;
                }

                // Insert new entry into ring + seen set
                journal.sstore(
                    crate::precompiles::NONCE_MANAGER_ADDRESS,
                    ring_slot,
                    U256::from_be_bytes(nf_hash.0),
                )?;
                journal.sstore(
                    crate::precompiles::NONCE_MANAGER_ADDRESS,
                    seen_slot,
                    U256::from(expiry),
                )?;

                // Advance pointer (wraps at capacity)
                let next_ptr =
                    if idx + 1 >= crate::handler_aa_helpers::EXPIRING_NONCE_SET_CAPACITY {
                        U256::ZERO
                    } else {
                        U256::from(idx + 1)
                    };
                journal.sstore(
                    crate::precompiles::NONCE_MANAGER_ADDRESS,
                    crate::handler_aa_helpers::EXPIRING_RING_PTR_SLOT,
                    next_ptr,
                )?;
            }

            // --- Delegation ---
            // Explicit entry (account_changes type 0x02) takes priority.
            // Otherwise bare EOAs get auto-delegated to DEFAULT_ACCOUNT.
            if let Some(target) = eip8130.delegation_target {
                let acc = journal.load_account_with_code_mut(sender)?.data;
                let current_code = acc.account().info.code.as_ref();
                let is_empty = current_code.map_or(true, |c| c.is_empty());
                let is_delegation = current_code.map_or(false, |c| c.is_eip7702());
                drop(acc);

                if !is_empty && !is_delegation {
                    return Err(ERROR::from_string(
                        "delegation entry rejected: sender has non-delegation bytecode".into(),
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
            } else if !sender_has_code
                && !eip8130.has_create_entry
                && eip8130.auto_delegation_code.len() == 23
            {
                let target = revm::primitives::Address::from_slice(
                    &eip8130.auto_delegation_code[3..],
                );
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
            // These logs become part of the receipt's log list, contributing to the
            // logs bloom and receipt root. Skipping them would diverge the receipt
            // root from base on any tx with account creation. Each entry is one of
            // OwnerAuthorized / AccountCreated, emitted in the order they appear in
            // `eip8130.account_creation_logs` (populated at conversion time).
            for event in &eip8130.account_creation_logs {
                journal.log(crate::transaction::eip8130::config_log_to_system_log(
                    crate::handler_aa_helpers::ACCOUNT_CONFIG_ADDRESS,
                    event,
                ));
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
        if evm.ctx().tx().tx_type() != crate::handler_aa_helpers::EIP8130_TX_TYPE {
            return self.mainnet.execution(evm, init_and_floor_gas);
        }

        use revm::interpreter::{
            CallOutcome, InstructionResult, InterpreterResult, SharedMemory,
            interpreter_action::{CallInput, CallInputs, CallScheme, CallValue, FrameInput},
        };
        use revm::primitives::{Address, B256};

        let eip8130 = evm.ctx().tx().eip8130_parts().clone();
        let sender = evm.ctx().tx().caller();

        // In estimation / eth_call mode we skip signature verification and
        // config validation since dummy (empty) auth blobs are expected.
        let is_estimation = evm.ctx().cfg().is_base_fee_check_disabled();

        // Determine whether the nonce channel is warm (previously used).
        // validate_against_state_and_deduct_caller already incremented the
        // nonce, so the current slot value is `original + 1`. If > 1 the
        // original was non-zero → warm SSTORE.
        //
        // Only adjust for real transactions — during estimation the handler
        // must stay consistent with validate_initial_tx_gas (which always
        // uses cold gas) so the binary search doesn't break.
        let nonce_warm_adjustment = if !is_estimation
            && eip8130.nonce_key != crate::handler_aa_helpers::NONCE_KEY_MAX
        {
            let nonce_slot = crate::handler_aa_helpers::aa_nonce_slot(sender, eip8130.nonce_key);
            let nonce_value = evm
                .ctx()
                .journal_mut()
                .sload(crate::precompiles::NONCE_MANAGER_ADDRESS, nonce_slot)?
                .data;
            if nonce_value > U256::from(1) {
                crate::handler_aa_helpers::NONCE_COLD_WARM_DELTA
            } else {
                0
            }
        } else {
            0
        };

        // Strip intrinsic and custom verifier cap from the revm gas_limit to
        // recover the sender's execution-only budget. During estimation, also
        // reserve calldata gas for auth blobs that will be present in the real tx.
        let overhead = if is_estimation {
            crate::handler_aa_helpers::estimation_calldata_overhead(&eip8130)
        } else {
            0
        };
        let gas_limit = evm
            .ctx()
            .tx()
            .gas_limit()
            .saturating_sub(eip8130.aa_intrinsic_gas + eip8130.custom_verifier_gas_cap + overhead)
            .saturating_add(nonce_warm_adjustment);

        let mut gas_remaining = gas_limit;
        let mut phase_results = Vec::with_capacity(eip8130.call_phases.len());

        // Ensure sender is loaded in the journal state for sub-call transfers.
        evm.ctx().journal_mut().load_account(sender)?;

        // Track gas used by custom verifier STATICCALLs. This is charged to
        // the payer separately from the sender's execution budget, capped at
        // `custom_verifier_gas_cap`.
        let mut verification_gas_used: u64 = 0;

        if !is_estimation {
            crate::handler_aa_helpers::validate_config_change_preconditions::<EVM, ERROR>(
                evm, sender, &eip8130,
            )?;

            // Validate config-change authorizer chain first against on-chain state.
            // This yields the pending owner overlay that is considered for
            // subsequent sender/payer auth checks.
            let pending_sender_owner_overrides =
                crate::handler_aa_helpers::validate_authorizer_chain::<EVM, ERROR, FRAME>(
                    &mut self.mainnet,
                    evm,
                    sender,
                    &eip8130,
                    &mut verification_gas_used,
                )?;

            // --- Custom verifier STATICCALL verification ---
            let verify_calls = [
                eip8130.sender_verify_call.as_ref(),
                eip8130.payer_verify_call.as_ref(),
            ];
            for verify_call in verify_calls.into_iter().flatten() {
                let owner_id =
                    crate::handler_aa_helpers::run_custom_verifier_staticcall::<EVM, ERROR, FRAME>(
                        &mut self.mainnet,
                        evm,
                        verify_call.verifier,
                        &verify_call.calldata,
                        sender,
                        eip8130.custom_verifier_gas_cap,
                        &mut verification_gas_used,
                        "custom verifier STATICCALL failed",
                        "custom verifier returned invalid owner_id (< 32 bytes)",
                    )?;

                let pending_overrides = if verify_call.account == sender {
                    Some(&pending_sender_owner_overrides)
                } else {
                    None
                };
                crate::handler_aa_helpers::validate_owner_config::<EVM, ERROR>(
                    evm,
                    verify_call.account,
                    owner_id,
                    verify_call.verifier,
                    verify_call.required_scope,
                    pending_overrides,
                )?;
            }

            // Delegate with nested custom verifier: validate the outer delegate owner
            // binding on sender/payer account.
            if eip8130.sender_verify_call.is_some()
                && eip8130.sender_verifier == crate::constants::DELEGATE_VERIFIER_ADDRESS
                && eip8130.owner_id != B256::ZERO
            {
                crate::handler_aa_helpers::validate_owner_config::<EVM, ERROR>(
                    evm,
                    sender,
                    U256::from_be_bytes(eip8130.owner_id.0),
                    crate::constants::DELEGATE_VERIFIER_ADDRESS,
                    crate::constants::OWNER_SCOPE_SENDER,
                    Some(&pending_sender_owner_overrides),
                )?;
            }
            if eip8130.payer_verify_call.is_some()
                && eip8130.payer_verifier == crate::constants::DELEGATE_VERIFIER_ADDRESS
                && eip8130.payer_owner_id != B256::ZERO
                && eip8130.payer != eip8130.sender
            {
                let payer_pending_overrides = if eip8130.payer == sender {
                    Some(&pending_sender_owner_overrides)
                } else {
                    None
                };
                crate::handler_aa_helpers::validate_owner_config::<EVM, ERROR>(
                    evm,
                    eip8130.payer,
                    U256::from_be_bytes(eip8130.payer_owner_id.0),
                    crate::constants::DELEGATE_VERIFIER_ADDRESS,
                    crate::constants::OWNER_SCOPE_PAYER,
                    payer_pending_overrides,
                )?;
            }

            // --- Native verifier re-validation at inclusion ---
            if eip8130.sender_verify_call.is_none()
                && eip8130.sender_verifier != Address::ZERO
            {
                crate::handler_aa_helpers::validate_native_verifier_owner::<EVM, ERROR>(
                    evm,
                    sender,
                    eip8130.sender_verifier,
                    eip8130.owner_id,
                    crate::constants::OWNER_SCOPE_SENDER,
                    Some(&pending_sender_owner_overrides),
                )?;
            }
            if eip8130.payer_verify_call.is_none()
                && eip8130.payer_verifier != Address::ZERO
                && eip8130.payer != eip8130.sender
            {
                let payer_pending_overrides = if eip8130.payer == sender {
                    Some(&pending_sender_owner_overrides)
                } else {
                    None
                };
                crate::handler_aa_helpers::validate_native_verifier_owner::<EVM, ERROR>(
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
            evm.ctx()
                .journal_mut()
                .load_account(crate::handler_aa_helpers::ACCOUNT_CONFIG_ADDRESS)?;
            for upd in &eip8130.sequence_updates {
                let current = evm
                    .ctx()
                    .journal_mut()
                    .sload(crate::handler_aa_helpers::ACCOUNT_CONFIG_ADDRESS, upd.slot)?
                    .data;
                let new_packed = upd.apply(current);
                evm.ctx().journal_mut().sstore(
                    crate::handler_aa_helpers::ACCOUNT_CONFIG_ADDRESS,
                    upd.slot,
                    new_packed,
                )?;
            }
        }

        // --- Emit AccountConfiguration events for config changes ---
        for event in &eip8130.config_change_logs {
            evm.ctx().journal_mut().log(crate::transaction::eip8130::config_log_to_system_log(
                crate::handler_aa_helpers::ACCOUNT_CONFIG_ADDRESS,
                event,
            ));
        }

        // Refund unused verification gas budget back to the execution gas pool.
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

                // Load callee bytecode so we can populate revm 38's
                // CallInputs::known_bytecode (now non-optional).
                let callee_known_bytecode = {
                    let info = &evm
                        .ctx()
                        .journal_mut()
                        .load_account_with_code(call.to)?
                        .data
                        .info;
                    (info.code_hash(), info.code.clone().unwrap_or_default())
                };

                let call_inputs = CallInputs {
                    input: CallInput::Bytes(call.data.clone()),
                    return_memory_offset: 0..0,
                    gas_limit: call_gas,
                    // EIP-8037 reservoir: phased calls do not propagate any
                    // pre-charged state gas budget into the callee frame.
                    reservoir: 0,
                    bytecode_address: call.to,
                    known_bytecode: callee_known_bytecode,
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

            phase_results.push(crate::transaction::eip8130::Eip8130PhaseResult {
                success: phase_ok,
                gas_used: phase_gas_start.saturating_sub(gas_remaining),
            });

            // EIP-8130 §Call Execution: "if any call in a phase reverts,
            // all state changes for that phase are discarded and remaining
            // phases are skipped." Stop dispatching further phases once
            // one has reverted; skipped phases are filled in below so
            // `phaseStatuses.len() == calls.len()`.
            if !phase_ok {
                break;
            }
        }

        // Pad with `success = false` entries for phases that were skipped
        // because an earlier phase reverted, so the receipt's
        // `phaseStatuses` length equals `calls.len()` per spec ("Phases
        // after a revert ... reported as 0x00").
        while phase_results.len() < eip8130.call_phases.len() {
            phase_results.push(crate::transaction::eip8130::Eip8130PhaseResult {
                success: false,
                gas_used: 0,
            });
        }

        // EIP-8130 §RPC Extensions: `status = 0x01` iff "all phases
        // succeeded **or `calls` was empty**". `Iterator::all` returns
        // `true` on an empty iterator (vacuous truth), which gives us
        // the empty-calls success case — including legitimate no-op
        // shapes like nonce-bump-only, lock-only, or pure
        // account-change transactions. Reaching this point with empty
        // `phase_results` means no phases were attempted; any
        // pre-execution failure (account_changes, validation) would
        // have errored out earlier on a different path.
        let all_phases_succeeded = phase_results.iter().all(|r| r.success);

        let tx_succeeded = is_estimation || all_phases_succeeded;

        // Emit a system log with per-phase statuses so they survive in the receipt's
        // log list and can be recovered at RPC time.
        if !phase_results.is_empty() {
            evm.ctx().journal_mut().log(
                crate::transaction::eip8130::phase_statuses_system_log(
                    crate::precompiles::TX_CONTEXT_ADDRESS,
                    &phase_results,
                ),
            );
        }

        let mut result_gas = Gas::new_spent(evm.ctx().tx().gas_limit());
        result_gas.erase_cost(gas_remaining + unused_verification_gas);
        if accumulated_refunds > 0 {
            result_gas.record_refund(accumulated_refunds);
        }

        let output = crate::transaction::eip8130::encode_phase_statuses(&phase_results);

        let instruction_result = if tx_succeeded {
            InstructionResult::Stop
        } else {
            InstructionResult::Revert
        };

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

        // For EIP-8130 sponsored transactions, refund the payer (not tx.caller()).
        if evm.ctx().tx().tx_type() == crate::handler_aa_helpers::EIP8130_TX_TYPE {
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
