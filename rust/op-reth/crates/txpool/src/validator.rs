use crate::{
    InvalidCrossTx, OpPooledTx, interop_filter::InteropFilterClient,
    xlayer_gasless::GaslessBlockMetrics,
};
use alloy_consensus::{BlockHeader, Header, Transaction};
use alloy_primitives::U256;
use op_revm::L1BlockInfo;
use parking_lot::RwLock;
use reth_chainspec::{ChainSpecProvider, EthChainSpec};
use reth_evm::{ConfigureEvm, block::BlockExecutionError};
use reth_optimism_evm::{GaslessContract, OpEvmConfig, RethL1BlockInfo, xlayer_gasless_contract};
use reth_optimism_forks::OpHardforks;
use reth_primitives_traits::{
    Block, BlockBody, BlockTy, GotExpected, SealedBlock,
    transaction::error::InvalidTransactionError,
};
use reth_revm::database::StateProviderDatabase;
use reth_storage_api::{AccountInfoReader, BlockReaderIdExt, HeaderProvider, StateProviderFactory};
use reth_transaction_pool::{
    EthPoolTransaction, EthTransactionValidator, TransactionOrigin, TransactionValidationOutcome,
    TransactionValidator, error::InvalidPoolTransactionError,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use tracing::warn;

/// The timeout for cross-chain transaction validation against the interop filter.
pub(crate) const CHECK_ACCESS_LIST_TIMEOUT_SECS: u64 = 7200;

/// Tracks additional infos for the current block.
#[derive(Debug, Default)]
pub struct OpL1BlockInfo {
    /// The current L1 block info.
    l1_block_info: RwLock<L1BlockInfo>,
    /// Current block timestamp.
    timestamp: AtomicU64,
}

impl OpL1BlockInfo {
    /// Returns the most recent timestamp
    pub fn timestamp(&self) -> u64 {
        self.timestamp.load(Ordering::Relaxed)
    }
}

/// Validator for Optimism transactions.
#[derive(Debug, Clone)]
pub struct OpTransactionValidator<Client, Tx, Evm> {
    /// The type that performs the actual validation.
    inner: Arc<EthTransactionValidator<Client, Tx, Evm>>,
    /// Additional block info required for validation.
    block_info: Arc<OpL1BlockInfo>,
    /// If true, ensure that the transaction's sender has enough balance to cover the L1 gas fee
    /// derived from the tracked L1 block info that is extracted from the first transaction in the
    /// L2 block.
    require_l1_data_gas_fee: bool,
    /// Client used to check transaction validity with the interop filter.
    interop_client: Option<InteropFilterClient>,
    /// tracks activated forks relevant for transaction validation
    fork_tracker: Arc<OpForkTracker>,
    /// When true, zero-priced ("gasless") transactions are admitted only if the on-chain gasless
    /// contract approves them (see `apply_xlayer_gasless_check`).
    allow_gasless: bool,
    /// Counters for gasless admission rejections. Shares the `optimism_transaction_pool.gasless`
    /// metrics scope with the block-level gasless metrics.
    gasless_metrics: GaslessBlockMetrics,
}

impl<Client, Tx, Evm> OpTransactionValidator<Client, Tx, Evm> {
    /// Returns the configured chain spec
    pub fn chain_spec(&self) -> Arc<Client::ChainSpec>
    where
        Client: ChainSpecProvider,
    {
        self.inner.chain_spec()
    }

    /// Returns the configured client
    pub fn client(&self) -> &Client {
        self.inner.client()
    }

    /// Returns the current block timestamp.
    fn block_timestamp(&self) -> u64 {
        self.block_info.timestamp.load(Ordering::Relaxed)
    }

    /// Whether to ensure that the transaction's sender has enough balance to also cover the L1 gas
    /// fee.
    pub fn require_l1_data_gas_fee(self, require_l1_data_gas_fee: bool) -> Self {
        Self { require_l1_data_gas_fee, ..self }
    }

    /// Returns whether this validator also requires the transaction's sender to have enough balance
    /// to cover the L1 gas fee.
    pub const fn requires_l1_data_gas_fee(&self) -> bool {
        self.require_l1_data_gas_fee
    }
}

impl<Client, Tx, Evm> OpTransactionValidator<Client, Tx, Evm>
where
    Client: ChainSpecProvider<ChainSpec: OpHardforks + EthChainSpec<Header = Header>>
        + StateProviderFactory
        + BlockReaderIdExt
        + HeaderProvider<Header = Header>
        + Sync,
    Tx: EthPoolTransaction + OpPooledTx,
    Evm: ConfigureEvm,
{
    /// Create a new [`OpTransactionValidator`].
    pub fn new(inner: EthTransactionValidator<Client, Tx, Evm>) -> Self {
        let this = Self::with_block_info(inner, OpL1BlockInfo::default());
        if let Ok(Some(block)) =
            this.inner.client().block_by_number_or_tag(alloy_eips::BlockNumberOrTag::Latest)
        {
            // genesis block has no txs, so we can't extract L1 info, we set the block info to empty
            // so that we will accept txs into the pool before the first block
            if block.header().number() == 0 {
                this.block_info.timestamp.store(block.header().timestamp(), Ordering::Relaxed);
            } else {
                this.update_l1_block_info(block.header(), block.body().transactions().first());
            }
        }

        this
    }

    /// Create a new [`OpTransactionValidator`] with the given [`OpL1BlockInfo`].
    pub fn with_block_info(
        inner: EthTransactionValidator<Client, Tx, Evm>,
        block_info: OpL1BlockInfo,
    ) -> Self {
        Self {
            inner: Arc::new(inner),
            block_info: Arc::new(block_info),
            require_l1_data_gas_fee: true,
            interop_client: None,
            fork_tracker: Arc::new(OpForkTracker { interop: AtomicBool::from(false) }),
            allow_gasless: false,
            gasless_metrics: GaslessBlockMetrics::default(),
        }
    }

    /// Sets the interop filter client and safety level.
    pub fn with_interop(mut self, interop_client: InteropFilterClient) -> Self {
        self.interop_client = Some(interop_client);
        self
    }

    /// Enables the `XLayer` gasless admission gate: zero-priced transactions are accepted only when
    /// the on-chain gasless contract approves them.
    pub const fn with_gasless(mut self, allow_gasless: bool) -> Self {
        self.allow_gasless = allow_gasless;
        self
    }

    /// Update the L1 block info for the given header and system transaction, if any.
    ///
    /// Note: this supports optional system transaction, in case this is used in a dev setup
    pub fn update_l1_block_info<H, T>(&self, header: &H, tx: Option<&T>)
    where
        H: BlockHeader,
        T: Transaction,
    {
        self.block_info.timestamp.store(header.timestamp(), Ordering::Relaxed);

        if let Some(Ok(l1_block_info)) = tx.map(reth_optimism_evm::extract_l1_info_from_tx) {
            *self.block_info.l1_block_info.write() = l1_block_info;
        }

        if self.chain_spec().is_interop_active_at_timestamp(header.timestamp()) {
            self.fork_tracker.interop.store(true, Ordering::Relaxed);
        }
    }

    /// Validates a single transaction.
    ///
    /// See also [`TransactionValidator::validate_transaction`]
    ///
    /// This behaves the same as [`OpTransactionValidator::validate_one_with_state`], but creates
    /// a new state provider internally.
    pub async fn validate_one(
        &self,
        origin: TransactionOrigin,
        transaction: Tx,
    ) -> TransactionValidationOutcome<Tx> {
        self.validate_one_with_state(origin, transaction, &mut None).await
    }

    /// Validates a single transaction with a provided state provider.
    ///
    /// This allows reusing the same state provider across multiple transaction validations.
    ///
    /// See also [`TransactionValidator::validate_transaction`]
    ///
    /// This behaves the same as [`EthTransactionValidator::validate_one_with_state`], but in
    /// addition applies OP validity checks:
    /// - ensures tx is not eip4844
    /// - ensures cross chain transactions are valid wrt locally configured safety level
    /// - ensures that the account has enough balance to cover the L1 gas cost
    pub async fn validate_one_with_state(
        &self,
        origin: TransactionOrigin,
        transaction: Tx,
        state: &mut Option<Box<dyn AccountInfoReader + Send>>,
    ) -> TransactionValidationOutcome<Tx> {
        if transaction.is_eip4844() {
            return TransactionValidationOutcome::Invalid(
                transaction,
                InvalidTransactionError::TxTypeNotSupported.into(),
            );
        }

        // Interop cross tx validation
        match self.is_valid_cross_tx(&transaction).await {
            Some(Err(err)) => {
                let err = match err {
                    InvalidCrossTx::CrossChainTxPreInterop => {
                        InvalidTransactionError::TxTypeNotSupported.into()
                    }
                    err => InvalidPoolTransactionError::Other(Box::new(err)),
                };
                return TransactionValidationOutcome::Invalid(transaction, err);
            }
            Some(Ok(_)) => {
                // valid interop tx
                transaction
                    .set_interop_deadline(self.block_timestamp() + CHECK_ACCESS_LIST_TIMEOUT_SECS);
            }
            _ => {}
        }

        let outcome = self.inner.validate_one_with_state(origin, transaction, state);

        self.apply_xlayer_gasless_check(self.apply_op_checks(outcome))
    }

    /// `XLayer` gasless admission gate.
    ///
    /// A zero-priced transaction (`max_fee_per_gas == 0`) is only admissible when the on-chain
    /// gasless contract approves it: `getGaslessAllowance(to, input)` must return `allowed == true`
    /// and a `gasLimit` not exceeded by the tx. This mirrors the executor's gasless decision so
    /// non-eligible zero-priced txs are rejected at `add_transaction` time rather than being
    /// admitted and failing at block execution. No-op unless gasless is enabled or the tx is not
    /// zero-priced.
    #[inline]
    fn apply_xlayer_gasless_check(
        &self,
        outcome: TransactionValidationOutcome<Tx>,
    ) -> TransactionValidationOutcome<Tx> {
        if !self.allow_gasless {
            return outcome;
        }
        let TransactionValidationOutcome::Valid {
            balance,
            state_nonce,
            transaction: valid_tx,
            propagate,
            bytecode_hash,
            authorities,
        } = outcome
        else {
            return outcome;
        };

        // Only zero-priced transactions are subject to the gasless gate; everything else passes
        // through unchanged.
        if valid_tx.transaction().max_fee_per_gas() != 0 {
            return TransactionValidationOutcome::Valid {
                balance,
                state_nonce,
                transaction: valid_tx,
                propagate,
                bytecode_hash,
                authorities,
            };
        }

        // The zero-priced tx must be approved by the on-chain gasless contract, mirroring the
        // executor's `is_gasless` decision so admission matches execution. Run the contract view
        // call against the latest committed state; reject if not whitelisted or if the tx's gas
        // limit exceeds the contract's per-tx allowance, and surface an error if the state read /
        // EVM call fails.
        match self.gasless_allowance(valid_tx.transaction()) {
            Ok((false, _)) => {
                self.gasless_metrics.gasless_rejected_not_whitelisted.increment(1);
                TransactionValidationOutcome::Invalid(
                    valid_tx.into_transaction(),
                    InvalidPoolTransactionError::Underpriced,
                )
            }
            Ok((true, gas_limit)) if valid_tx.transaction().gas_limit() > gas_limit => {
                self.gasless_metrics.gasless_rejected_gas_limit_exceeded.increment(1);
                let tx_gas_limit = valid_tx.transaction().gas_limit();
                TransactionValidationOutcome::Invalid(
                    valid_tx.into_transaction(),
                    InvalidPoolTransactionError::MaxTxGasLimitExceeded(tx_gas_limit, gas_limit),
                )
            }
            Ok((true, _)) => TransactionValidationOutcome::Valid {
                balance,
                state_nonce,
                transaction: valid_tx,
                propagate,
                bytecode_hash,
                authorities,
            },
            Err(err) => {
                warn!(
                    target: "txpool::gasless",
                    tx = %valid_tx.hash(),
                    %err,
                    "gasless allowance check failed; rejecting zero-priced tx",
                );
                TransactionValidationOutcome::Error(*valid_tx.hash(), Box::new(err))
            }
        }
    }

    /// Runs the on-chain gasless contract's `getGaslessAllowance(to, input)` against the latest
    /// committed state and returns the reported `(allowed, gas_limit)`.
    ///
    /// This reuses the exact same [`GaslessContract`] the block executor uses, so a tx admitted
    /// here is the one the executor will treat as gasless. Returns `(false, 0)` when the chain has
    /// no gasless contract or the latest header is unavailable.
    fn gasless_allowance(&self, tx: &Tx) -> Result<(bool, u64), BlockExecutionError> {
        // No gasless contract on this chain => never gasless. Derived from the chain id so the
        // address matches what the executor uses (see `xlayer_gasless_contract`).
        let Some(contract) = xlayer_gasless_contract(self.chain_spec().chain().id()) else {
            return Ok((false, 0));
        };
        let Some(header) = self.client().latest_header().map_err(BlockExecutionError::other)?
        else {
            // Abnormal: we have a gasless contract for this chain but no latest header to read
            // state against.
            warn!(
                target: "txpool::gasless",
                "latest header unavailable during gasless allowance check; treating tx as non-gasless",
            );
            return Ok((false, 0));
        };
        let state = self.client().latest().map_err(BlockExecutionError::other)?;
        // The inner `EthTransactionValidator` does not carry an EVM config, so build a
        // gasless-aware `OpEvmConfig` from the chain spec. It derives the same gasless contract by
        // chain id, keeping the validation view consensus-uniform with the block executor.
        let evm_config = OpEvmConfig::optimism(self.chain_spec());
        let mut evm = evm_config
            .evm_for_block(StateProviderDatabase::new(state), header.header())
            .map_err(BlockExecutionError::other)?;
        let consensus = tx.clone_into_consensus().into_inner();
        GaslessContract::new(contract).get_gasless_allowance(&mut evm, &consensus)
    }

    /// Performs the necessary opstack specific checks based on top of the regular eth outcome.
    fn apply_op_checks(
        &self,
        outcome: TransactionValidationOutcome<Tx>,
    ) -> TransactionValidationOutcome<Tx> {
        if !self.requires_l1_data_gas_fee() {
            // no need to check L1 gas fee
            return outcome;
        }
        // ensure that the account has enough balance to cover the L1 gas cost
        if let TransactionValidationOutcome::Valid {
            balance,
            state_nonce,
            transaction: valid_tx,
            propagate,
            bytecode_hash,
            authorities,
        } = outcome
        {
            // Gasless (zero fee-cap) candidates pay no L1 data fee or L2 execution fee when
            // executed gaslessly, so their admission must not require ETH to cover the
            // L1 data fee.
            let cost_addition =
                if self.allow_gasless && valid_tx.transaction().max_fee_per_gas() == 0 {
                    U256::ZERO
                } else {
                    let mut l1_block_info = self.block_info.l1_block_info.read().clone();
                    let encoded = valid_tx.transaction().encoded_2718();
                    match l1_block_info.l1_tx_data_fee(
                        self.chain_spec(),
                        self.block_timestamp(),
                        &encoded,
                        false,
                    ) {
                        Ok(cost) => cost,
                        Err(err) => {
                            return TransactionValidationOutcome::Error(
                                *valid_tx.hash(),
                                Box::new(err),
                            );
                        }
                    }
                };
            let cost = valid_tx.transaction().cost().saturating_add(cost_addition);

            // Checks for max cost
            if cost > balance {
                return TransactionValidationOutcome::Invalid(
                    valid_tx.into_transaction(),
                    InvalidTransactionError::InsufficientFunds(
                        GotExpected { got: balance, expected: cost }.into(),
                    )
                    .into(),
                );
            }

            return TransactionValidationOutcome::Valid {
                balance,
                state_nonce,
                transaction: valid_tx,
                propagate,
                bytecode_hash,
                authorities,
            };
        }
        outcome
    }

    /// Wrapper for is valid cross tx
    pub async fn is_valid_cross_tx(&self, tx: &Tx) -> Option<Result<(), InvalidCrossTx>> {
        // We don't need to check for deposit transaction in here, because they won't come from
        // txpool
        self.interop_client
            .as_ref()?
            .is_valid_cross_tx(
                tx.access_list(),
                tx.hash(),
                self.block_info.timestamp.load(Ordering::Relaxed),
                Some(CHECK_ACCESS_LIST_TIMEOUT_SECS),
                self.fork_tracker.is_interop_activated(),
            )
            .await
    }
}

impl<Client, Tx, Evm> TransactionValidator for OpTransactionValidator<Client, Tx, Evm>
where
    Client: ChainSpecProvider<ChainSpec: OpHardforks + EthChainSpec<Header = Header>>
        + StateProviderFactory
        + BlockReaderIdExt
        + HeaderProvider<Header = Header>
        + Sync,
    Tx: EthPoolTransaction + OpPooledTx,
    Evm: ConfigureEvm,
{
    type Transaction = Tx;
    type Block = BlockTy<Evm::Primitives>;

    async fn validate_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> TransactionValidationOutcome<Self::Transaction> {
        self.validate_one(origin, transaction).await
    }

    fn on_new_head_block(&self, new_tip_block: &SealedBlock<Self::Block>) {
        self.inner.on_new_head_block(new_tip_block);
        self.update_l1_block_info(
            new_tip_block.header(),
            new_tip_block.body().transactions().first(),
        );
    }
}

/// Keeps track of whether certain forks are activated
#[derive(Debug)]
pub(crate) struct OpForkTracker {
    /// Tracks if interop is activated at the block's timestamp.
    interop: AtomicBool,
}

impl OpForkTracker {
    /// Returns `true` if Lagoon fork is activated.
    pub(crate) fn is_interop_activated(&self) -> bool {
        self.interop.load(Ordering::Relaxed)
    }
}
