//! X Layer blacklist integration tests.

use super::*;
use alloy_consensus::{SignableTransaction, TxLegacy};
use alloy_evm::{Evm, EvmEnv, EvmFactory, FromRecoveredTx};
use alloy_primitives::{B256, Signature, TxKind, U256};
use op_alloy::consensus::TxDeposit;
use op_revm::transaction::{OpTxTr, deposit::DepositTransactionParts};
use revm::{
    Database,
    context::{BlockEnv, CfgEnv, TxEnv},
    database::{DBErrorMarker, InMemoryDB},
    state::{AccountInfo, Bytecode, EvmState},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tracing::{
    Event, Level, Metadata, Subscriber,
    field::{Field, Visit},
    span::Id,
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
        // Unlike `OpTransaction::default()` (which carries a one-byte placeholder), model a deposit
        // whose encoded bytes are unavailable.
        enveloped_tx: None,
        deposit: DepositTransactionParts { source_hash, mint: Some(mint), is_system_transaction },
        ..Default::default()
    })
}

fn deposit_db(query_code: &'static [u8]) -> InMemoryDB {
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
    db
}

fn run_tx<DB>(
    db: DB,
    tx: OpTx,
    cfg: CfgEnv<OpSpecId>,
    block: BlockEnv,
    track_post_exec: bool,
) -> DepositRun
where
    DB: Database + Debug,
    DB::Error: Debug,
{
    let mut evm = OpEvmFactory::<OpTx>::default().create_evm(db, EvmEnv::new(cfg, block));
    evm.set_test_blacklist_contract(TxBlacklistContract::new(BLACKLIST_TEST_CONTRACT));
    if track_post_exec {
        evm.begin_post_exec_tx(post_exec::PostExecTxContext {
            tx_index: 0,
            kind: post_exec::PostExecTxKind::Deposit,
        });
    }

    let result = evm.transact_raw(tx).expect("deposit execution must not abort");
    let context_tx = evm.ctx().tx.clone();
    let tracking_active = evm.post_exec_tracking_active;
    let post_exec = evm.take_last_post_exec_tx_result();

    DepositRun { result, context_tx, tracking_active, post_exec }
}

fn run_deposit(
    query_code: &'static [u8],
    mint: u128,
    spec: OpSpecId,
    track_post_exec: bool,
) -> DepositRun {
    run_tx(
        deposit_db(query_code),
        blacklist_deposit_tx(mint),
        CfgEnv::new_with_spec(spec),
        BlockEnv { gas_limit: 1_000_000, ..Default::default() },
        track_post_exec,
    )
}

/// The alert is the only observable output of a blacklist hit, so the tests read it back through a
/// minimal test-local subscriber. Nothing is added to the production surface for this.
const ALERT_TOKEN: &str = "XL_RCS_ALERT";
const ALERT_TARGET: &str = "alloy_op_evm::xlayer_blacklist";
const ALERT_MESSAGE: &str = "XL_RCS_ALERT force transaction blocked by TxBlacklist";

#[derive(Debug, Clone)]
struct CapturedEvent {
    level: Level,
    target: String,
    fields: BTreeMap<String, String>,
}

struct FieldVisitor<'a>(&'a mut BTreeMap<String, String>);

impl Visit for FieldVisitor<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_owned(), value.to_owned());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        self.0.insert(field.name().to_owned(), format!("{value:?}"));
    }
}

#[derive(Clone, Default)]
struct EventCapture {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl EventCapture {
    fn take(&self) -> Vec<CapturedEvent> {
        let mut events = self.events.lock().unwrap_or_else(|error| error.into_inner());
        core::mem::take(&mut *events)
    }
}

impl Subscriber for EventCapture {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut fields = BTreeMap::new();
        event.record(&mut FieldVisitor(&mut fields));
        self.events.lock().unwrap_or_else(|error| error.into_inner()).push(CapturedEvent {
            level: *event.metadata().level(),
            target: event.metadata().target().to_owned(),
            fields,
        });
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

/// Installing a thread-scoped subscriber rebuilds the process-wide callsite interest cache, so
/// capture windows are serialized; otherwise two concurrently finishing windows could clear each
/// other's interest and silently drop the event.
static CAPTURE_LOCK: Mutex<()> = Mutex::new(());

fn capture_events<R>(f: impl FnOnce() -> R) -> (R, Vec<CapturedEvent>) {
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let capture = EventCapture::default();
    let result = tracing::subscriber::with_default(capture.clone(), f);
    (result, capture.take())
}

fn blacklist_alerts(events: &[CapturedEvent]) -> Vec<&CapturedEvent> {
    events
        .iter()
        .filter(|event| event.fields.get("alert").map(String::as_str) == Some(ALERT_TOKEN))
        .collect()
}

/// Exact expected alert payload; every recorded field is compared, so a missing, extra or
/// placeholder value fails the assertion.
struct ExpectedAlert {
    tx_hash: Option<B256>,
    source_hash: B256,
    from: Address,
    to: TxKind,
    value: U256,
    mint: Option<u128>,
    gas_limit: u64,
    input: Bytes,
    chain_id: u64,
    block_height: U256,
}

impl ExpectedAlert {
    /// Alert expected for [`blacklist_deposit_tx`], which carries no enveloped transaction bytes.
    fn without_enveloped_tx(mint: u128) -> Self {
        Self {
            tx_hash: None,
            source_hash: BLACKLIST_TEST_SOURCE,
            from: BLACKLIST_TEST_CALLER,
            to: TxKind::Call(BLACKLIST_TEST_TARGET),
            value: U256::ZERO,
            mint: Some(mint),
            gas_limit: BLACKLIST_TEST_GAS_LIMIT,
            input: Bytes::new(),
            chain_id: 1,
            block_height: U256::ZERO,
        }
    }

    fn fields(&self) -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        fields.insert("alert".to_owned(), ALERT_TOKEN.to_owned());
        fields.insert("stage".to_owned(), "force_tx_blacklist".to_owned());
        fields.insert("reason".to_owned(), "tx_blacklist_hit".to_owned());
        fields.insert("outcome".to_owned(), "failed_deposit_payload_skipped".to_owned());
        fields.insert("source_hash".to_owned(), self.source_hash.to_string());
        fields.insert("from".to_owned(), self.from.to_string());
        fields.insert("to".to_owned(), format!("{:?}", self.to));
        fields.insert("value".to_owned(), self.value.to_string());
        fields.insert("gas_limit".to_owned(), self.gas_limit.to_string());
        fields.insert("input_len".to_owned(), self.input.len().to_string());
        fields.insert("input_hash".to_owned(), keccak256(&self.input).to_string());
        fields.insert("blacklist_contract".to_owned(), BLACKLIST_TEST_CONTRACT.to_string());
        fields.insert("chain_id".to_owned(), self.chain_id.to_string());
        fields.insert("block_height".to_owned(), self.block_height.to_string());
        fields.insert("message".to_owned(), ALERT_MESSAGE.to_owned());
        if let Some(tx_hash) = self.tx_hash {
            fields.insert("tx_hash".to_owned(), tx_hash.to_string());
        }
        if let Some(mint) = self.mint {
            fields.insert("mint".to_owned(), mint.to_string());
        }
        fields
    }
}

fn assert_single_alert<'a>(
    events: &'a [CapturedEvent],
    expected: &ExpectedAlert,
) -> &'a CapturedEvent {
    let alerts = blacklist_alerts(events);
    assert_eq!(alerts.len(), 1, "expected exactly one alert, got {alerts:#?}");
    let alert = alerts[0];
    assert_eq!(alert.level, Level::WARN);
    assert_eq!(alert.target, ALERT_TARGET);
    assert_eq!(alert.fields, expected.fields());
    alert
}

fn assert_no_alert(events: &[CapturedEvent]) {
    let alerts = blacklist_alerts(events);
    assert!(alerts.is_empty(), "expected no alert, got {alerts:#?}");
}

fn caller_state(state: &EvmState) -> &revm::state::Account {
    state.get(&BLACKLIST_TEST_CALLER).expect("caller state must be persisted")
}

#[test]
fn hit_restores_original_tx_before_catch_error() {
    let (run, events) = capture_events(|| run_deposit(RETURN_TRUE, 10, OpSpecId::REGOLITH, false));
    assert_eq!(run.context_tx.caller(), BLACKLIST_TEST_CALLER);
    assert_eq!(run.context_tx.source_hash(), Some(BLACKLIST_TEST_SOURCE));
    assert_eq!(run.context_tx.mint(), Some(10));
    assert_eq!(run.context_tx.gas_limit(), BLACKLIST_TEST_GAS_LIMIT);
    assert_single_alert(&events, &ExpectedAlert::without_enveloped_tx(10));
}

#[test]
fn hit_with_mint_uses_existing_failed_deposit_path() {
    let (run, events) = capture_events(|| run_deposit(RETURN_TRUE, 10, OpSpecId::REGOLITH, false));
    assert!(matches!(
        run.result.result,
        revm::context::result::ExecutionResult::Halt { reason: OpHaltReason::FailedDeposit, .. }
    ));
    let caller = caller_state(&run.result.state);
    assert_eq!(caller.info.balance, U256::from(10));
    assert_eq!(caller.info.nonce, 1);
    assert!(!run.result.state.contains_key(&BLACKLIST_TEST_TARGET));
    assert_single_alert(&events, &ExpectedAlert::without_enveloped_tx(10));
}

#[test]
fn hit_without_mint_only_bumps_nonce() {
    let (run, events) = capture_events(|| run_deposit(RETURN_TRUE, 0, OpSpecId::REGOLITH, false));
    let caller = caller_state(&run.result.state);
    assert_eq!(caller.info.balance, U256::ZERO);
    assert_eq!(caller.info.nonce, 1);
    assert_single_alert(&events, &ExpectedAlert::without_enveloped_tx(0));
}

#[test]
fn hit_regolith_gas_is_tx_limit() {
    let (run, events) = capture_events(|| run_deposit(RETURN_TRUE, 0, OpSpecId::REGOLITH, false));
    assert_eq!(run.result.result.tx_gas_used(), BLACKLIST_TEST_GAS_LIMIT);
    assert_single_alert(&events, &ExpectedAlert::without_enveloped_tx(0));
}

#[test]
fn hit_pre_regolith_user_gas_matches_existing_rule() {
    let (run, events) = capture_events(|| run_deposit(RETURN_TRUE, 0, OpSpecId::BEDROCK, false));
    assert_eq!(run.result.result.tx_gas_used(), BLACKLIST_TEST_GAS_LIMIT);
    assert_single_alert(&events, &ExpectedAlert::without_enveloped_tx(0));
}

#[test]
fn miss_executes_original_tx() {
    let (run, events) = capture_events(|| run_deposit(RETURN_FALSE, 10, OpSpecId::REGOLITH, false));
    assert!(run.result.result.is_success());
    assert!(run.result.state.contains_key(&BLACKLIST_TEST_TARGET));
    let caller = caller_state(&run.result.state);
    assert_eq!(caller.info.balance, U256::from(10));
    assert_eq!(caller.info.nonce, 1);
    assert_no_alert(&events);
}

#[test]
fn query_failure_executes_original_tx() {
    let (run, events) = capture_events(|| run_deposit(REVERT, 10, OpSpecId::REGOLITH, false));
    assert!(run.result.result.is_success());
    assert!(run.result.state.contains_key(&BLACKLIST_TEST_TARGET));
    assert_no_alert(&events);
}

#[test]
fn hit_runs_common_post_exec_cleanup() {
    let (run, events) = capture_events(|| run_deposit(RETURN_TRUE, 0, OpSpecId::REGOLITH, true));
    assert!(!run.tracking_active);
    assert_eq!(run.post_exec, post_exec::PostExecExecutedTx::default());
    assert_single_alert(&events, &ExpectedAlert::without_enveloped_tx(0));
}

#[test]
fn hit_with_enveloped_deposit_logs_deposit_hash() {
    const CHAIN_ID: u64 = 4242;
    const BLOCK_NUMBER: u64 = 1234;
    const DEPOSIT_INPUT: &[u8] = &[0xca, 0xfe, 0xba, 0xbe];

    // A real encoded deposit carries the EIP-2718 bytes, so the alert can report the transaction
    // hash instead of omitting it.
    let deposit = TxDeposit {
        source_hash: BLACKLIST_TEST_SOURCE,
        from: BLACKLIST_TEST_CALLER,
        to: TxKind::Call(BLACKLIST_TEST_TARGET),
        mint: 10,
        value: U256::from(7),
        gas_limit: BLACKLIST_TEST_GAS_LIMIT,
        is_system_transaction: false,
        input: Bytes::from_static(DEPOSIT_INPUT),
    };
    let tx = OpTx::from_recovered_tx(&deposit, BLACKLIST_TEST_CALLER);

    let (run, events) = capture_events(|| {
        run_tx(
            deposit_db(RETURN_TRUE),
            tx,
            CfgEnv::new_with_spec(OpSpecId::REGOLITH).with_chain_id(CHAIN_ID),
            BlockEnv {
                gas_limit: 1_000_000,
                number: U256::from(BLOCK_NUMBER),
                ..Default::default()
            },
            false,
        )
    });

    assert!(matches!(
        run.result.result,
        revm::context::result::ExecutionResult::Halt { reason: OpHaltReason::FailedDeposit, .. }
    ));
    let caller = caller_state(&run.result.state);
    assert_eq!(caller.info.balance, U256::from(10));
    assert_eq!(caller.info.nonce, 1);
    assert!(!run.result.state.contains_key(&BLACKLIST_TEST_TARGET));

    let alert = assert_single_alert(
        &events,
        &ExpectedAlert {
            tx_hash: Some(deposit.tx_hash()),
            source_hash: BLACKLIST_TEST_SOURCE,
            from: BLACKLIST_TEST_CALLER,
            to: TxKind::Call(BLACKLIST_TEST_TARGET),
            value: U256::from(7),
            mint: Some(10),
            gas_limit: BLACKLIST_TEST_GAS_LIMIT,
            input: Bytes::from_static(DEPOSIT_INPUT),
            chain_id: CHAIN_ID,
            block_height: U256::from(BLOCK_NUMBER),
        },
    );

    // Calldata is never logged, only its length and hash.
    let rendered = format!("{alert:?}").to_lowercase();
    assert!(!rendered.contains("cafebabe"), "raw input must not be logged: {rendered}");
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

/// Database with the test blacklist contract answering `true` and a funded caller.
fn funded_blacklist_db() -> InMemoryDB {
    let mut db = deposit_db(RETURN_TRUE);
    db.insert_account_info(
        BLACKLIST_TEST_CALLER,
        AccountInfo { balance: U256::from(1_000_000_000u64), ..Default::default() },
    );
    db
}

fn gasless_op_tx(nonce: u64, caller: Address, target: Address) -> OpTx {
    let tx = TxLegacy {
        nonce,
        gas_limit: 100_000,
        to: TxKind::Call(target),
        gas_price: 0,
        ..Default::default()
    }
    .into_signed(Signature::new(Default::default(), Default::default(), Default::default()));
    let mut op_tx = OpTx::from_recovered_tx(&tx, caller);
    op_tx.is_gasless = true;
    op_tx
}

#[test]
fn normal_and_gasless_transactions_never_alert() {
    // The blacklist contract answers `true` here; ordinary and gasless transactions must not even
    // be queried, let alone reported.
    let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (results, events) = capture_events(|| {
        let mut legacy_evm = OpEvmFactory::<OpTx>::default().create_evm(
            CountingDb { inner: funded_blacklist_db(), blacklist_reads: reads.clone() },
            EvmEnv::new(
                CfgEnv::new_with_spec(OpSpecId::REGOLITH),
                BlockEnv { gas_limit: 1_000_000, ..Default::default() },
            ),
        );
        legacy_evm.set_test_blacklist_contract(TxBlacklistContract::new(BLACKLIST_TEST_CONTRACT));
        let legacy = legacy_evm
            .transact_raw(legacy_op_tx(0, BLACKLIST_TEST_CALLER, BLACKLIST_TEST_TARGET))
            .expect("legacy transaction executes");

        let mut gasless_evm = OpEvmFactory::<OpTx>::default().create_evm(
            CountingDb { inner: funded_blacklist_db(), blacklist_reads: reads.clone() },
            EvmEnv::new(
                CfgEnv::new_with_spec(OpSpecId::REGOLITH),
                BlockEnv { gas_limit: 1_000_000, ..Default::default() },
            ),
        );
        gasless_evm.set_test_blacklist_contract(TxBlacklistContract::new(BLACKLIST_TEST_CONTRACT));
        let gasless = gasless_evm
            .transact_raw(gasless_op_tx(0, BLACKLIST_TEST_CALLER, BLACKLIST_TEST_TARGET))
            .expect("gasless transaction executes");

        (legacy, gasless)
    });

    assert!(results.0.result.is_success());
    assert!(results.1.result.is_success());
    assert_eq!(
        reads.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "non-deposit transactions must not query the blacklist"
    );
    assert_no_alert(&events);
}

#[test]
fn non_blacklist_failed_deposit_emits_no_alert() {
    // Below the 21_000 intrinsic gas floor: revm's own validation halts this deposit, so it reaches
    // the same `FailedDeposit` outcome without any blacklist hit.
    const INTRINSIC_GAS_TOO_LOW: u64 = 20_000;

    let tx = OpTx(OpTransaction {
        base: TxEnv {
            tx_type: op_revm::transaction::deposit::DEPOSIT_TRANSACTION_TYPE,
            caller: BLACKLIST_TEST_CALLER,
            gas_limit: INTRINSIC_GAS_TOO_LOW,
            kind: TxKind::Call(BLACKLIST_TEST_TARGET),
            ..Default::default()
        },
        deposit: DepositTransactionParts {
            source_hash: BLACKLIST_TEST_SOURCE,
            mint: Some(10),
            is_system_transaction: false,
        },
        ..Default::default()
    });

    let (run, events) = capture_events(|| {
        run_tx(
            deposit_db(RETURN_FALSE),
            tx,
            CfgEnv::new_with_spec(OpSpecId::REGOLITH),
            BlockEnv { gas_limit: 1_000_000, ..Default::default() },
            false,
        )
    });

    assert!(
        matches!(
            run.result.result,
            revm::context::result::ExecutionResult::Halt {
                reason: OpHaltReason::FailedDeposit,
                ..
            }
        ),
        "expected a non-blacklist failed deposit, got {:?}",
        run.result.result
    );
    assert_no_alert(&events);
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
    let (result, events) = capture_events(|| evm.transact_raw(blacklist_deposit_tx(10)).unwrap());

    assert!(matches!(
        result.result,
        revm::context::result::ExecutionResult::Halt { reason: OpHaltReason::FailedDeposit, .. }
    ));
    assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert!(!result.state.contains_key(&BLACKLIST_TEST_TARGET));
    assert_single_alert(&events, &ExpectedAlert::without_enveloped_tx(10));
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
    let (result, events) = capture_events(|| evm.transact_raw(tx).unwrap());

    assert!(result.result.is_success());
    assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), 0);
    assert_no_alert(&events);
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

    let (error, events) =
        capture_events(|| evm.transact_raw(blacklist_deposit_tx(10)).unwrap_err());

    assert!(matches!(error, EVMError::Database(InjectedDbError)));
    assert!(!evm.ctx().journaled_state.state.contains_key(&BLACKLIST_TEST_TARGET));
    assert!(!evm.post_exec_tracking_active);
    assert_eq!(evm.ctx().tx.caller(), BLACKLIST_TEST_CALLER);
    assert_eq!(evm.ctx().tx.source_hash(), Some(BLACKLIST_TEST_SOURCE));
    assert_eq!(evm.take_last_post_exec_tx_result(), post_exec::PostExecExecutedTx::default());
    assert_no_alert(&events);

    evm.begin_post_exec_tx(post_exec::PostExecTxContext {
        tx_index: 1,
        kind: post_exec::PostExecTxKind::Normal,
    });
    let (_, following_events) = capture_events(|| {
        evm.transact_raw(legacy_op_tx(0, BLACKLIST_TEST_CALLER, BLACKLIST_TEST_TARGET))
            .expect("following transaction executes");
    });
    assert_eq!(
        evm.take_last_post_exec_tx_result(),
        post_exec::PostExecExecutedTx::default(),
        "failed blacklist queries must not warm accounts for later transactions"
    );
    assert_no_alert(&following_events);
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

    let (error, events) =
        capture_events(|| evm.transact_raw(blacklist_deposit_tx(10)).unwrap_err());
    assert!(matches!(error, EVMError::Database(InjectedDbError)));
    assert!(!evm.ctx().journaled_state.state.contains_key(&BLACKLIST_TEST_TARGET));
    let query = evm
        .ctx()
        .journaled_state
        .state
        .get(&BLACKLIST_TEST_CONTRACT)
        .expect("query account was loaded");
    assert!(!query.is_touched());
    let slot = query.storage.get(&U256::ZERO).expect("query wrote slot zero before failing");
    assert_eq!(slot.present_value(), U256::ZERO);
    assert!(!slot.is_changed());
    assert_no_alert(&events);
}
