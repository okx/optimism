#![doc = include_str!("../README.md")]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/alloy-rs/core/main/assets/alloy.jpg",
    html_favicon_url = "https://raw.githubusercontent.com/alloy-rs/core/main/assets/favicon.ico"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod env;
#[cfg(feature = "engine")]
pub use env::evm_env_for_op_payload;
pub use env::{
    evm_env_for_op_block, evm_env_for_op_next_block, spec, spec_by_timestamp_after_bedrock,
};

pub mod error;
pub use error::{OpTxError, map_op_err};

use alloy_evm::{Database, Evm, EvmEnv, EvmFactory, IntoTxEnv, precompiles::PrecompilesMap};
use alloy_primitives::{Address, Bytes};
use core::{
    fmt::Debug,
    marker::PhantomData,
    ops::{Deref, DerefMut},
};
use op_revm::{
    L1BlockInfo, OpBuilder, OpHaltReason, OpSpecId, OpTransaction, OpTransactionError,
    constants::{BASE_FEE_RECIPIENT, L1_FEE_RECIPIENT, OPERATOR_FEE_RECIPIENT},
    handler::OpHandler,
    precompiles::OpPrecompiles,
};
use revm::{
    Context, ExecuteEvm, InspectEvm, Inspector, Journal, MainContext, SystemCallEvm,
    context::{BlockEnv, CfgEnv, DBErrorMarker, TxEnv},
    context_interface::{
        ContextSetters, JournalTr, Transaction,
        result::{EVMError, InvalidTransaction, ResultAndState},
    },
    handler::{EthFrame, Handler, PrecompileProvider, instructions::EthInstructions},
    inspector::NoOpInspector,
    interpreter::{InterpreterResult, interpreter::EthInterpreter},
};

pub mod tx;
pub use tx::OpTx;

pub mod block;
pub use block::{
    GaslessContract, OpBlockExecutionCtx, OpBlockExecutor, OpBlockExecutorFactory, PostExecMode,
    PreRefundGasUsed, TxBlacklistContract, XLAYER_DEVNET_BLACKLIST_CONTRACT,
    XLAYER_DEVNET_GASLESS_CONTRACT, XLAYER_MAINNET_BLACKLIST_CONTRACT,
    XLAYER_MAINNET_GASLESS_CONTRACT, XLAYER_TESTNET_BLACKLIST_CONTRACT,
    XLAYER_TESTNET_GASLESS_CONTRACT, interceptable_user_deposit_source, xlayer_blacklist_contract,
    xlayer_gasless_contract,
};

pub mod post_exec;

/// The OP EVM context type.
pub type OpEvmContext<DB> = Context<BlockEnv, OpTx, CfgEnv<OpSpecId>, DB, Journal<DB>, L1BlockInfo>;

/// OP EVM implementation.
///
/// This is a wrapper type around the `revm` evm with optional [`Inspector`] (tracing)
/// support. [`Inspector`] support is configurable at runtime because it's part of the underlying
/// [`OpEvm`](op_revm::OpEvm) type.
///
/// The `Tx` type parameter controls the transaction environment type. By default it uses
/// [`OpTx`] which wraps [`OpTransaction<TxEnv>`] and implements the necessary foreign traits.
#[allow(missing_debug_implementations)] // missing revm::OpContext Debug impl
pub struct OpEvm<DB: Database, I, P = OpPrecompiles, Tx = OpTx> {
    inner: op_revm::OpEvm<
        OpEvmContext<DB>,
        post_exec::PostExecCompositeInspector<I>,
        EthInstructions<EthInterpreter, OpEvmContext<DB>>,
        P,
    >,
    inspect: bool,
    post_exec_tracking_active: bool,
    last_tx_post_exec_result: post_exec::PostExecExecutedTx,
    #[cfg(test)]
    test_blacklist_contract: Option<TxBlacklistContract>,
    _tx: PhantomData<Tx>,
}

impl<DB: Database, I, P, Tx> OpEvm<DB, I, P, Tx> {
    /// Consumes self and return the inner EVM instance.
    pub fn into_inner(
        self,
    ) -> op_revm::OpEvm<OpEvmContext<DB>, I, EthInstructions<EthInterpreter, OpEvmContext<DB>>, P>
    {
        let op_revm::OpEvm(revm::context::Evm {
            ctx,
            inspector,
            instruction,
            precompiles,
            frame_stack,
        }) = self.inner;

        op_revm::OpEvm(revm::context::Evm {
            ctx,
            inspector: inspector.into_inner(),
            instruction,
            precompiles,
            frame_stack,
        })
    }

    /// Provides a reference to the EVM context.
    pub const fn ctx(&self) -> &OpEvmContext<DB> {
        &self.inner.0.ctx
    }

    /// Provides a mutable reference to the EVM context.
    pub const fn ctx_mut(&mut self) -> &mut OpEvmContext<DB> {
        &mut self.inner.0.ctx
    }
}

impl<DB: Database, I, P, Tx> OpEvm<DB, I, P, Tx> {
    /// Creates a new OP EVM instance.
    ///
    /// The `inspect` argument determines whether the configured [`Inspector`] of the given
    /// [`OpEvm`](op_revm::OpEvm) should be invoked on [`Evm::transact`].
    pub fn new(
        evm: op_revm::OpEvm<
            OpEvmContext<DB>,
            I,
            EthInstructions<EthInterpreter, OpEvmContext<DB>>,
            P,
        >,
        inspect: bool,
    ) -> Self {
        let op_revm::OpEvm(revm::context::Evm {
            ctx,
            inspector,
            instruction,
            precompiles,
            frame_stack,
        }) = evm;

        Self {
            inner: op_revm::OpEvm(revm::context::Evm {
                ctx,
                inspector: post_exec::PostExecCompositeInspector::new(inspector),
                instruction,
                precompiles,
                frame_stack,
            }),
            inspect,
            post_exec_tracking_active: false,
            last_tx_post_exec_result: Default::default(),
            #[cfg(test)]
            test_blacklist_contract: None,
            _tx: PhantomData,
        }
    }

    /// Begin post-exec tracking for the next transaction.
    pub fn begin_post_exec_tx(&mut self, ctx: post_exec::PostExecTxContext) {
        self.post_exec_tracking_active = true;
        self.inner.0.inspector.begin_post_exec_tx(ctx);
    }

    fn note_post_exec_account_touch(&mut self, address: Address) {
        self.inner.0.inspector.note_account_touch(address);
    }

    /// Take the extracted post-exec result for the most recently executed transaction.
    pub fn take_last_post_exec_tx_result(&mut self) -> post_exec::PostExecExecutedTx {
        core::mem::take(&mut self.last_tx_post_exec_result)
    }

    /// Snapshot the block-scoped warming state for carry-forward across flashblock executors.
    pub fn warming_state(&self) -> post_exec::WarmingState {
        self.inner.0.inspector.warming_state()
    }

    /// Seed the block-scoped warming state captured from a prior flashblock's executor.
    pub fn seed_warming_state(&mut self, state: post_exec::WarmingState) {
        self.inner.0.inspector.seed_warming_state(state);
    }

    fn blacklist_contract(&self) -> Option<TxBlacklistContract> {
        #[cfg(test)]
        if let Some(contract) = self.test_blacklist_contract {
            return Some(contract);
        }

        block::xlayer_blacklist_contract(self.inner.0.ctx.cfg.chain_id)
            .map(TxBlacklistContract::new)
    }

    #[cfg(test)]
    const fn set_test_blacklist_contract(&mut self, contract: TxBlacklistContract) {
        self.test_blacklist_contract = Some(contract);
    }
}

impl<DB: Database, I, P, Tx> post_exec::PostExecEvm for OpEvm<DB, I, P, Tx>
where
    Self: Evm,
{
    fn begin_post_exec_tx(&mut self, ctx: post_exec::PostExecTxContext) {
        Self::begin_post_exec_tx(self, ctx);
    }

    fn take_last_post_exec_tx_result(&mut self) -> post_exec::PostExecExecutedTx {
        Self::take_last_post_exec_tx_result(self)
    }

    fn warming_state(&self) -> post_exec::WarmingState {
        Self::warming_state(self)
    }

    fn seed_warming_state(&mut self, state: post_exec::WarmingState) {
        Self::seed_warming_state(self, state);
    }
}

impl<Tx> post_exec::PostExecEvmFactoryHooks for OpEvmFactory<Tx>
where
    Tx: IntoTxEnv<Tx> + Into<OpTransaction<TxEnv>> + Default + Clone + Debug,
{
    fn begin_post_exec_tx<DB, I>(evm: &mut Self::Evm<DB, I>, ctx: post_exec::PostExecTxContext)
    where
        DB: Database,
        I: Inspector<Self::Context<DB>>,
    {
        evm.begin_post_exec_tx(ctx);
    }

    fn take_last_post_exec_tx_result<DB, I>(
        evm: &mut Self::Evm<DB, I>,
    ) -> post_exec::PostExecExecutedTx
    where
        DB: Database,
        I: Inspector<Self::Context<DB>>,
    {
        evm.take_last_post_exec_tx_result()
    }

    fn warming_state<DB, I>(evm: &Self::Evm<DB, I>) -> post_exec::WarmingState
    where
        DB: Database,
        I: Inspector<Self::Context<DB>>,
    {
        evm.warming_state()
    }

    fn seed_warming_state<DB, I>(evm: &mut Self::Evm<DB, I>, state: post_exec::WarmingState)
    where
        DB: Database,
        I: Inspector<Self::Context<DB>>,
    {
        evm.seed_warming_state(state);
    }
}

impl<DB: Database, I, P, Tx> Deref for OpEvm<DB, I, P, Tx> {
    type Target = OpEvmContext<DB>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.ctx()
    }
}

impl<DB: Database, I, P, Tx> DerefMut for OpEvm<DB, I, P, Tx> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.ctx_mut()
    }
}

impl<DB, I, P, Tx> OpEvm<DB, I, P, Tx>
where
    DB: Database,
    I: Inspector<OpEvmContext<DB>>,
    P: PrecompileProvider<OpEvmContext<DB>, Output = InterpreterResult>,
    Tx: IntoTxEnv<Tx> + Into<OpTransaction<TxEnv>>,
{
    /// Runs the blacklist query and, on a hit, produces the canonical OP failed-deposit result.
    /// `Ok(None)` means the transaction is ineligible or the deterministic fail-open query did
    /// not return `true`. Local database errors are propagated to prevent node-local execution
    /// decisions.
    fn intercept_blacklisted_deposit(
        &mut self,
        op_tx: &OpTx,
        contract: TxBlacklistContract,
    ) -> Result<Option<ResultAndState<OpHaltReason>>, EVMError<DB::Error, OpTxError>> {
        let Some(source_hash) = block::interceptable_user_deposit_source(op_tx) else {
            return Ok(None);
        };
        let is_blacklisted = contract.is_blacklisted(self, source_hash);

        // The system call replaces the context transaction on both success and error. Restore the
        // original deposit before either propagating an infrastructure error or running common
        // post-exec cleanup, which classifies the current transaction and records warming state.
        self.inner.0.ctx.set_tx(op_tx.clone());

        if !is_blacklisted? {
            return Ok(None);
        }

        let handler =
            OpHandler::<_, EVMError<DB::Error, OpTransactionError>, EthFrame<EthInterpreter>>::new(
            );
        let error = EVMError::Transaction(OpTransactionError::Base(InvalidTransaction::Str(
            "deposit blacklisted".into(),
        )));
        let output = handler.catch_error(&mut self.inner, error);
        let state = self.inner.finalize();
        output.map(|result| Some(ResultAndState::new(result, state))).map_err(map_op_err)
    }
}

impl<DB, I, P, Tx> Evm for OpEvm<DB, I, P, Tx>
where
    DB: Database,
    I: Inspector<OpEvmContext<DB>>,
    P: PrecompileProvider<OpEvmContext<DB>, Output = InterpreterResult>,
    Tx: IntoTxEnv<Tx> + Into<OpTransaction<TxEnv>>,
{
    type DB = DB;
    type Tx = Tx;
    type Error = EVMError<DB::Error, OpTxError>;
    type HaltReason = OpHaltReason;
    type Spec = OpSpecId;
    type BlockEnv = BlockEnv;
    type Precompiles = P;
    type Inspector = I;

    fn block(&self) -> &BlockEnv {
        &self.block
    }

    fn cfg_env(&self) -> &CfgEnv<OpSpecId> {
        &self.cfg
    }

    fn chain_id(&self) -> u64 {
        self.cfg.chain_id
    }

    fn transact_raw(
        &mut self,
        tx: Self::Tx,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        self.last_tx_post_exec_result = post_exec::PostExecExecutedTx::default();

        let op_tx = OpTx(tx.into());
        let pre_basefee = self.inner.0.ctx.block.basefee;
        if op_tx.is_gasless {
            self.inner.0.ctx.block.basefee = 0;
        }

        let track_post_exec = self.post_exec_tracking_active;
        let blacklist_result = self
            .blacklist_contract()
            .map(|contract| self.intercept_blacklisted_deposit(&op_tx, contract))
            .unwrap_or(Ok(None));

        let result = match blacklist_result {
            Ok(Some(result)) => Ok(result),
            Ok(None) if self.inspect || track_post_exec => {
                self.inner.inspect_tx(op_tx).map_err(map_op_err)
            }
            Ok(None) => self.inner.transact(op_tx).map_err(map_op_err),
            Err(error) => Err(error),
        };

        self.inner.0.ctx.block.basefee = pre_basefee;

        if track_post_exec {
            if self.inner.0.ctx.tx.tx_type() !=
                op_revm::transaction::deposit::DEPOSIT_TRANSACTION_TYPE
            {
                self.note_post_exec_account_touch(L1_FEE_RECIPIENT);
                self.note_post_exec_account_touch(BASE_FEE_RECIPIENT);
                if self.inner.0.ctx.cfg.spec.is_enabled_in(OpSpecId::ISTHMUS) {
                    self.note_post_exec_account_touch(OPERATOR_FEE_RECIPIENT);
                }
            }

            self.last_tx_post_exec_result = self.inner.0.inspector.finish_post_exec_tx();
            self.post_exec_tracking_active = false;
        }

        result
    }

    fn transact_system_call(
        &mut self,
        caller: Address,
        contract: Address,
        data: Bytes,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        match self.inner.system_call_with_caller(caller, contract, data) {
            Ok(result) => Ok(result),
            Err(error) => {
                // `op-revm` uses `OpHandler` for system calls. Unlike the default mainnet
                // handler, its non-transaction error path does not discard the journal. A
                // blacklist/gasless fail-open query may therefore have already touched state
                // before a database error. Restore the pre-call journal before the caller runs
                // the real transaction.
                self.inner.0.ctx.journaled_state.discard_tx();
                Err(map_op_err(error))
            }
        }
    }

    fn finish(self) -> (Self::DB, EvmEnv<Self::Spec, Self::BlockEnv>) {
        let Context { block: block_env, cfg: cfg_env, journaled_state, .. } = self.inner.0.ctx;

        (journaled_state.database, EvmEnv { block_env, cfg_env })
    }

    fn set_inspector_enabled(&mut self, enabled: bool) {
        self.inspect = enabled;
    }

    fn components(&self) -> (&Self::DB, &Self::Inspector, &Self::Precompiles) {
        (
            &self.inner.0.ctx.journaled_state.database,
            self.inner.0.inspector.inner(),
            &self.inner.0.precompiles,
        )
    }

    fn components_mut(&mut self) -> (&mut Self::DB, &mut Self::Inspector, &mut Self::Precompiles) {
        (
            &mut self.inner.0.ctx.journaled_state.database,
            self.inner.0.inspector.inner_mut(),
            &mut self.inner.0.precompiles,
        )
    }
}

/// Factory producing [`OpEvm`]s.
///
/// The `Tx` type parameter controls the transaction type used by the created EVMs.
/// By default it uses [`OpTx`] which wraps [`OpTransaction<TxEnv>`] and implements
/// the necessary foreign traits.
#[derive(Debug)]
pub struct OpEvmFactory<Tx = OpTx>(PhantomData<Tx>);

impl<Tx> Clone for OpEvmFactory<Tx> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<Tx> Copy for OpEvmFactory<Tx> {}

impl<Tx> Default for OpEvmFactory<Tx> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<Tx> EvmFactory for OpEvmFactory<Tx>
where
    Tx: IntoTxEnv<Tx> + Into<OpTransaction<TxEnv>> + Default + Clone + Debug,
{
    type Evm<DB: Database, I: Inspector<OpEvmContext<DB>>> = OpEvm<DB, I, Self::Precompiles, Tx>;
    type Context<DB: Database> = OpEvmContext<DB>;
    type Tx = Tx;
    type Error<DBError: DBErrorMarker> = EVMError<DBError, OpTxError>;
    type HaltReason = OpHaltReason;
    type Spec = OpSpecId;
    type BlockEnv = BlockEnv;
    type Precompiles = PrecompilesMap;

    fn create_evm<DB: Database>(
        &self,
        db: DB,
        input: EvmEnv<OpSpecId, BlockEnv>,
    ) -> Self::Evm<DB, NoOpInspector> {
        let spec_id = input.cfg_env.spec;
        let inner = Context::mainnet()
            .with_tx(OpTx(OpTransaction::builder().build_fill()))
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::BEDROCK))
            .with_chain(L1BlockInfo::default())
            .with_db(db)
            .with_block(input.block_env)
            .with_cfg(input.cfg_env)
            .build_op_with_inspector(NoOpInspector {})
            .with_precompiles(PrecompilesMap::from_static(
                OpPrecompiles::new_with_spec(spec_id).precompiles(),
            ));

        OpEvm::new(inner, false)
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        input: EvmEnv<OpSpecId, BlockEnv>,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        let spec_id = input.cfg_env.spec;
        let inner = Context::mainnet()
            .with_tx(OpTx(OpTransaction::builder().build_fill()))
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::BEDROCK))
            .with_chain(L1BlockInfo::default())
            .with_db(db)
            .with_block(input.block_env)
            .with_cfg(input.cfg_env)
            .build_op_with_inspector(inspector)
            .with_precompiles(PrecompilesMap::from_static(
                OpPrecompiles::new_with_spec(spec_id).precompiles(),
            ));

        OpEvm::new(inner, true)
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloy_consensus::{SignableTransaction, TxLegacy};
    use alloy_evm::{
        EvmInternals, FromRecoveredTx,
        precompiles::{Precompile, PrecompileInput},
    };
    use alloy_primitives::{B256, Signature, TxKind, U256};
    use op_revm::{
        precompiles::{bls12_381, bn254_pair},
        transaction::{OpTxTr, deposit::DepositTransactionParts},
    };
    use revm::{
        Database,
        context::CfgEnv,
        database::{DBErrorMarker, EmptyDB, InMemoryDB},
        precompile::PrecompileHalt,
        state::{AccountInfo, Bytecode, EvmState},
    };

    use super::*;

    /// Runtime of a contract that reads (warms) storage slot 0: `PUSH1 0x00; SLOAD; POP; STOP`.
    const WARMING_CONTRACT_CODE: [u8; 5] = [0x60, 0x00, 0x54, 0x50, 0x00];

    fn legacy_op_tx(nonce: u64, caller: Address, target: Address) -> OpTx {
        let tx =
            TxLegacy { nonce, gas_limit: 100_000, to: TxKind::Call(target), ..Default::default() }
                .into_signed(Signature::new(
                    Default::default(),
                    Default::default(),
                    Default::default(),
                ));

        OpTx::from_recovered_tx(&tx, caller)
    }

    const BLACKLIST_TEST_CONTRACT: Address = Address::repeat_byte(0x77);
    const BLACKLIST_TEST_CALLER: Address = Address::repeat_byte(0x11);
    const BLACKLIST_TEST_TARGET: Address = Address::repeat_byte(0x22);
    const BLACKLIST_TEST_SOURCE: B256 = B256::repeat_byte(0x42);
    const BLACKLIST_TEST_GAS_LIMIT: u64 = 100_000;
    const RETURN_TRUE: &[u8] = &[0x60, 0x01, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xf3];
    const RETURN_FALSE: &[u8] = &[0x60, 0x00, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xf3];
    const REVERT: &[u8] = &[0x60, 0x00, 0x60, 0x00, 0xfd];
    const PAYLOAD_SSTORE: &[u8] = &[0x60, 0x01, 0x60, 0x00, 0x55, 0x00];

    struct DepositRun {
        result: ResultAndState<OpHaltReason>,
        context_tx: OpTx,
        tracking_active: bool,
        post_exec: post_exec::PostExecExecutedTx,
    }

    fn blacklist_deposit_tx(mint: u128) -> OpTx {
        blacklist_deposit_tx_from(BLACKLIST_TEST_CALLER, BLACKLIST_TEST_SOURCE, false, mint)
    }

    fn blacklist_deposit_tx_from(
        caller: Address,
        source_hash: B256,
        is_system_transaction: bool,
        mint: u128,
    ) -> OpTx {
        OpTx(OpTransaction {
            base: TxEnv {
                tx_type: op_revm::transaction::deposit::DEPOSIT_TRANSACTION_TYPE,
                caller,
                gas_limit: BLACKLIST_TEST_GAS_LIMIT,
                kind: TxKind::Call(BLACKLIST_TEST_TARGET),
                ..Default::default()
            },
            deposit: DepositTransactionParts {
                source_hash,
                mint: Some(mint),
                is_system_transaction,
            },
            ..Default::default()
        })
    }

    fn run_deposit(
        query_code: &'static [u8],
        mint: u128,
        spec: OpSpecId,
        track_post_exec: bool,
    ) -> DepositRun {
        let mut db = InMemoryDB::default();
        for (address, raw_code) in
            [(BLACKLIST_TEST_CONTRACT, query_code), (BLACKLIST_TEST_TARGET, PAYLOAD_SSTORE)]
        {
            let code = Bytecode::new_raw(Bytes::from_static(raw_code));
            db.insert_account_info(
                address,
                AccountInfo { code_hash: code.hash_slow(), code: Some(code), ..Default::default() },
            );
        }

        let mut evm = OpEvmFactory::<OpTx>::default().create_evm(
            db,
            EvmEnv::new(
                CfgEnv::new_with_spec(spec),
                BlockEnv { gas_limit: 1_000_000, ..Default::default() },
            ),
        );
        evm.set_test_blacklist_contract(TxBlacklistContract::new(BLACKLIST_TEST_CONTRACT));
        if track_post_exec {
            evm.begin_post_exec_tx(post_exec::PostExecTxContext {
                tx_index: 0,
                kind: post_exec::PostExecTxKind::Deposit,
            });
        }

        let tx = blacklist_deposit_tx(mint);
        let result = evm.transact_raw(tx).expect("deposit execution must not abort");
        let context_tx = evm.ctx().tx.clone();
        let tracking_active = evm.post_exec_tracking_active;
        let post_exec = evm.take_last_post_exec_tx_result();

        DepositRun { result, context_tx, tracking_active, post_exec }
    }

    fn caller_state(state: &EvmState) -> &revm::state::Account {
        state.get(&BLACKLIST_TEST_CALLER).expect("caller state must be persisted")
    }

    #[test]
    fn hit_restores_original_tx_before_catch_error() {
        let run = run_deposit(RETURN_TRUE, 10, OpSpecId::REGOLITH, false);
        assert_eq!(run.context_tx.caller(), BLACKLIST_TEST_CALLER);
        assert_eq!(run.context_tx.source_hash(), Some(BLACKLIST_TEST_SOURCE));
        assert_eq!(run.context_tx.mint(), Some(10));
        assert_eq!(run.context_tx.gas_limit(), BLACKLIST_TEST_GAS_LIMIT);
    }

    #[test]
    fn hit_with_mint_uses_existing_failed_deposit_path() {
        let run = run_deposit(RETURN_TRUE, 10, OpSpecId::REGOLITH, false);
        assert!(matches!(
            run.result.result,
            revm::context::result::ExecutionResult::Halt {
                reason: OpHaltReason::FailedDeposit,
                ..
            }
        ));
        let caller = caller_state(&run.result.state);
        assert_eq!(caller.info.balance, U256::from(10));
        assert_eq!(caller.info.nonce, 1);
        assert!(!run.result.state.contains_key(&BLACKLIST_TEST_TARGET));
    }

    #[test]
    fn hit_without_mint_only_bumps_nonce() {
        let run = run_deposit(RETURN_TRUE, 0, OpSpecId::REGOLITH, false);
        let caller = caller_state(&run.result.state);
        assert_eq!(caller.info.balance, U256::ZERO);
        assert_eq!(caller.info.nonce, 1);
    }

    #[test]
    fn hit_regolith_gas_is_tx_limit() {
        let run = run_deposit(RETURN_TRUE, 0, OpSpecId::REGOLITH, false);
        assert_eq!(run.result.result.tx_gas_used(), BLACKLIST_TEST_GAS_LIMIT);
    }

    #[test]
    fn hit_pre_regolith_user_gas_matches_existing_rule() {
        let run = run_deposit(RETURN_TRUE, 0, OpSpecId::BEDROCK, false);
        assert_eq!(run.result.result.tx_gas_used(), BLACKLIST_TEST_GAS_LIMIT);
    }

    #[test]
    fn miss_executes_original_tx() {
        let run = run_deposit(RETURN_FALSE, 10, OpSpecId::REGOLITH, false);
        assert!(run.result.result.is_success());
        assert!(run.result.state.contains_key(&BLACKLIST_TEST_TARGET));
        let caller = caller_state(&run.result.state);
        assert_eq!(caller.info.balance, U256::from(10));
        assert_eq!(caller.info.nonce, 1);
    }

    #[test]
    fn query_failure_executes_original_tx() {
        let run = run_deposit(REVERT, 10, OpSpecId::REGOLITH, false);
        assert!(run.result.result.is_success());
        assert!(run.result.state.contains_key(&BLACKLIST_TEST_TARGET));
    }

    #[test]
    fn hit_runs_common_post_exec_cleanup() {
        let run = run_deposit(RETURN_TRUE, 0, OpSpecId::REGOLITH, true);
        assert!(!run.tracking_active);
        assert_eq!(run.post_exec, post_exec::PostExecExecutedTx::default());
    }

    #[test]
    fn normal_and_gasless_paths_unchanged() {
        let miss = run_deposit(RETURN_FALSE, 10, OpSpecId::REGOLITH, false);
        assert!(miss.result.result.is_success());
        // Dedicated existing tests below cover basefee bypass/restoration for gasless txs; this
        // assertion locks the blacklist-specific invariant that non-deposits are never candidates.
        assert_eq!(
            block::interceptable_user_deposit_source(&legacy_op_tx(
                0,
                BLACKLIST_TEST_CALLER,
                BLACKLIST_TEST_TARGET,
            )),
            None
        );
    }

    #[derive(Debug)]
    struct CountingDb {
        inner: InMemoryDB,
        blacklist_reads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Database for CountingDb {
        type Error = <InMemoryDB as Database>::Error;

        fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
            if address == BLACKLIST_TEST_CONTRACT {
                self.blacklist_reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            self.inner.basic(address)
        }

        fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
            self.inner.code_by_hash(code_hash)
        }

        fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
            self.inner.storage(address, index)
        }

        fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
            self.inner.block_hash(number)
        }
    }

    #[test]
    fn it_query_executes_once_before_deposit() {
        let mut inner = InMemoryDB::default();
        for (address, raw_code) in
            [(BLACKLIST_TEST_CONTRACT, RETURN_TRUE), (BLACKLIST_TEST_TARGET, PAYLOAD_SSTORE)]
        {
            let code = Bytecode::new_raw(Bytes::from_static(raw_code));
            inner.insert_account_info(
                address,
                AccountInfo { code_hash: code.hash_slow(), code: Some(code), ..Default::default() },
            );
        }
        let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let db = CountingDb { inner, blacklist_reads: reads.clone() };
        let mut evm = OpEvmFactory::<OpTx>::default().create_evm(
            db,
            EvmEnv::new(
                CfgEnv::new_with_spec(OpSpecId::REGOLITH),
                BlockEnv { gas_limit: 1_000_000, ..Default::default() },
            ),
        );
        evm.set_test_blacklist_contract(TxBlacklistContract::new(BLACKLIST_TEST_CONTRACT));
        let result = evm.transact_raw(blacklist_deposit_tx(10)).unwrap();

        assert!(matches!(
            result.result,
            revm::context::result::ExecutionResult::Halt {
                reason: OpHaltReason::FailedDeposit,
                ..
            }
        ));
        assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert!(!result.state.contains_key(&BLACKLIST_TEST_TARGET));
    }

    #[test]
    fn attributes_deposit_does_not_query_blacklist() {
        let mut inner = InMemoryDB::default();
        for (address, raw_code) in
            [(BLACKLIST_TEST_CONTRACT, RETURN_TRUE), (BLACKLIST_TEST_TARGET, PAYLOAD_SSTORE)]
        {
            let code = Bytecode::new_raw(Bytes::from_static(raw_code));
            inner.insert_account_info(
                address,
                AccountInfo { code_hash: code.hash_slow(), code: Some(code), ..Default::default() },
            );
        }
        let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let db = CountingDb { inner, blacklist_reads: reads.clone() };
        let mut evm = OpEvmFactory::<OpTx>::default().create_evm(
            db,
            EvmEnv::new(
                CfgEnv::new_with_spec(OpSpecId::REGOLITH),
                BlockEnv { gas_limit: 1_000_000, ..Default::default() },
            ),
        );
        evm.set_test_blacklist_contract(TxBlacklistContract::new(BLACKLIST_TEST_CONTRACT));

        let tx = blacklist_deposit_tx_from(
            block::xlayer_blacklist_contract::L1_ATTRIBUTES_DEPOSITOR,
            BLACKLIST_TEST_SOURCE,
            false,
            0,
        );
        let result = evm.transact_raw(tx).unwrap();

        assert!(result.result.is_success());
        assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    #[derive(Debug)]
    struct InjectedDbError;

    impl core::fmt::Display for InjectedDbError {
        fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            formatter.write_str("injected blacklist read failure")
        }
    }

    impl core::error::Error for InjectedDbError {}
    impl DBErrorMarker for InjectedDbError {}

    #[derive(Debug)]
    struct FailBlacklistReadOnceDb {
        inner: InMemoryDB,
        failed: bool,
    }

    impl Database for FailBlacklistReadOnceDb {
        type Error = InjectedDbError;

        fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
            if address == BLACKLIST_TEST_CONTRACT && !self.failed {
                self.failed = true;
                return Err(InjectedDbError);
            }
            Ok(self.inner.basic(address).expect("InMemoryDB is infallible"))
        }

        fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
            Ok(self.inner.code_by_hash(code_hash).expect("InMemoryDB is infallible"))
        }

        fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
            Ok(self.inner.storage(address, index).expect("InMemoryDB is infallible"))
        }

        fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
            Ok(self.inner.block_hash(number).expect("InMemoryDB is infallible"))
        }
    }

    #[test]
    fn query_database_error_aborts_execution() {
        let mut inner = InMemoryDB::default();
        let code = Bytecode::new_raw(Bytes::from_static(PAYLOAD_SSTORE));
        inner.insert_account_info(
            BLACKLIST_TEST_TARGET,
            AccountInfo { code_hash: code.hash_slow(), code: Some(code), ..Default::default() },
        );
        inner.insert_account_info(
            BLACKLIST_TEST_CALLER,
            AccountInfo { balance: U256::from(1_000_000_000u64), ..Default::default() },
        );
        let db = FailBlacklistReadOnceDb { inner, failed: false };
        let mut evm = OpEvmFactory::<OpTx>::default().create_evm(
            db,
            EvmEnv::new(
                CfgEnv::new_with_spec(OpSpecId::REGOLITH),
                BlockEnv { gas_limit: 1_000_000, ..Default::default() },
            ),
        );
        evm.set_test_blacklist_contract(TxBlacklistContract::new(BLACKLIST_TEST_CONTRACT));

        evm.begin_post_exec_tx(post_exec::PostExecTxContext {
            tx_index: 0,
            kind: post_exec::PostExecTxKind::Deposit,
        });

        let error = evm.transact_raw(blacklist_deposit_tx(10)).unwrap_err();

        assert!(matches!(error, EVMError::Database(InjectedDbError)));
        assert!(!evm.inner.0.ctx.journaled_state.state.contains_key(&BLACKLIST_TEST_TARGET));
        assert!(!evm.post_exec_tracking_active);
        assert_eq!(evm.ctx().tx.caller(), BLACKLIST_TEST_CALLER);
        assert_eq!(evm.ctx().tx.source_hash(), Some(BLACKLIST_TEST_SOURCE));
        assert_eq!(evm.take_last_post_exec_tx_result(), post_exec::PostExecExecutedTx::default());

        evm.begin_post_exec_tx(post_exec::PostExecTxContext {
            tx_index: 1,
            kind: post_exec::PostExecTxKind::Normal,
        });
        evm.transact_raw(legacy_op_tx(0, BLACKLIST_TEST_CALLER, BLACKLIST_TEST_TARGET))
            .expect("following transaction executes");
        assert_eq!(
            evm.take_last_post_exec_tx_result(),
            post_exec::PostExecExecutedTx::default(),
            "failed blacklist queries must not warm accounts for later transactions"
        );
    }

    #[derive(Debug)]
    struct FailAfterQueryWriteDb {
        inner: InMemoryDB,
        failed: bool,
    }

    impl Database for FailAfterQueryWriteDb {
        type Error = InjectedDbError;

        fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
            if address == Address::repeat_byte(0x99) && !self.failed {
                self.failed = true;
                return Err(InjectedDbError);
            }
            Ok(self.inner.basic(address).expect("InMemoryDB is infallible"))
        }

        fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
            Ok(self.inner.code_by_hash(code_hash).expect("InMemoryDB is infallible"))
        }

        fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
            Ok(self.inner.storage(address, index).expect("InMemoryDB is infallible"))
        }

        fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
            Ok(self.inner.block_hash(number).expect("InMemoryDB is infallible"))
        }
    }

    #[test]
    fn query_database_error_after_state_write_discards_journal() {
        // SSTORE(0, 1), then EXTCODESIZE(0x99..99). The latter DB read fails after the query has
        // already journaled a storage write.
        const WRITE_THEN_FAIL: &[u8] = &[
            0x60, 0x01, 0x60, 0x00, 0x55, 0x73, 0x99, 0x99, 0x99, 0x99, 0x99, 0x99, 0x99, 0x99,
            0x99, 0x99, 0x99, 0x99, 0x99, 0x99, 0x99, 0x99, 0x99, 0x99, 0x99, 0x99, 0x3b, 0x50,
            0x00,
        ];

        let mut inner = InMemoryDB::default();
        for (address, raw_code) in
            [(BLACKLIST_TEST_CONTRACT, WRITE_THEN_FAIL), (BLACKLIST_TEST_TARGET, PAYLOAD_SSTORE)]
        {
            let code = Bytecode::new_raw(Bytes::from_static(raw_code));
            inner.insert_account_info(
                address,
                AccountInfo { code_hash: code.hash_slow(), code: Some(code), ..Default::default() },
            );
        }
        let db = FailAfterQueryWriteDb { inner, failed: false };
        let mut evm = OpEvmFactory::<OpTx>::default().create_evm(
            db,
            EvmEnv::new(
                CfgEnv::new_with_spec(OpSpecId::REGOLITH),
                BlockEnv { gas_limit: 1_000_000, ..Default::default() },
            ),
        );
        evm.set_test_blacklist_contract(TxBlacklistContract::new(BLACKLIST_TEST_CONTRACT));

        let error = evm.transact_raw(blacklist_deposit_tx(10)).unwrap_err();
        assert!(matches!(error, EVMError::Database(InjectedDbError)));
        assert!(!evm.inner.0.ctx.journaled_state.state.contains_key(&BLACKLIST_TEST_TARGET));
        let query = evm
            .inner
            .0
            .ctx
            .journaled_state
            .state
            .get(&BLACKLIST_TEST_CONTRACT)
            .expect("query account was loaded");
        assert!(!query.is_touched());
        let slot = query.storage.get(&U256::ZERO).expect("query wrote slot zero before failing");
        assert_eq!(slot.present_value(), U256::ZERO);
        assert!(!slot.is_changed());
    }

    // Verifies the raw OpEvm post-exec hook: this test enables SDM tracking directly with
    // `begin_post_exec_tx` rather than via node config, and confirms it forces the inspector path
    // even when normal tracing is disabled.
    #[test]
    fn op_evm_post_exec_tracking_runs_when_inspector_is_otherwise_disabled() {
        let caller = Address::ZERO;
        let target = Address::from([0x22; 20]);
        let mut db = InMemoryDB::default();
        db.insert_account_info(
            caller,
            AccountInfo { balance: U256::from(1_000_000_000u64), ..Default::default() },
        );
        // `target` is a contract that reads (warms) storage slot 0 (`WARMING_CONTRACT_CODE`).
        // The second tx re-touches that slot cross-tx and earns a genuine warming rebate — a plain
        // value transfer would touch only intrinsic accounts (sender/`to`) and earn nothing.
        db.insert_account_info(
            target,
            AccountInfo {
                code: Some(Bytecode::new_raw(alloy_primitives::Bytes::from_static(
                    &WARMING_CONTRACT_CODE,
                ))),
                ..Default::default()
            },
        );
        let mut evm = OpEvmFactory::<OpTx>::default().create_evm(
            db,
            EvmEnv::new(
                CfgEnv::new_with_spec(OpSpecId::JOVIAN),
                BlockEnv { gas_limit: 1_000_000, ..Default::default() },
            ),
        );
        assert!(!evm.inspect, "factory-created EVM should start with user inspection disabled");

        let mut tracked_refund = |tx_index| {
            evm.begin_post_exec_tx(post_exec::PostExecTxContext {
                tx_index,
                kind: post_exec::PostExecTxKind::Normal,
            });
            // `transact_raw` does not commit state in this low-level test, so reuse nonce 0.
            evm.transact_raw(legacy_op_tx(0, caller, target)).expect("tx executes");
            evm.take_last_post_exec_tx_result().refund_total
        };

        assert_eq!(tracked_refund(0), 0);
        assert!(tracked_refund(1) > 0, "second tx should observe block-warmed addresses");
    }

    #[test]
    fn test_precompiles_jovian_fail() {
        let mut evm = OpEvmFactory::<OpTx>::default().create_evm(
            EmptyDB::default(),
            EvmEnv::new(CfgEnv::new_with_spec(OpSpecId::JOVIAN), BlockEnv::default()),
        );

        let (precompiles, ctx) = (&mut evm.inner.0.precompiles, &mut evm.inner.0.ctx);

        let jovian_precompile = precompiles.get(bn254_pair::JOVIAN.address()).unwrap();
        let result = jovian_precompile
            .call(PrecompileInput {
                data: &vec![0; bn254_pair::JOVIAN_MAX_INPUT_SIZE + 1],
                gas: u64::MAX,
                reservoir: 0,
                caller: Address::ZERO,
                value: U256::ZERO,
                is_static: false,
                target_address: Address::ZERO,
                bytecode_address: Address::ZERO,
                internals: EvmInternals::from_context(ctx),
            })
            .unwrap();

        assert!(result.is_halt());
        assert!(matches!(result.halt_reason(), Some(&PrecompileHalt::Bn254PairLength)));

        let jovian_precompile = precompiles.get(bls12_381::JOVIAN_G1_MSM.address()).unwrap();
        let result = jovian_precompile
            .call(PrecompileInput {
                data: &vec![0; bls12_381::JOVIAN_G1_MSM_MAX_INPUT_SIZE + 1],
                gas: u64::MAX,
                reservoir: 0,
                caller: Address::ZERO,
                value: U256::ZERO,
                is_static: false,
                target_address: Address::ZERO,
                bytecode_address: Address::ZERO,
                internals: EvmInternals::from_context(ctx),
            })
            .unwrap();

        assert!(result.is_halt());
        assert!(matches!(
            result.halt_reason(),
            Some(PrecompileHalt::Other(msg)) if msg.contains("G1MSM input length too long")
        ));

        let jovian_precompile = precompiles.get(bls12_381::JOVIAN_G2_MSM.address()).unwrap();
        let result = jovian_precompile
            .call(PrecompileInput {
                data: &vec![0; bls12_381::JOVIAN_G2_MSM_MAX_INPUT_SIZE + 1],
                gas: u64::MAX,
                reservoir: 0,
                caller: Address::ZERO,
                value: U256::ZERO,
                is_static: false,
                target_address: Address::ZERO,
                bytecode_address: Address::ZERO,
                internals: EvmInternals::from_context(ctx),
            })
            .unwrap();

        assert!(result.is_halt());
        assert!(matches!(
            result.halt_reason(),
            Some(PrecompileHalt::Other(msg)) if msg.contains("G2MSM input length too long")
        ));

        let jovian_precompile = precompiles.get(bls12_381::JOVIAN_PAIRING.address()).unwrap();
        let result = jovian_precompile
            .call(PrecompileInput {
                data: &vec![0; bls12_381::JOVIAN_PAIRING_MAX_INPUT_SIZE + 1],
                gas: u64::MAX,
                reservoir: 0,
                caller: Address::ZERO,
                value: U256::ZERO,
                is_static: false,
                target_address: Address::ZERO,
                bytecode_address: Address::ZERO,
                internals: EvmInternals::from_context(ctx),
            })
            .unwrap();

        assert!(result.is_halt());
        assert!(matches!(
            result.halt_reason(),
            Some(PrecompileHalt::Other(msg)) if msg.contains("Pairing input length too long")
        ));
    }

    #[test]
    fn test_precompiles_jovian() {
        let mut evm = OpEvmFactory::<OpTx>::default().create_evm(
            EmptyDB::default(),
            EvmEnv::new(CfgEnv::new_with_spec(OpSpecId::JOVIAN), BlockEnv::default()),
        );
        let (precompiles, ctx) = (&mut evm.inner.0.precompiles, &mut evm.inner.0.ctx);
        let jovian_precompile = precompiles.get(bn254_pair::JOVIAN.address()).unwrap();
        let result = jovian_precompile.call(PrecompileInput {
            data: &vec![0; bn254_pair::JOVIAN_MAX_INPUT_SIZE],
            gas: u64::MAX,
            reservoir: 0,
            caller: Address::ZERO,
            value: U256::ZERO,
            is_static: false,
            target_address: Address::ZERO,
            bytecode_address: Address::ZERO,
            internals: EvmInternals::from_context(ctx),
        });

        assert!(result.is_ok());

        let jovian_precompile = precompiles.get(bls12_381::JOVIAN_G1_MSM.address()).unwrap();
        let result = jovian_precompile.call(PrecompileInput {
            data: &vec![0; bls12_381::JOVIAN_G1_MSM_MAX_INPUT_SIZE],
            gas: u64::MAX,
            reservoir: 0,
            caller: Address::ZERO,
            value: U256::ZERO,
            is_static: false,
            target_address: Address::ZERO,
            bytecode_address: Address::ZERO,
            internals: EvmInternals::from_context(ctx),
        });

        assert!(result.is_ok());

        let jovian_precompile = precompiles.get(bls12_381::JOVIAN_G2_MSM.address()).unwrap();
        let result = jovian_precompile.call(PrecompileInput {
            data: &vec![0; bls12_381::JOVIAN_G2_MSM_MAX_INPUT_SIZE],
            gas: u64::MAX,
            reservoir: 0,
            caller: Address::ZERO,
            value: U256::ZERO,
            is_static: false,
            target_address: Address::ZERO,
            bytecode_address: Address::ZERO,
            internals: EvmInternals::from_context(ctx),
        });

        assert!(result.is_ok());

        let jovian_precompile = precompiles.get(bls12_381::JOVIAN_PAIRING.address()).unwrap();
        let result = jovian_precompile.call(PrecompileInput {
            data: &vec![0; bls12_381::JOVIAN_PAIRING_MAX_INPUT_SIZE],
            gas: u64::MAX,
            reservoir: 0,
            caller: Address::ZERO,
            value: U256::ZERO,
            is_static: false,
            target_address: Address::ZERO,
            bytecode_address: Address::ZERO,
            internals: EvmInternals::from_context(ctx),
        });

        assert!(result.is_ok());
    }

    mod xlayer_tests {
        use super::*;

        #[test]
        fn test_gasless_tx_bypasses_basefee_check() {
            let env = EvmEnv::new(
                CfgEnv::new_with_spec(OpSpecId::REGOLITH),
                BlockEnv { basefee: 100, gas_limit: 30_000, ..Default::default() },
            );
            let tx = OpTransaction::builder()
                .base(TxEnv::builder().gas_limit(21_000).gas_price(0))
                .build_fill();

            // A zero-priced non-gasless tx is rejected: gas price (0) is below the base fee (100).
            let mut evm = OpEvmFactory::default().create_evm(EmptyDB::default(), env.clone());
            assert!(evm.transact(OpTx(tx.clone())).is_err());

            // The same tx flagged gasless executes because `transact_raw` (which `transact`
            // delegates to) zeroes the base fee for the duration of the tx, then restores it.
            let mut evm = OpEvmFactory::default().create_evm(EmptyDB::default(), env);
            assert!(evm.transact(OpTx(OpTransaction { is_gasless: true, ..tx })).is_ok());
            assert_eq!(evm.ctx().block.basefee, 100);
        }

        #[test]
        fn test_gasless_tx_restores_basefee_when_tx_fails() {
            let mut evm = OpEvmFactory::default().create_evm(
                EmptyDB::default(),
                EvmEnv::new(
                    CfgEnv::new_with_spec(OpSpecId::REGOLITH),
                    BlockEnv { basefee: 100, gas_limit: 20_000, ..Default::default() },
                ),
            );
            // Gas limit (21000) exceeds the block gas limit (20000), so the tx is rejected even
            // though it is gasless: zeroing the base fee only relaxes the fee check, not other
            // validation. This gives us a failing gasless tx to exercise the error path.
            let tx = OpTransaction::builder()
                .base(TxEnv::builder().gas_limit(21_000).gas_price(0))
                .gasless(true)
                .build_fill();

            // `transact_raw` (which `transact` delegates to) restores the base fee on the error
            // path, not just on success.
            assert!(evm.transact(OpTx(tx)).is_err());
            assert_eq!(evm.ctx().block.basefee, 100);
        }
    }
}
