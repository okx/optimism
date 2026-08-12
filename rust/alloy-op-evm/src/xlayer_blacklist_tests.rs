//! X Layer blacklist integration tests.

use super::*;
use alloy_consensus::{SignableTransaction, TxLegacy};
use alloy_evm::{Evm, EvmEnv, EvmFactory, FromRecoveredTx};
use alloy_primitives::{B256, Signature, TxKind, U256};
use op_revm::transaction::{OpTxTr, deposit::DepositTransactionParts};
use revm::{
    Database,
    context::{BlockEnv, CfgEnv, TxEnv},
    database::{DBErrorMarker, InMemoryDB},
    state::{AccountInfo, Bytecode, EvmState},
};

fn legacy_op_tx(nonce: u64, caller: Address, target: Address) -> OpTx {
    let tx = TxLegacy { nonce, gas_limit: 100_000, to: TxKind::Call(target), ..Default::default() }
        .into_signed(Signature::new(Default::default(), Default::default(), Default::default()));

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
        deposit: DepositTransactionParts { source_hash, mint: Some(mint), is_system_transaction },
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
        revm::context::result::ExecutionResult::Halt { reason: OpHaltReason::FailedDeposit, .. }
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
        revm::context::result::ExecutionResult::Halt { reason: OpHaltReason::FailedDeposit, .. }
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
        0x60, 0x01, 0x60, 0x00, 0x55, 0x73, 0x99, 0x99, 0x99, 0x99, 0x99, 0x99, 0x99, 0x99, 0x99,
        0x99, 0x99, 0x99, 0x99, 0x99, 0x99, 0x99, 0x99, 0x99, 0x99, 0x99, 0x3b, 0x50, 0x00,
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
