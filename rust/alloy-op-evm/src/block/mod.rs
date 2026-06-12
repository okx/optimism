//! Block executor for Optimism.

use crate::OpEvmFactory;
use alloc::{borrow::Cow, boxed::Box, sync::Arc, vec::Vec};
use alloy_consensus::{Eip658Value, Header, Transaction, TransactionEnvelope, TxReceipt};
use alloy_eips::{Encodable2718, Typed2718};
use alloy_evm::{
    Database, Evm, EvmFactory, FromRecoveredTx, FromTxWithEncoded, RecoveredTx,
    block::{
        BlockExecutionError, BlockExecutionResult, BlockExecutor, BlockExecutorFactory,
        BlockExecutorFor, BlockValidationError, ExecutableTx, GasOutput, OnStateHook,
        StateChangePostBlockSource, StateChangeSource, StateDB, SystemCaller, TxResult,
        state_changes::{balance_increment_state, post_block_balance_increments},
    },
    eth::{EthTxResult, receipt_builder::ReceiptBuilderCtx},
};
use alloy_op_hardforks::{OpChainHardforks, OpHardforks};
use alloy_primitives::{Address, B256, Bytes, Log, U256};
use canyon::ensure_create2_deployer;
use op_alloy::consensus::OpDepositReceipt;
use op_revm::{
    L1BlockInfo, OpTransaction, constants::L1_BLOCK_CONTRACT, estimate_tx_compressed_size,
    transaction::OpTxTr, transaction::deposit::DEPOSIT_TRANSACTION_TYPE,
};
pub use receipt_builder::OpAlloyReceiptBuilder;
use receipt_builder::OpReceiptBuilder;
use revm::{
    Database as _, DatabaseCommit, Inspector,
    context::{Block, result::ResultAndState},
    database::DatabaseCommitExt,
    state::{Account, EvmState},
};

mod canyon;
pub mod receipt_builder;

/// Trait for OP transaction environments. Allows to recover the transaction encoded bytes if
/// they're available.
pub trait OpTxEnv {
    /// Returns the encoded bytes of the transaction.
    fn encoded_bytes(&self) -> Option<&Bytes>;

    /// XLayer (XLOP-1100): the deposit mint amount, if this is a deposit carrying a mint.
    /// Added to the already-bounded `OpTxEnv` trait so the executor can read mint without a
    /// new generic bound (which would cascade into `OpEvmConfig` and beyond).
    fn deposit_mint(&self) -> Option<u128>;
}

impl<T: revm::context::Transaction> OpTxEnv for OpTransaction<T> {
    fn encoded_bytes(&self) -> Option<&Bytes> {
        self.enveloped_tx.as_ref()
    }

    fn deposit_mint(&self) -> Option<u128> {
        OpTxTr::mint(self)
    }
}

/// Context for OP block execution.
#[derive(Debug, Default, Clone)]
pub struct OpBlockExecutionCtx {
    /// Parent block hash.
    pub parent_hash: B256,
    /// Parent beacon block root.
    pub parent_beacon_block_root: Option<B256>,
    /// The block's extra data.
    pub extra_data: Bytes,
}

/// The result of executing an OP transaction.
#[derive(Debug)]
pub struct OpTxResult<H, T> {
    /// The inner result of the transaction execution.
    pub inner: EthTxResult<H, T>,
    /// Whether the transaction is a deposit transaction.
    pub is_deposit: bool,
    /// The sender of the transaction.
    pub sender: Address,
    /// XLayer (XLOP-1100): set when a deposit was decided to be included-as-reverted by the
    /// blacklist hook; carries the data `commit_transaction` needs to apply the revert.
    pub deposit_revert: Option<DepositRevertData>,
}

/// XLayer (XLOP-1100): data captured at deposit execution time, needed at commit time to
/// apply the included-as-reverted post-state (op-revm failed-deposit parity: keep mint,
/// bump nonce, gasUsed = gasLimit).
#[derive(Debug, Clone, Copy)]
pub struct DepositRevertData {
    /// Deposit mint amount (re-credited to the sender across the revert).
    pub mint: u128,
    /// Deposit gas limit (becomes `gasUsed` and the gas charged to the block).
    pub gas_limit: u64,
}

/// XLayer (XLOP-1100): downstream decision hook for blacklisting deposit (L1→L2) txs.
///
/// Implemented by `xlayer-blacklist-node`; the executor depends only on this trait, never on
/// the blacklist crate. The decision uses committed effects only — the tx's logs
/// (Transfer-event check) and the post-execution state diff (native-ETH balance check).
/// check① (committed CALL-frame touch) is intentionally NOT part of the deposit decision
/// (cross-client decision B, see `IMPL-blacklist-full-alignment`): the follower validation
/// EVM cannot mount an inspector, so to keep sequencer/follower byte-identical, deposits are
/// judged on logs + balance only.
pub trait DepositBlacklistHook: Send + Sync + core::fmt::Debug {
    /// Returns true if this deposit must be included-as-reverted, i.e. a committed
    /// Transfer-class event or a committed native-ETH balance change involves a listed
    /// address. Exempt senders (system / L1-attributes) MUST be handled by the impl.
    ///
    /// `balance_changes` is `(address, balance_before, balance_after)` for every account the
    /// tx changed (before is the pre-tx committed balance, after is the post-execution / pre-
    /// commit balance) — the executor reads these from the pre-commit db so the hook can do
    /// the native-ETH balance check without db access.
    fn should_intercept_deposit(
        &self,
        sender: Address,
        logs: &[Log],
        balance_changes: &[(Address, U256, U256)],
    ) -> bool;

    /// XLayer (XLOP-1100): refresh the block-head blacklist snapshot for the follower face.
    /// Called once per block from `apply_pre_execution_changes`, before any tx. `static_call`
    /// performs a read-only system-address call `(to, calldata, gas) -> output`, returning
    /// `None` on failure (the impl then fails open to an empty list). The impl drives the
    /// `getBlacklist` pagination and stores the snapshot in its shared runtime context.
    fn refresh_snapshot(&self, static_call: &mut dyn FnMut(Address, Bytes, u64) -> Option<Bytes>);
}

/// XLayer (XLOP-1100): system-address caller for the block-head mirror read (0xff..fe).
const BLACKLIST_SYSTEM_CALLER: Address =
    alloy_primitives::address!("fffffffffffffffffffffffffffffffffffffffe");

impl<H, T> TxResult for OpTxResult<H, T> {
    type HaltReason = H;

    fn result(&self) -> &ResultAndState<Self::HaltReason> {
        &self.inner.result
    }

    fn into_result(self) -> ResultAndState<Self::HaltReason> {
        self.inner.result
    }
}

/// Block executor for Optimism.
#[derive(Debug)]
pub struct OpBlockExecutor<Evm, R: OpReceiptBuilder, Spec> {
    /// Spec.
    pub spec: Spec,
    /// Receipt builder.
    pub receipt_builder: R,
    /// Context for block execution.
    pub ctx: OpBlockExecutionCtx,
    /// The EVM used by executor.
    pub evm: Evm,
    /// Receipts of executed transactions.
    pub receipts: Vec<R::Receipt>,
    /// Total gas used by executed transactions.
    pub gas_used: u64,
    /// Da footprint.
    ///
    /// This is only set for blocks post-Jovian activation.
    /// See [DA footprint block limit spec](https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/jovian/exec-engine.md#da-footprint-block-limit)
    pub da_footprint_used: u64,
    /// Whether Regolith hardfork is active.
    pub is_regolith: bool,
    /// Utility to call system smart contracts.
    pub system_caller: SystemCaller<Spec>,
    /// XLayer (XLOP-1100): optional blacklist deposit-intercept hook. `None` = no-op (default),
    /// so this field never changes behaviour for non-XLayer builds.
    pub blacklist_hook: Option<Arc<dyn DepositBlacklistHook>>,
}

impl<E, R, Spec> OpBlockExecutor<E, R, Spec>
where
    E: Evm,
    R: OpReceiptBuilder,
    Spec: OpHardforks + Clone,
{
    /// Creates a new [`OpBlockExecutor`].
    pub fn new(evm: E, ctx: OpBlockExecutionCtx, spec: Spec, receipt_builder: R) -> Self {
        Self {
            is_regolith: spec
                .is_regolith_active_at_timestamp(evm.block().timestamp().saturating_to()),
            evm,
            system_caller: SystemCaller::new(spec.clone()),
            spec,
            receipt_builder,
            receipts: Vec::new(),
            gas_used: 0,
            da_footprint_used: 0,
            ctx,
            blacklist_hook: None,
        }
    }

    /// XLayer (XLOP-1100): attach the blacklist deposit-intercept hook (builder pattern).
    pub fn with_blacklist_hook(mut self, hook: Option<Arc<dyn DepositBlacklistHook>>) -> Self {
        self.blacklist_hook = hook;
        self
    }
}

/// Custom errors that can occur during OP block execution.
#[derive(Debug, thiserror::Error)]
pub enum OpBlockExecutionError {
    /// Failed to load cache account.
    #[error("failed to load cache account")]
    LoadCacheAccount,

    /// Failed to get Jovian da footprint gas scalar from database.
    #[error("failed to get da footprint gas scalar from database: {_0}")]
    GetJovianDaFootprintScalar(Box<dyn core::error::Error + Send + Sync + 'static>),

    /// Transaction DA footprint exceeds available block DA footprint.
    #[error(
        "transaction DA footprint exceeds available block DA footprint. transaction_da_footprint: {transaction_da_footprint}, available_block_da_footprint: {available_block_da_footprint}"
    )]
    TransactionDaFootprintAboveGasLimit {
        /// The DA footprint of the transaction to execute.
        transaction_da_footprint: u64,
        /// The available block DA footprint.
        available_block_da_footprint: u64,
    },
}

impl<E, R, Spec> OpBlockExecutor<E, R, Spec>
where
    E: Evm<
            DB: Database + DatabaseCommit + StateDB,
            Tx: FromRecoveredTx<R::Transaction> + FromTxWithEncoded<R::Transaction> + OpTxEnv,
        >,
    R: OpReceiptBuilder<Transaction: Transaction + Encodable2718, Receipt: TxReceipt>,
    Spec: OpHardforks,
{
    fn jovian_da_footprint_estimation(
        &mut self,
        tx_env: &E::Tx,
        tx: impl RecoveredTx<R::Transaction>,
    ) -> Result<u64, BlockExecutionError> {
        // Try to use the enveloped tx if it exists, otherwise use the encoded 2718 bytes
        let encoded = tx_env
            .encoded_bytes()
            .map_or_else(
                || estimate_tx_compressed_size(tx.tx().encoded_2718().as_ref()),
                |encoded| estimate_tx_compressed_size(encoded),
            )
            .saturating_div(1_000_000);

        // Load the L1 block contract into the cache. If the L1 block contract is not pre-loaded the
        // database will panic when trying to fetch the DA footprint gas scalar.
        self.evm.db_mut().basic(L1_BLOCK_CONTRACT).map_err(BlockExecutionError::other)?;

        let da_footprint_gas_scalar = L1BlockInfo::fetch_da_footprint_gas_scalar(self.evm.db_mut())
            .map_err(BlockExecutionError::other)?
            .into();

        Ok(encoded.saturating_mul(da_footprint_gas_scalar))
    }
}

impl<E, R, Spec> BlockExecutor for OpBlockExecutor<E, R, Spec>
where
    E: Evm<
            DB: Database + DatabaseCommit + StateDB,
            Tx: FromRecoveredTx<R::Transaction> + FromTxWithEncoded<R::Transaction> + OpTxEnv,
        >,
    R: OpReceiptBuilder<Transaction: Transaction + Encodable2718, Receipt: TxReceipt>,
    Spec: OpHardforks,
{
    type Transaction = R::Transaction;
    type Receipt = R::Receipt;
    type Evm = E;
    type Result = OpTxResult<E::HaltReason, <R::Transaction as TransactionEnvelope>::TxType>;

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        // XLayer (XLOP-1100): refresh the block-head blacklist snapshot once per block (before
        // any tx / pre-execution change), from the executor's pre-state EVM. Follower face
        // snapshot supply; no-op when no hook is attached.
        if let Some(hook) = self.blacklist_hook.clone() {
            let evm = &mut self.evm;
            hook.refresh_snapshot(&mut |to, input, _gas| {
                match evm.transact_system_call(BLACKLIST_SYSTEM_CALLER, to, input) {
                    Ok(ResultAndState {
                        result: revm::context::result::ExecutionResult::Success { output, .. },
                        ..
                    }) => Some(output.into_data()),
                    _ => None,
                }
            });
        }

        self.system_caller.apply_blockhashes_contract_call(self.ctx.parent_hash, &mut self.evm)?;
        self.system_caller
            .apply_beacon_root_contract_call(self.ctx.parent_beacon_block_root, &mut self.evm)?;

        // Ensure that the create2deployer is force-deployed at the canyon transition. Optimism
        // blocks will always have at least a single transaction in them (the L1 info transaction),
        // so we can safely assume that this will always be triggered upon the transition and that
        // the above check for empty blocks will never be hit on OP chains.
        ensure_create2_deployer(
            &self.spec,
            self.evm.block().timestamp().saturating_to(),
            self.evm.db_mut(),
        )
        .map_err(BlockExecutionError::other)?;

        Ok(())
    }

    fn execute_transaction_without_commit(
        &mut self,
        tx: impl ExecutableTx<Self>,
    ) -> Result<Self::Result, BlockExecutionError> {
        let (tx_env, tx) = tx.into_parts();
        let is_deposit = tx.tx().ty() == DEPOSIT_TRANSACTION_TYPE;

        // The sum of the transaction's gas limit, Tg, and the gas utilized in this block prior,
        // must be no greater than the block's gasLimit.
        let block_available_gas = self.evm.block().gas_limit() - self.gas_used;
        if tx.tx().gas_limit() > block_available_gas && (self.is_regolith || !is_deposit) {
            return Err(BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                transaction_gas_limit: tx.tx().gas_limit(),
                block_available_gas,
            }
            .into());
        }

        let da_footprint_used = if self
            .spec
            .is_jovian_active_at_timestamp(self.evm.block().timestamp().saturating_to()) &&
            !is_deposit
        {
            let da_footprint_available = self.evm.block().gas_limit() - self.da_footprint_used;

            let tx_da_footprint = self.jovian_da_footprint_estimation(&tx_env, &tx)?;

            if tx_da_footprint > da_footprint_available {
                return Err(BlockExecutionError::Validation(BlockValidationError::Other(
                    Box::new(OpBlockExecutionError::TransactionDaFootprintAboveGasLimit {
                        transaction_da_footprint: tx_da_footprint,
                        available_block_da_footprint: da_footprint_available,
                    }),
                )));
            }

            tx_da_footprint
        } else {
            0
        };

        // XLayer (XLOP-1100): capture the deposit mint before the tx_env is consumed; needed
        // by the included-as-reverted apply at commit time.
        let deposit_mint = is_deposit.then(|| tx_env.deposit_mint().unwrap_or_default());
        let deposit_gas_limit = tx.tx().gas_limit();

        // Execute transaction and return the result
        let result = self.evm.transact(tx_env).map_err(|err| {
            let hash = tx.tx().trie_hash();
            BlockExecutionError::evm(err, hash)
        })?;

        // XLayer (XLOP-1100): on the deposit path, ask the blacklist hook (logs + balance
        // changes) whether this deposit must be included-as-reverted. check① is intentionally
        // excluded for deposits (decision B). No hook / non-deposit → never intercept.
        let deposit_revert = match (self.blacklist_hook.clone(), is_deposit) {
            (Some(hook), true) => {
                // Pre-tx committed balance for each account this tx changed (db is pre-commit).
                let balance_changes: Vec<(Address, U256, U256)> = result
                    .state
                    .iter()
                    .map(|(addr, acct)| {
                        let before = self
                            .evm
                            .db_mut()
                            .basic(*addr)
                            .ok()
                            .flatten()
                            .map(|i| i.balance)
                            .unwrap_or_default();
                        (*addr, before, acct.info.balance)
                    })
                    .collect();
                if hook.should_intercept_deposit(
                    *tx.signer(),
                    result.result.logs(),
                    &balance_changes,
                ) {
                    Some(DepositRevertData {
                        mint: deposit_mint.unwrap_or_default(),
                        gas_limit: deposit_gas_limit,
                    })
                } else {
                    None
                }
            }
            _ => None,
        };

        Ok(OpTxResult {
            inner: EthTxResult {
                result,
                blob_gas_used: da_footprint_used,
                tx_type: tx.tx().tx_type(),
            },
            is_deposit,
            sender: *tx.signer(),
            deposit_revert,
        })
    }

    fn commit_transaction(
        &mut self,
        output: Self::Result,
    ) -> Result<GasOutput, BlockExecutionError> {
        let OpTxResult {
            inner: EthTxResult { result: ResultAndState { result, state }, blob_gas_used, tx_type },
            is_deposit,
            sender,
            deposit_revert,
        } = output;

        // Fetch the depositor account from the database for the deposit nonce.
        // Note that this *only* needs to be done post-regolith hardfork, as deposit nonces
        // were not introduced in Bedrock. In addition, regular transactions don't have deposit
        // nonces, so we don't need to touch the DB for those.
        let depositor = (self.is_regolith && is_deposit)
            .then(|| self.evm.db_mut().basic(sender).map(|acc| acc.unwrap_or_default()))
            .transpose()
            .map_err(BlockExecutionError::other)?;

        // XLayer (XLOP-1100): included-as-reverted for a blacklisted deposit. Discard the
        // execution effects (do NOT commit `state`); reproduce op-revm's failed-deposit
        // post-state — keep the mint, bump the sender nonce, status=0, gasUsed=gasLimit, empty
        // logs, DepositNonce = pre-exec nonce N. Byte-identical with the builder face (Step 3).
        if let Some(rev) = deposit_revert {
            let pre = depositor.unwrap_or_default();
            let mut info = pre.clone();
            info.nonce = pre.nonce.saturating_add(1);
            info.balance = pre.balance.saturating_add(U256::from(rev.mint));
            let mut account = Account { info, ..Default::default() };
            account.mark_touch();
            let mut revert_state = EvmState::default();
            revert_state.insert(sender, account);

            // Fire the state hook with the reverted state (trie/consensus path consistency).
            self.system_caller
                .on_state(StateChangeSource::Transaction(self.receipts.len()), &revert_state);

            // Full gasLimit charged to the block (op-geth ChargeUsed parity).
            self.gas_used += rev.gas_limit;

            let receipt = alloy_consensus::Receipt {
                status: Eip658Value::Eip658(false),
                cumulative_gas_used: self.gas_used,
                logs: Vec::new(),
            };
            self.receipts.push(self.receipt_builder.build_deposit_receipt(OpDepositReceipt {
                inner: receipt,
                deposit_nonce: Some(pre.nonce),
                deposit_receipt_version: self
                    .spec
                    .is_canyon_active_at_timestamp(self.evm.block().timestamp().saturating_to())
                    .then_some(1),
            }));
            self.evm.db_mut().commit(revert_state);
            return Ok(GasOutput::new(rev.gas_limit));
        }

        self.system_caller.on_state(StateChangeSource::Transaction(self.receipts.len()), &state);

        let gas_used = result.tx_gas_used();

        // append gas used
        self.gas_used += gas_used;

        // Update DA footprint if Jovian is active
        if self.spec.is_jovian_active_at_timestamp(self.evm.block().timestamp().saturating_to()) &&
            !is_deposit
        {
            // Add to DA footprint used
            self.da_footprint_used = self.da_footprint_used.saturating_add(blob_gas_used);
        }

        self.receipts.push(
            match self.receipt_builder.build_receipt(ReceiptBuilderCtx {
                tx_type,
                result,
                cumulative_gas_used: self.gas_used,
                evm: &self.evm,
                state: &state,
            }) {
                Ok(receipt) => receipt,
                Err(ctx) => {
                    let receipt = alloy_consensus::Receipt {
                        // Success flag was added in `EIP-658: Embedding transaction status code
                        // in receipts`.
                        status: Eip658Value::Eip658(ctx.result.is_success()),
                        cumulative_gas_used: self.gas_used,
                        logs: ctx.result.into_logs(),
                    };

                    self.receipt_builder.build_deposit_receipt(OpDepositReceipt {
                        inner: receipt,
                        deposit_nonce: depositor.map(|account| account.nonce),
                        // The deposit receipt version was introduced in Canyon to indicate an
                        // update to how receipt hashes should be computed
                        // when set. The state transition process ensures
                        // this is only set for post-Canyon deposit
                        // transactions.
                        deposit_receipt_version: (is_deposit &&
                            self.spec.is_canyon_active_at_timestamp(
                                self.evm.block().timestamp().saturating_to(),
                            ))
                        .then_some(1),
                    })
                }
            },
        );

        self.evm.db_mut().commit(state);

        Ok(GasOutput::new(gas_used))
    }

    fn finish(
        mut self,
    ) -> Result<(Self::Evm, BlockExecutionResult<R::Receipt>), BlockExecutionError> {
        let balance_increments =
            post_block_balance_increments::<Header>(&self.spec, self.evm.block(), &[], None);
        // increment balances
        self.evm
            .db_mut()
            .increment_balances(balance_increments.clone())
            .map_err(|_| BlockValidationError::IncrementBalanceFailed)?;
        // call state hook with changes due to balance increments.
        self.system_caller.try_on_state_with(|| {
            balance_increment_state(&balance_increments, self.evm.db_mut()).map(|state| {
                (
                    StateChangeSource::PostBlock(StateChangePostBlockSource::BalanceIncrements),
                    Cow::Owned(state),
                )
            })
        })?;

        let legacy_gas_used =
            self.receipts.last().map(|r| r.cumulative_gas_used()).unwrap_or_default();

        Ok((
            self.evm,
            BlockExecutionResult {
                receipts: self.receipts,
                requests: Default::default(),
                gas_used: legacy_gas_used,
                blob_gas_used: self.da_footprint_used,
            },
        ))
    }

    fn set_state_hook(&mut self, hook: Option<Box<dyn OnStateHook>>) {
        self.system_caller.with_state_hook(hook);
    }

    fn evm_mut(&mut self) -> &mut Self::Evm {
        &mut self.evm
    }

    fn evm(&self) -> &Self::Evm {
        &self.evm
    }

    fn receipts(&self) -> &[Self::Receipt] {
        &self.receipts
    }
}

/// Ethereum block executor factory.
// `Copy` removed: the optional `blacklist_hook: Option<Arc<dyn DepositBlacklistHook>>` field
// (XLOP-1100) is `Clone` but not `Copy`.
#[derive(Debug, Clone, Default)]
pub struct OpBlockExecutorFactory<
    R = OpAlloyReceiptBuilder,
    Spec = OpChainHardforks,
    EvmFactory = OpEvmFactory,
> {
    /// Receipt builder.
    receipt_builder: R,
    /// Chain specification.
    spec: Spec,
    /// EVM factory.
    evm_factory: EvmFactory,
    /// XLayer (XLOP-1100): optional blacklist deposit-intercept hook, cloned into every
    /// executor produced by this factory. `None` = no-op (default).
    blacklist_hook: Option<Arc<dyn DepositBlacklistHook>>,
}

impl<R, Spec, EvmFactory> OpBlockExecutorFactory<R, Spec, EvmFactory> {
    /// Creates a new [`OpBlockExecutorFactory`] with the given spec, [`EvmFactory`], and
    /// [`OpReceiptBuilder`].
    pub const fn new(receipt_builder: R, spec: Spec, evm_factory: EvmFactory) -> Self {
        Self { receipt_builder, spec, evm_factory, blacklist_hook: None }
    }

    /// XLayer (XLOP-1100): attach the blacklist deposit-intercept hook (builder pattern).
    pub fn with_blacklist_hook(mut self, hook: Option<Arc<dyn DepositBlacklistHook>>) -> Self {
        self.blacklist_hook = hook;
        self
    }

    /// Exposes the receipt builder.
    pub const fn receipt_builder(&self) -> &R {
        &self.receipt_builder
    }

    /// Exposes the chain specification.
    pub const fn spec(&self) -> &Spec {
        &self.spec
    }

    /// Exposes the EVM factory.
    pub const fn evm_factory(&self) -> &EvmFactory {
        &self.evm_factory
    }
}

impl<R, Spec, EvmF> BlockExecutorFactory for OpBlockExecutorFactory<R, Spec, EvmF>
where
    R: OpReceiptBuilder<Transaction: Transaction + Encodable2718, Receipt: TxReceipt>,
    Spec: OpHardforks,
    EvmF: EvmFactory<
        Tx: FromRecoveredTx<R::Transaction> + FromTxWithEncoded<R::Transaction> + OpTxEnv,
    >,
    Self: 'static,
{
    type EvmFactory = EvmF;
    type ExecutionCtx<'a> = OpBlockExecutionCtx;
    type Transaction = R::Transaction;
    type Receipt = R::Receipt;

    fn evm_factory(&self) -> &Self::EvmFactory {
        &self.evm_factory
    }

    fn create_executor<'a, DB, I>(
        &'a self,
        evm: EvmF::Evm<DB, I>,
        ctx: Self::ExecutionCtx<'a>,
    ) -> impl BlockExecutorFor<'a, Self, DB, I>
    where
        DB: StateDB + 'a,
        I: Inspector<EvmF::Context<DB>> + 'a,
    {
        OpBlockExecutor::new(evm, ctx, &self.spec, &self.receipt_builder)
            .with_blacklist_hook(self.blacklist_hook.clone())
    }
}

#[cfg(test)]
mod tests {
    use alloc::{string::ToString, vec};
    use alloy_consensus::{SignableTransaction, TxLegacy, transaction::Recovered};
    use alloy_eips::eip2718::WithEncoded;
    use alloy_evm::{EvmEnv, ToTxEnv};
    use alloy_hardforks::ForkCondition;
    use alloy_op_hardforks::OpHardfork;
    use alloy_primitives::{Address, Signature, U256, uint};
    use op_alloy::consensus::OpTxEnvelope;
    use op_revm::{
        L1BlockInfo, OpBuilder, OpSpecId, OpTransaction,
        constants::{
            BASE_FEE_SCALAR_OFFSET, ECOTONE_L1_BLOB_BASE_FEE_SLOT, ECOTONE_L1_FEE_SCALARS_SLOT,
            L1_BASE_FEE_SLOT, L1_BLOCK_CONTRACT, OPERATOR_FEE_SCALARS_SLOT,
        },
    };
    use revm::{
        Context, MainContext,
        context::{BlockEnv, CfgEnv},
        database::{CacheDB, EmptyDB, InMemoryDB, State},
        inspector::NoOpInspector,
        primitives::HashMap,
        state::AccountInfo,
    };

    use crate::OpEvm;

    use super::*;

    #[test]
    fn test_with_encoded() {
        let executor_factory = OpBlockExecutorFactory::new(
            OpAlloyReceiptBuilder::default(),
            OpChainHardforks::op_mainnet(),
            OpEvmFactory::<crate::OpTx>::default(),
        );
        let mut db = State::builder().with_database(CacheDB::<EmptyDB>::default()).build();
        let evm = executor_factory.evm_factory.create_evm(&mut db, EvmEnv::default());
        let mut executor = executor_factory.create_executor(evm, OpBlockExecutionCtx::default());
        let tx = Recovered::new_unchecked(
            OpTxEnvelope::Legacy(TxLegacy::default().into_signed(Signature::new(
                Default::default(),
                Default::default(),
                Default::default(),
            ))),
            Address::ZERO,
        );
        let tx_with_encoded = WithEncoded::new(tx.encoded_2718().into(), tx.clone());

        // make sure we can use both `WithEncoded` and transaction itself as inputs.
        let _ = executor.execute_transaction(&tx);
        let _ = executor.execute_transaction(&tx_with_encoded);
    }

    fn prepare_jovian_db(da_footprint_gas_scalar: u16) -> State<InMemoryDB> {
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
        let da_footprint_gas_scalar_bytes = da_footprint_gas_scalar.to_be_bytes();
        let mut operator_fee_and_da_footprint = [0u8; 32];
        operator_fee_and_da_footprint[31] = OPERATOR_FEE_CONST;
        operator_fee_and_da_footprint[23] = OPERATOR_FEE_SCALAR;
        operator_fee_and_da_footprint[19] = da_footprint_gas_scalar_bytes[1];
        operator_fee_and_da_footprint[18] = da_footprint_gas_scalar_bytes[0];
        let operator_fee_and_da_footprint_u256 = U256::from_be_bytes(operator_fee_and_da_footprint);

        let mut db = State::builder().with_database(InMemoryDB::default()).build();

        db.insert_account_with_storage(
            L1_BLOCK_CONTRACT,
            Default::default(),
            HashMap::from_iter([
                (L1_BASE_FEE_SLOT, L1_BASE_FEE),
                (ECOTONE_L1_FEE_SCALARS_SLOT, L1_FEE_SCALARS),
                (ECOTONE_L1_BLOB_BASE_FEE_SLOT, L1_BLOB_BASE_FEE),
                (OPERATOR_FEE_SCALARS_SLOT, operator_fee_and_da_footprint_u256),
            ]),
        );

        db.insert_account(
            Address::ZERO,
            AccountInfo { balance: U256::from(400_000_000), ..Default::default() },
        );

        db
    }

    fn build_executor<'a>(
        db: &'a mut State<InMemoryDB>,
        receipt_builder: &'a OpAlloyReceiptBuilder,
        op_chain_hardforks: &'a OpChainHardforks,
        gas_limit: u64,
        jovian_timestamp: u64,
    ) -> OpBlockExecutor<
        OpEvm<
            &'a mut State<InMemoryDB>,
            NoOpInspector,
            op_revm::precompiles::OpPrecompiles,
            crate::OpTx,
        >,
        &'a OpAlloyReceiptBuilder,
        &'a OpChainHardforks,
    > {
        let ctx = Context::mainnet()
            .with_tx(crate::OpTx(OpTransaction::builder().build_fill()))
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::BEDROCK))
            .with_chain(L1BlockInfo::default())
            .with_db(db)
            .with_chain(L1BlockInfo {
                operator_fee_scalar: Some(U256::from(2)),
                operator_fee_constant: Some(U256::from(50)),
                ..Default::default()
            })
            .with_block(BlockEnv {
                timestamp: U256::from(jovian_timestamp),
                gas_limit,
                ..Default::default()
            })
            .modify_cfg_chained(|cfg| cfg.spec = OpSpecId::JOVIAN);

        let evm = OpEvm::new(ctx.build_op_with_inspector(NoOpInspector {}), true);

        OpBlockExecutor::new(
            evm,
            OpBlockExecutionCtx::default(),
            op_chain_hardforks,
            receipt_builder,
        )
    }

    #[test]
    fn test_jovian_da_footprint_estimation() {
        const DA_FOOTPRINT_GAS_SCALAR: u16 = 7;
        const GAS_LIMIT: u64 = 100_000;
        const JOVIAN_TIMESTAMP: u64 = 1746806402;

        let mut db = prepare_jovian_db(DA_FOOTPRINT_GAS_SCALAR);
        let op_chain_hardforks = OpChainHardforks::new(
            OpHardfork::op_mainnet()
                .into_iter()
                .chain(vec![(OpHardfork::Jovian, ForkCondition::Timestamp(JOVIAN_TIMESTAMP))]),
        );

        let receipt_builder = OpAlloyReceiptBuilder::default();
        let mut executor = build_executor(
            &mut db,
            &receipt_builder,
            &op_chain_hardforks,
            GAS_LIMIT,
            JOVIAN_TIMESTAMP,
        );

        let tx_inner = TxLegacy { gas_limit: GAS_LIMIT, ..Default::default() };

        let tx = Recovered::new_unchecked(
            OpTxEnvelope::Legacy(tx_inner.into_signed(Signature::new(
                Default::default(),
                Default::default(),
                Default::default(),
            ))),
            Address::ZERO,
        );
        let tx_env = tx.to_tx_env();

        assert!(executor.da_footprint_used == 0);

        let expected_da_footprint = executor.jovian_da_footprint_estimation(&tx_env, &tx).unwrap();

        // make sure we can use both `WithEncoded` and transaction itself as inputs.
        let res = executor.execute_transaction(&tx);
        assert!(res.is_ok());

        assert!(executor.da_footprint_used == expected_da_footprint);
    }

    #[test]
    fn test_jovian_da_footprint_estimation_out_of_gas() {
        const DA_FOOTPRINT_GAS_SCALAR: u16 = 7;
        const JOVIAN_TIMESTAMP: u64 = 1746806402;
        const GAS_LIMIT: u64 = 100;

        let mut db = prepare_jovian_db(DA_FOOTPRINT_GAS_SCALAR);
        let op_chain_hardforks = OpChainHardforks::new(
            OpHardfork::op_mainnet()
                .into_iter()
                .chain(vec![(OpHardfork::Jovian, ForkCondition::Timestamp(JOVIAN_TIMESTAMP))]),
        );

        let receipt_builder = OpAlloyReceiptBuilder::default();
        let mut executor = build_executor(
            &mut db,
            &receipt_builder,
            &op_chain_hardforks,
            GAS_LIMIT,
            JOVIAN_TIMESTAMP,
        );

        let tx_inner = TxLegacy { gas_limit: GAS_LIMIT, ..Default::default() };

        let tx = Recovered::new_unchecked(
            OpTxEnvelope::Legacy(tx_inner.into_signed(Signature::new(
                Default::default(),
                Default::default(),
                Default::default(),
            ))),
            Address::ZERO,
        );
        let tx_env = tx.to_tx_env();

        assert!(executor.da_footprint_used == 0);

        let expected_da_footprint = executor.jovian_da_footprint_estimation(&tx_env, &tx).unwrap();

        // make sure we can use both `WithEncoded` and transaction itself as inputs.
        let res = executor.execute_transaction(&tx);
        assert!(res.is_err());
        let err = res.unwrap_err();
        match err {
            BlockExecutionError::Validation(BlockValidationError::Other(err)) => {
                assert_eq!(
                    err.to_string(),
                    OpBlockExecutionError::TransactionDaFootprintAboveGasLimit {
                        transaction_da_footprint: expected_da_footprint,
                        available_block_da_footprint: GAS_LIMIT,
                    }
                    .to_string(),
                );
            }
            _ => panic!("expected TransactionDaFootprintAboveGasLimit error"),
        }
    }

    #[test]
    fn test_jovian_da_footprint_estimation_maxed_out_da_footprint() {
        const DA_FOOTPRINT_GAS_SCALAR: u16 = 2000;
        const JOVIAN_TIMESTAMP: u64 = 1746806402;
        const GAS_LIMIT: u64 = 200_000;

        let mut db = prepare_jovian_db(DA_FOOTPRINT_GAS_SCALAR);
        let op_chain_hardforks = OpChainHardforks::new(
            OpHardfork::op_mainnet()
                .into_iter()
                .chain(vec![(OpHardfork::Jovian, ForkCondition::Timestamp(JOVIAN_TIMESTAMP))]),
        );

        let receipt_builder = OpAlloyReceiptBuilder::default();
        let mut executor = build_executor(
            &mut db,
            &receipt_builder,
            &op_chain_hardforks,
            GAS_LIMIT,
            JOVIAN_TIMESTAMP,
        );

        let tx_inner = TxLegacy { gas_limit: GAS_LIMIT, ..Default::default() };

        let tx = Recovered::new_unchecked(
            OpTxEnvelope::Legacy(tx_inner.into_signed(Signature::new(
                Default::default(),
                Default::default(),
                Default::default(),
            ))),
            Address::ZERO,
        );
        let tx_env = tx.to_tx_env();

        assert!(executor.da_footprint_used == 0);

        let expected_da_footprint = executor.jovian_da_footprint_estimation(&tx_env, &tx).unwrap();

        // make sure we can use both `WithEncoded` and transaction itself as inputs.
        let gas_used_tx =
            executor.execute_transaction(&tx).expect("failed to execute transaction").tx_gas_used();

        // The gas used when executing the transaction should be the legacy value...
        assert!(gas_used_tx < expected_da_footprint);

        // The gas used when finishing the executor should be the DA footprint since this is higher
        // than the legacy gas used and jovian is active...
        let (_, result) = executor.finish().expect("failed to finish executor");
        assert_eq!(result.blob_gas_used, expected_da_footprint);
        assert_eq!(result.gas_used, gas_used_tx);
        assert!(result.blob_gas_used > result.gas_used);
    }
}
