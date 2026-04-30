use crate::{
    Eip8130InvalidationIndex, Eip8130ValidationError, InvalidCrossTx, OpPooledTx,
    VerifierAdmissionPolicy, VerifierPurityCache,
    eip8130_validate::{DEFAULT_CUSTOM_VERIFIER_GAS_LIMIT, validate_eip8130_transaction},
    supervisor::SupervisorClient,
};
use std::collections::HashSet;
use alloy_consensus::{BlockHeader, Transaction};
use op_revm::L1BlockInfo;
use parking_lot::RwLock;
use reth_chainspec::{ChainSpecProvider, EthChainSpec};
use reth_evm::ConfigureEvm;
use reth_optimism_evm::RethL1BlockInfo;
use reth_optimism_forks::OpHardforks;
use reth_primitives_traits::{
    Block, BlockBody, BlockTy, GotExpected, SealedBlock,
    transaction::error::InvalidTransactionError,
};
use reth_storage_api::{AccountInfoReader, BlockReaderIdExt, StateProviderFactory};
use reth_transaction_pool::{
    EthPoolTransaction, EthTransactionValidator, TransactionOrigin, TransactionValidationOutcome,
    TransactionValidator, error::InvalidPoolTransactionError,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

/// The timeout for cross-chain transaction validation against the supervisor/interop-filter.
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
    /// Client used to check transaction validity with op-supervisor
    supervisor_client: Option<SupervisorClient>,
    /// tracks activated forks relevant for transaction validation
    fork_tracker: Arc<OpForkTracker>,
    /// Mempool admission policy for EIP-8130 verifiers (defaults to AllowlistOrPure
    /// with empty allowlist — only pure custom verifiers + native verifiers admitted).
    eip8130_verifier_policy: Arc<VerifierAdmissionPolicy>,
    /// Verifier purity verdict cache, keyed by runtime bytecode hash.
    eip8130_purity_cache: Arc<VerifierPurityCache>,
    /// Per-storage-slot invalidation index for AA transactions in the pool.
    eip8130_invalidation_index: Arc<RwLock<Eip8130InvalidationIndex>>,
    /// Gas limit for custom verifier STATICCALLs in the txpool.
    eip8130_custom_verifier_gas_limit: u64,
    /// Trusted payer-account bytecode hashes (currently unused — reserved for the
    /// trusted-payer optimization base uses to skip on-chain payer auth re-checks).
    eip8130_trusted_payer_bytecodes: Arc<HashSet<alloy_primitives::B256>>,
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
    Client:
        ChainSpecProvider<ChainSpec: OpHardforks> + StateProviderFactory + BlockReaderIdExt + Sync,
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
            supervisor_client: None,
            fork_tracker: Arc::new(OpForkTracker { interop: AtomicBool::from(false) }),
            eip8130_verifier_policy: Arc::new(VerifierAdmissionPolicy::default()),
            eip8130_purity_cache: Arc::new(VerifierPurityCache::default()),
            eip8130_invalidation_index: Arc::new(RwLock::new(
                Eip8130InvalidationIndex::default(),
            )),
            eip8130_custom_verifier_gas_limit: DEFAULT_CUSTOM_VERIFIER_GAS_LIMIT,
            eip8130_trusted_payer_bytecodes: Arc::new(HashSet::new()),
        }
    }

    /// Replace the EIP-8130 verifier admission policy.
    pub fn with_eip8130_verifier_policy(mut self, policy: VerifierAdmissionPolicy) -> Self {
        self.eip8130_verifier_policy = Arc::new(policy);
        self
    }

    /// Override the EIP-8130 custom-verifier STATICCALL gas limit.
    pub fn with_eip8130_custom_verifier_gas_limit(mut self, gas_limit: u64) -> Self {
        self.eip8130_custom_verifier_gas_limit = gas_limit;
        self
    }

    /// Returns a handle to the AA invalidation index. The maintenance task
    /// (canon-state listener) consumes this index to evict stale AA txs.
    pub fn eip8130_invalidation_index(&self) -> Arc<RwLock<Eip8130InvalidationIndex>> {
        Arc::clone(&self.eip8130_invalidation_index)
    }

    /// Set the supervisor client and safety level
    pub fn with_supervisor(mut self, supervisor_client: SupervisorClient) -> Self {
        self.supervisor_client = Some(supervisor_client);
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

        if transaction.ty() == op_alloy_consensus::transaction::eip8130::AA_TX_TYPE_ID {
            if !self.chain_spec().is_eip8130_active_at_timestamp(self.block_timestamp()) {
                return TransactionValidationOutcome::Invalid(
                    transaction,
                    InvalidTransactionError::TxTypeNotSupported.into(),
                );
            }

            return match crate::validate_eip8130_transaction(
                &transaction,
                self.block_timestamp(),
                self.chain_spec().chain().id(),
                self.client(),
                self.eip8130_verifier_policy.as_ref(),
                self.eip8130_purity_cache.as_ref(),
                self.eip8130_custom_verifier_gas_limit,
                &self.eip8130_trusted_payer_bytecodes,
            ) {
                Ok(outcome) => {
                    if self.requires_l1_data_gas_fee() {
                        let mut l1_info = self.block_info.l1_block_info.read().clone();
                        let encoded = transaction.encoded_2718();
                        match l1_info.l1_tx_data_fee(
                            self.chain_spec(),
                            self.block_timestamp(),
                            &encoded,
                            false,
                        ) {
                            Ok(l1_cost) => {
                                let total = transaction.cost().saturating_add(l1_cost);
                                if total > outcome.balance {
                                    return TransactionValidationOutcome::Invalid(
                                        transaction,
                                        InvalidTransactionError::InsufficientFunds(
                                            GotExpected { got: outcome.balance, expected: total }
                                                .into(),
                                        )
                                        .into(),
                                    );
                                }
                            }
                            Err(err) => {
                                return TransactionValidationOutcome::Error(
                                    *transaction.hash(),
                                    Box::new(err),
                                );
                            }
                        }
                    }

                    // EIP8130_POOL_TODO: route ALL AA txs to the 2D nonce pool. base
                    // lines 310-364 — the dedicated `Eip8130Pool` handles expiry,
                    // invalidation, and 2D ordering. Without that pool ported, AA
                    // txs sit in the standard reth pool with linear-nonce ordering,
                    // which is wrong for nonce_key != 0. First cut accepts the
                    // mismatch; the maintenance task still evicts on state changes
                    // via the invalidation index.

                    if !outcome.invalidation_keys.is_empty() {
                        let tx_hash = *transaction.hash();
                        self.eip8130_invalidation_index.write().insert(
                            tx_hash,
                            outcome.invalidation_keys.clone(),
                            outcome.sponsored_payer,
                        );
                    }

                    // EIP8130_METADATA_TODO: base attaches Eip8130Metadata to the
                    // pooled tx via OpPooledTx::attach_aa_metadata (base lines
                    // 375-382). We have not yet ported that metadata struct + the
                    // attach hook on OpPooledTx; downstream consumers (RPC, P2P
                    // dedup) lose visibility into the resolved sender/payer/keys
                    // until that lands.

                    // The standard pool still receives the Valid outcome for
                    // backward-compatible RPC visibility, but `propagate: false`
                    // prevents it from firing its own P2P gossip. (When the
                    // Eip8130Pool ports, its broadcast channel will drive gossip
                    // instead; for now AA tx P2P propagation is suppressed
                    // entirely — same as base.)
                    let _ = origin;
                    let _ = state;
                    TransactionValidationOutcome::Valid {
                        balance: outcome.balance,
                        state_nonce: outcome.state_nonce,
                        transaction: reth_transaction_pool::validate::ValidTransaction::Valid(transaction),
                        propagate: false,
                        bytecode_hash: None,
                        authorities: None,
                    }
                }
                Err(e) => {
                    tracing::debug!(target: "txpool", error = %e, "EIP-8130 transaction validation failed");
                    TransactionValidationOutcome::Invalid(
                        transaction,
                        InvalidPoolTransactionError::Other(Box::new(e)),
                    )
                }
            };
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

        self.apply_op_checks(outcome)
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
            let mut l1_block_info = self.block_info.l1_block_info.read().clone();

            let encoded = valid_tx.transaction().encoded_2718();

            let cost_addition = match l1_block_info.l1_tx_data_fee(
                self.chain_spec(),
                self.block_timestamp(),
                &encoded,
                false,
            ) {
                Ok(cost) => cost,
                Err(err) => {
                    return TransactionValidationOutcome::Error(*valid_tx.hash(), Box::new(err));
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
        self.supervisor_client
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
    Client:
        ChainSpecProvider<ChainSpec: OpHardforks> + StateProviderFactory + BlockReaderIdExt + Sync,
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
    /// Returns `true` if Interop fork is activated.
    pub(crate) fn is_interop_activated(&self) -> bool {
        self.interop.load(Ordering::Relaxed)
    }
}
