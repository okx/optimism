use crate::{
    conditional::MaybeConditionalTransaction, estimated_da_size::DataAvailabilitySized,
    interop::MaybeInteropTransaction,
};
use alloy_consensus::{BlobTransactionValidationError, Typed2718, transaction::Recovered};
use alloy_eips::{
    eip2718::{Encodable2718, WithEncoded},
    eip2930::AccessList,
    eip7594::BlobTransactionSidecarVariant,
    eip7702::SignedAuthorization,
};
use alloy_evm::FromTxWithEncoded;
use alloy_primitives::{Address, B256, Bytes, TxHash, TxKind, U256};
use alloy_rpc_types_eth::erc4337::TransactionConditional;
use c_kzg::KzgSettings;
use core::fmt::Debug;
use op_alloy_consensus::TxEip8130;
use op_revm::{OpEip8130TxTr, transaction::eip8130::Eip8130Parts};
use reth_evm::execute::WithTxEnv;
use reth_optimism_evm::OpTx;
use reth_optimism_primitives::OpTransactionSigned;
use reth_primitives_traits::{InMemorySize, SignedTransaction};
use reth_transaction_pool::{
    EthBlobTransactionSidecar, EthPoolTransaction, EthPooledTransaction, PoolTransaction,
};
use std::{
    borrow::Cow,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

/// Marker for no-interop transactions
pub(crate) const NO_INTEROP_TX: u64 = 0;

/// Pool transaction for OP.
///
/// This type wraps the actual transaction and caches values that are frequently used by the pool.
/// For payload building this lazily tracks values that are required during payload building:
///  - Estimated compressed size of this transaction
#[derive(Debug, Clone, derive_more::Deref)]
pub struct OpPooledTransaction<
    Cons = OpTransactionSigned,
    Pooled = op_alloy_consensus::OpPooledTransaction,
> {
    #[deref]
    inner: EthPooledTransaction<Cons>,
    /// The estimated size of this transaction, lazily computed.
    estimated_tx_compressed_size: OnceLock<u64>,
    /// The pooled transaction type.
    _pd: core::marker::PhantomData<Pooled>,

    /// Optional conditional attached to this transaction.
    conditional: Option<Box<TransactionConditional>>,

    /// Optional interop deadline attached to this transaction.
    interop: Arc<AtomicU64>,

    /// Cached EIP-2718 encoded bytes of the transaction, lazily computed.
    encoded_2718: OnceLock<Bytes>,

    /// Cached executable transaction environment for payload building.
    tx_env: OnceLock<OpTx>,

    /// Cached EIP-8130 parts resolved during AA validation. The side AA pool
    /// reuses this to derive invalidation rules without rebuilding native
    /// auth state while holding its write lock.
    eip8130_parts: OnceLock<Eip8130Parts>,
}

impl<Cons: SignedTransaction, Pooled> OpPooledTransaction<Cons, Pooled> {
    /// Create new instance of [Self].
    pub fn new(transaction: Recovered<Cons>, encoded_length: usize) -> Self {
        Self {
            inner: EthPooledTransaction::new(transaction, encoded_length),
            estimated_tx_compressed_size: Default::default(),
            conditional: None,
            interop: Arc::new(AtomicU64::new(NO_INTEROP_TX)),
            _pd: core::marker::PhantomData,
            encoded_2718: Default::default(),
            tx_env: Default::default(),
            eip8130_parts: Default::default(),
        }
    }

    /// Returns the estimated compressed size of a transaction in bytes.
    /// This value is computed based on the following formula:
    /// `max(minTransactionSize, intercept + fastlzCoef*fastlzSize) / 1e6`
    /// Uses cached EIP-2718 encoded bytes to avoid recomputing the encoding for each estimation.
    pub fn estimated_compressed_size(&self) -> u64 {
        *self
            .estimated_tx_compressed_size
            .get_or_init(|| op_alloy_flz::tx_estimated_size_fjord_bytes(self.encoded_2718()))
    }

    /// Returns lazily computed EIP-2718 encoded bytes of the transaction.
    pub fn encoded_2718(&self) -> &Bytes {
        self.encoded_2718.get_or_init(|| self.inner.transaction().encoded_2718().into())
    }

    /// Builds the executable transaction environment for this pooled transaction.
    fn tx_env_slow(&self) -> OpTx
    where
        OpTx: FromTxWithEncoded<Cons>,
    {
        OpTx::from_encoded_tx(
            self.inner.transaction.inner(),
            self.inner.transaction.signer(),
            self.encoded_2718().clone(),
        )
    }

    /// Returns the cached executable transaction environment.
    pub fn tx_env(&self) -> &OpTx
    where
        OpTx: FromTxWithEncoded<Cons>,
    {
        self.tx_env.get_or_init(|| self.tx_env_slow())
    }

    /// Returns a [`WithTxEnv`] wrapper containing the cached transaction environment.
    pub fn into_with_tx_env(mut self) -> WithTxEnv<OpTx, Recovered<Cons>>
    where
        OpTx: FromTxWithEncoded<Cons>,
    {
        let tx_env = self.tx_env.take().unwrap_or_else(|| self.tx_env_slow());
        WithTxEnv { tx_env, tx: Arc::new(self.inner.transaction) }
    }

    /// Conditional setter.
    pub fn with_conditional(mut self, conditional: TransactionConditional) -> Self {
        self.conditional = Some(Box::new(conditional));
        self
    }
}

impl<Cons, Pooled> MaybeConditionalTransaction for OpPooledTransaction<Cons, Pooled> {
    fn set_conditional(&mut self, conditional: TransactionConditional) {
        self.conditional = Some(Box::new(conditional))
    }

    fn conditional(&self) -> Option<&TransactionConditional> {
        self.conditional.as_deref()
    }
}

impl<Cons, Pooled> MaybeInteropTransaction for OpPooledTransaction<Cons, Pooled> {
    fn set_interop_deadline(&self, deadline: u64) {
        self.interop.store(deadline, Ordering::Relaxed);
    }

    fn interop_deadline(&self) -> Option<u64> {
        let interop = self.interop.load(Ordering::Relaxed);
        if interop > NO_INTEROP_TX {
            return Some(interop);
        }
        None
    }
}

impl<Cons: SignedTransaction, Pooled> DataAvailabilitySized for OpPooledTransaction<Cons, Pooled> {
    fn estimated_da_size(&self) -> u64 {
        self.estimated_compressed_size()
    }
}

/// Sentinel cost reported to reth's inner ETH validator for AA txs. The AA
/// payer-keyed balance check (in [`crate::validate_eip8130_transaction`])
/// is authoritative; the inner check (`cost > balance`) is keyed on the
/// envelope signer, which is the wrong account on sponsored shapes.
/// Returning `&U256::ZERO` makes the inner predicate vacuous without
/// needing to mutate `self.inner.cost`.
const AA_INNER_COST_SENTINEL: U256 = U256::ZERO;

impl<Cons, Pooled> PoolTransaction for OpPooledTransaction<Cons, Pooled>
where
    Cons: SignedTransaction + From<Pooled> + OpEip8130TxTr,
    Pooled: SignedTransaction + TryFrom<Cons, Error: core::error::Error>,
{
    type TryFromConsensusError = <Pooled as TryFrom<Cons>>::Error;
    type Consensus = Cons;
    type Pooled = Pooled;

    fn clone_into_consensus(&self) -> Recovered<Self::Consensus> {
        self.inner.transaction().clone()
    }

    fn consensus_ref(&self) -> Recovered<&Self::Consensus> {
        Recovered::new_unchecked(self.inner.transaction.inner(), self.inner.transaction.signer())
    }

    fn into_consensus(self) -> Recovered<Self::Consensus> {
        self.inner.transaction
    }

    fn into_consensus_with2718(self) -> WithEncoded<Recovered<Self::Consensus>> {
        let encoding = self.encoded_2718().clone();
        self.inner.transaction.into_encoded_with(encoding)
    }

    fn from_pooled(tx: Recovered<Self::Pooled>) -> Self {
        let encoded_len = tx.encode_2718_len();
        Self::new(tx.convert(), encoded_len)
    }

    fn hash(&self) -> &TxHash {
        self.inner.transaction.tx_hash()
    }

    fn sender(&self) -> Address {
        self.inner.transaction.signer()
    }

    fn sender_ref(&self) -> &Address {
        self.inner.transaction.signer_ref()
    }

    fn cost(&self) -> &U256 {
        // AA txs: see `AA_INNER_COST_SENTINEL`. The AA validator wrapper
        // re-runs the balance check against the correct (payer) account
        // before emitting the final outcome.
        if self.inner.transaction.inner().as_eip8130().is_some() {
            return &AA_INNER_COST_SENTINEL;
        }
        &self.inner.cost
    }

    fn encoded_length(&self) -> usize {
        self.inner.encoded_length
    }

    fn requires_nonce_check(&self) -> bool {
        // AA txs key off `NONCE_MANAGER_ADDRESS[aa_nonce_slot(sender, key)]`,
        // which is unrelated to `account.nonce`. Skipping the inner check
        // (also paired with the `nonce()` override below) lets AA admission
        // run the spec layer's nonce check instead. Non-AA txs keep reth's
        // default `true`.
        self.inner.transaction.inner().as_eip8130().is_none()
    }
}

impl<Cons: Typed2718, Pooled> Typed2718 for OpPooledTransaction<Cons, Pooled> {
    fn ty(&self) -> u8 {
        self.inner.ty()
    }
}

impl<Cons: InMemorySize, Pooled> InMemorySize for OpPooledTransaction<Cons, Pooled> {
    fn size(&self) -> usize {
        self.inner.size()
    }
}

impl<Cons, Pooled> alloy_consensus::Transaction for OpPooledTransaction<Cons, Pooled>
where
    Cons: alloy_consensus::Transaction + OpEip8130TxTr,
    Pooled: Debug + Send + Sync + 'static,
{
    fn chain_id(&self) -> Option<u64> {
        self.inner.chain_id()
    }

    fn nonce(&self) -> u64 {
        // AA txs report `u64::MAX - 1` so reth's `tx.nonce() < account.nonce`
        // predicate (transaction-pool `eth.rs:708`) never rejects on the
        // inner stateful path. We can't use `u64::MAX` because reth's
        // EIP-2681 stateless check rejects exactly that value
        // (`eth.rs:437-441`). The AA validator wrapper computes the
        // authoritative state nonce from `NONCE_MANAGER_ADDRESS` and
        // overwrites the outcome's `state_nonce` before propagation.
        // `requires_nonce_check()` is also `false` for AA, so this acts
        // as belt-and-suspenders against future inner refactors that
        // bypass the predicate flag.
        if self.inner.transaction.inner().as_eip8130().is_some() {
            return u64::MAX - 1;
        }
        self.inner.nonce()
    }

    fn gas_limit(&self) -> u64 {
        self.inner.gas_limit()
    }

    fn gas_price(&self) -> Option<u128> {
        self.inner.gas_price()
    }

    fn max_fee_per_gas(&self) -> u128 {
        self.inner.max_fee_per_gas()
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        self.inner.max_priority_fee_per_gas()
    }

    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        self.inner.max_fee_per_blob_gas()
    }

    fn priority_fee_or_price(&self) -> u128 {
        self.inner.priority_fee_or_price()
    }

    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        self.inner.effective_gas_price(base_fee)
    }

    fn is_dynamic_fee(&self) -> bool {
        self.inner.is_dynamic_fee()
    }

    fn kind(&self) -> TxKind {
        self.inner.kind()
    }

    fn is_create(&self) -> bool {
        self.inner.is_create()
    }

    fn value(&self) -> U256 {
        self.inner.value()
    }

    fn input(&self) -> &Bytes {
        self.inner.input()
    }

    fn access_list(&self) -> Option<&AccessList> {
        self.inner.access_list()
    }

    fn blob_versioned_hashes(&self) -> Option<&[B256]> {
        self.inner.blob_versioned_hashes()
    }

    fn authorization_list(&self) -> Option<&[SignedAuthorization]> {
        self.inner.authorization_list()
    }
}

impl<Cons, Pooled> EthPoolTransaction for OpPooledTransaction<Cons, Pooled>
where
    Cons: SignedTransaction + From<Pooled> + OpEip8130TxTr,
    Pooled: SignedTransaction + TryFrom<Cons>,
    <Pooled as TryFrom<Cons>>::Error: core::error::Error,
{
    fn take_blob(&mut self) -> EthBlobTransactionSidecar {
        EthBlobTransactionSidecar::None
    }

    fn try_into_pooled_eip4844(
        self,
        _sidecar: Arc<BlobTransactionSidecarVariant>,
    ) -> Option<Recovered<Self::Pooled>> {
        None
    }

    fn try_from_eip4844(
        _tx: Recovered<Self::Consensus>,
        _sidecar: BlobTransactionSidecarVariant,
    ) -> Option<Self> {
        None
    }

    fn validate_blob(
        &self,
        _sidecar: &BlobTransactionSidecarVariant,
        _settings: &KzgSettings,
    ) -> Result<(), BlobTransactionValidationError> {
        Err(BlobTransactionValidationError::NotBlobTransaction(self.ty()))
    }
}

/// Helper trait to provide payload builder with access to conditionals and encoded bytes of
/// transaction.
pub trait OpPooledTx:
    MaybeConditionalTransaction + MaybeInteropTransaction + PoolTransaction + DataAvailabilitySized
{
    /// Returns the EIP-2718 encoded bytes of the transaction.
    fn encoded_2718(&self) -> Cow<'_, Bytes>;

    /// Returns the inner [`TxEip8130`] if this transaction is an EIP-8130 AA transaction.
    ///
    /// Returns `None` for any other transaction type, and also for consensus-tx wrappers
    /// that do not expose the EIP-8130 variant. The mempool AA-validation path uses this to
    /// pull the structured AA fields without re-decoding the EIP-2718 envelope.
    fn as_eip8130(&self) -> Option<&TxEip8130> {
        None
    }

    /// Precomputes the executable transaction environment, if the concrete
    /// pooled transaction stores one.
    fn precompute_tx_env(&self)
    where
        OpTx: FromTxWithEncoded<Self::Consensus>,
    {
    }

    /// Precomputes an EIP-8130 transaction environment with parts that were
    /// already built during validation. Implementations that cannot store the
    /// prepared environment may fall back to the generic precompute path.
    fn precompute_eip8130_tx_env(&self, _parts: Eip8130Parts)
    where
        OpTx: FromTxWithEncoded<Self::Consensus>,
    {
        self.precompute_tx_env();
    }

    /// Returns cached EIP-8130 parts if the validator already resolved them.
    fn cached_eip8130_parts(&self) -> Option<&Eip8130Parts> {
        None
    }

    /// Consumes this pool transaction with a prepared transaction environment.
    fn into_with_tx_env(self) -> WithTxEnv<OpTx, Recovered<Self::Consensus>>
    where
        Self: Sized,
        OpTx: FromTxWithEncoded<Self::Consensus>;
}

impl<Cons, Pooled> OpPooledTx for OpPooledTransaction<Cons, Pooled>
where
    Cons: SignedTransaction + From<Pooled> + OpEip8130TxTr,
    Pooled: SignedTransaction + TryFrom<Cons>,
    <Pooled as TryFrom<Cons>>::Error: core::error::Error,
{
    fn encoded_2718(&self) -> Cow<'_, Bytes> {
        Cow::Borrowed(self.encoded_2718())
    }

    fn as_eip8130(&self) -> Option<&TxEip8130> {
        self.inner.transaction.inner().as_eip8130().map(|sealed| sealed.inner())
    }

    fn precompute_tx_env(&self)
    where
        OpTx: FromTxWithEncoded<Cons>,
    {
        let _ = self.tx_env();
    }

    fn precompute_eip8130_tx_env(&self, parts: Eip8130Parts)
    where
        OpTx: FromTxWithEncoded<Cons>,
    {
        let _ = self.eip8130_parts.set(parts.clone());
        if let Some(tx) = self.inner.transaction.inner().as_eip8130().map(|sealed| sealed.inner()) {
            let tx_env = OpTx::from_eip8130_parts(
                tx,
                self.inner.transaction.signer(),
                self.encoded_2718().clone(),
                parts,
            );
            let _ = self.tx_env.set(tx_env);
        } else {
            let _ = self.tx_env();
        }
    }

    fn cached_eip8130_parts(&self) -> Option<&Eip8130Parts> {
        self.eip8130_parts.get()
    }

    fn into_with_tx_env(self) -> WithTxEnv<OpTx, Recovered<Self::Consensus>>
    where
        Self: Sized,
        OpTx: FromTxWithEncoded<Cons>,
    {
        OpPooledTransaction::into_with_tx_env(self)
    }
}

#[cfg(test)]
mod tests {
    use crate::{OpPooledTransaction, OpTransactionValidator};
    use alloy_consensus::transaction::Recovered;
    use alloy_eips::eip2718::Encodable2718;
    use alloy_primitives::{TxKind, U256};
    use op_alloy_consensus::TxDeposit;
    use reth_optimism_chainspec::OP_MAINNET;
    use reth_optimism_evm::OpEvmConfig;
    use reth_optimism_primitives::{OpPrimitives, OpTransactionSigned};
    use reth_provider::test_utils::MockEthProvider;
    use reth_transaction_pool::{
        TransactionOrigin, TransactionValidationOutcome, blobstore::InMemoryBlobStore,
        validate::EthTransactionValidatorBuilder,
    };
    #[tokio::test]
    async fn validate_optimism_transaction() {
        let client = MockEthProvider::<OpPrimitives>::new()
            .with_chain_spec(OP_MAINNET.clone())
            .with_genesis_block();
        let evm_config = OpEvmConfig::optimism(OP_MAINNET.clone());
        let validator = EthTransactionValidatorBuilder::new(client, evm_config)
            .no_shanghai()
            .no_cancun()
            .build(InMemoryBlobStore::default());
        let validator = OpTransactionValidator::new(validator);

        let origin = TransactionOrigin::External;
        let signer = Default::default();
        let deposit_tx = TxDeposit {
            source_hash: Default::default(),
            from: signer,
            to: TxKind::Create,
            mint: 0,
            value: U256::ZERO,
            gas_limit: 0,
            is_system_transaction: false,
            input: Default::default(),
        };
        let signed_tx: OpTransactionSigned = deposit_tx.into();
        let signed_recovered = Recovered::new_unchecked(signed_tx, signer);
        let len = signed_recovered.encode_2718_len();
        let pooled_tx: OpPooledTransaction = OpPooledTransaction::new(signed_recovered, len);
        let outcome = validator.validate_one(origin, pooled_tx).await;

        let err = match outcome {
            TransactionValidationOutcome::Invalid(_, err) => err,
            _ => panic!("Expected invalid transaction"),
        };
        assert_eq!(err.to_string(), "transaction type not supported");
    }
}
