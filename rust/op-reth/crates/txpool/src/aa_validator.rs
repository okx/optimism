//! [`OpAaTransactionValidator`]: a transaction-validator wrapper that layers
//! EIP-8130 (XLayer AA) spec validation on top of an inner validator.
//!
//! Mirrors tempo's [`tempo_pool/src/validator.rs::TempoTransactionValidator`]
//! shape (`inner` + AA-specific checks), with one key difference: tempo runs
//! AA-specific checks *before* the inner ETH validator, then partially
//! delegates back. We keep it strictly additive — inner runs first to enforce
//! reth's standard mempool gates (encoded size, block gas limit, intrinsic
//! gas, EIP-3860, chain_id, fee cap, ...). On a `Valid` outcome for an AA
//! tx, [`crate::validate_eip8130_transaction`] is run against current state
//! to enforce the spec MUSTs (chain_id reasserted, expiry, structural bounds,
//! nonce-manager read, balance vs. cost). If the AA layer rejects, the
//! outcome is converted to `Invalid` carrying the AA error.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use alloy_consensus::BlockHeader;
use alloy_evm::FromTxWithEncoded;
use alloy_primitives::U256;
use op_alloy_consensus::AA_TX_TYPE_ID;
use reth_chainspec::{ChainSpecProvider, EthChainSpec};
use reth_optimism_evm::{OpTx, RethL1BlockInfo};
use reth_optimism_forks::OpHardforks;
use reth_primitives_traits::{SealedBlock, transaction::error::InvalidTransactionError};
use reth_storage_api::StateProviderFactory;
use reth_transaction_pool::{
    PoolTransaction, TransactionOrigin, TransactionValidationOutcome, TransactionValidator,
    error::InvalidPoolTransactionError,
};

use crate::{
    Eip8130PoolTx, Eip8130ValidationError, OpL1BlockInfo, OpPooledTx,
    eip8130_xlayer::validate_eip8130_transaction_with_recovered_sender_and_parts,
};

/// Wraps an inner [`TransactionValidator`] and layers EIP-8130 (XLayer AA)
/// spec validation on top of any AA tx the inner validator accepts.
///
/// Non-AA txs are forwarded unchanged. AA txs first run through the inner
/// validator (which must accept the AA tx-type — register via
/// [`reth_transaction_pool::validate::EthTransactionValidatorBuilder::with_custom_tx_type`]
/// at construction); on a `Valid` outcome, we re-check chain_id / expiry /
/// structural caps / nonce-manager state / balance via
/// [`validate_eip8130_transaction`]. The AA-spec `state_nonce` overrides
/// the inner outcome's `state_nonce` because reth's standard nonce check
/// queries `account.nonce`, which is unrelated to the 2D-nonce sequence
/// stored in `NONCE_MANAGER_ADDRESS`.
#[derive(Debug)]
pub struct OpAaTransactionValidator<V, Client> {
    inner: Arc<V>,
    client: Arc<Client>,
    /// Latest committed-tip block timestamp, mirrored from the inner
    /// validator on every [`TransactionValidator::on_new_head_block`].
    /// Read by AA admission for the `validate_eip8130_transaction`
    /// `block_timestamp` argument so `expiry` checks fire against fresh
    /// time without re-reading the inner validator's private state.
    tip_timestamp: Arc<AtomicU64>,
    /// Shared [`OpL1BlockInfo`] handle (cloned from the inner OP validator
    /// at construction). The AA admission path bypasses the OP wrapper's
    /// L1-data-fee guard (which keys on `tx.sender_ref()`) and instead
    /// folds the L1 component into a payer-keyed balance check; this Arc
    /// gives us the same `L1BlockInfo` snapshot the OP wrapper would have
    /// used. `None` means L1-data-fee accounting is disabled (dev mode or
    /// tests with `require_l1_data_gas_fee = false`).
    l1_block_info: Option<Arc<OpL1BlockInfo>>,
}

impl<V, Client> Clone for OpAaTransactionValidator<V, Client> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            client: Arc::clone(&self.client),
            tip_timestamp: Arc::clone(&self.tip_timestamp),
            l1_block_info: self.l1_block_info.clone(),
        }
    }
}

impl<V, Client> OpAaTransactionValidator<V, Client>
where
    Client: StateProviderFactory + Send + Sync + 'static,
{
    /// Build a new wrapper. `chain_id` is captured upfront so AA admission
    /// doesn't have to read it from the chain spec on every tx.
    pub fn new(inner: V, client: Arc<Client>) -> Self {
        Self {
            inner: Arc::new(inner),
            client,
            tip_timestamp: Arc::new(AtomicU64::new(0)),
            l1_block_info: None,
        }
    }

    /// Returns the configured client.
    pub fn client(&self) -> &Client {
        self.client.as_ref()
    }

    /// Attach a shared [`OpL1BlockInfo`] so AA admission can compute the L1
    /// data fee against the same snapshot the inner OP validator uses.
    /// Construction sites that want sponsored AA balance correctness call
    /// this immediately after [`Self::new`] with `inner.block_info().clone()`.
    pub fn with_l1_block_info(mut self, l1_block_info: Arc<OpL1BlockInfo>) -> Self {
        self.l1_block_info = Some(l1_block_info);
        self
    }

    /// The inner validator. Use sparingly; downstream code should normally
    /// go through this wrapper so AA txs see the spec layer.
    pub fn inner(&self) -> &V {
        &self.inner
    }

    /// Latest committed-tip timestamp (0 before the first head update).
    pub fn tip_timestamp(&self) -> u64 {
        self.tip_timestamp.load(Ordering::Acquire)
    }

    /// Test-only: drive the cached tip timestamp without going through
    /// `on_new_head_block`. Used by routing-level tests that don't run a
    /// full chain.
    #[cfg(test)]
    pub(crate) fn tip_timestamp_for_test_set(&self, ts: u64) {
        self.tip_timestamp.store(ts, Ordering::Release);
    }

    /// Returns whether the AA-specific layer should fire on this transaction.
    fn should_apply_aa_layer<T: Eip8130PoolTx>(&self, tx: &T) -> bool {
        tx.is_eip8130()
    }
}

impl<V, Client> OpAaTransactionValidator<V, Client>
where
    V: TransactionValidator,
    V::Transaction: Eip8130PoolTx + OpPooledTx,
    OpTx: FromTxWithEncoded<<V::Transaction as PoolTransaction>::Consensus>,
    Client: ChainSpecProvider<ChainSpec: OpHardforks + EthChainSpec>
        + StateProviderFactory
        + Send
        + Sync
        + 'static,
{
    /// Compute the L1 data fee for `tx` against the cached
    /// [`OpL1BlockInfo`] snapshot. Returns `Ok(U256::ZERO)` when no
    /// `OpL1BlockInfo` was attached (test fixtures, dev mode). Errors
    /// surface as `TransactionValidationOutcome::Error` upstream.
    ///
    /// Exposed at `pub(crate)` so the dual pool can recompute the
    /// admission-time payer-balance predicate (`gas_limit * max_fee_per_gas
    /// + l1_data_fee`) without threading the value back through a
    /// validator-return tuple — per the user correction (2026-05-11), the
    /// AA pre-validator trait stays at a single-outcome return.
    pub(crate) fn compute_l1_data_fee(
        &self,
        tx: &V::Transaction,
    ) -> Result<U256, Box<dyn core::error::Error + Send + Sync>> {
        let Some(l1_block_info) = self.l1_block_info.as_ref() else {
            return Ok(U256::ZERO);
        };
        let encoded = OpPooledTx::encoded_2718(tx);
        let mut snapshot = l1_block_info.l1_block_info_snapshot();
        let timestamp = l1_block_info.timestamp();
        snapshot
            .l1_tx_data_fee(self.client.chain_spec(), timestamp, encoded.as_ref(), false)
            .map_err(|e| -> Box<dyn core::error::Error + Send + Sync> { Box::new(e) })
    }

    /// Apply the EIP-8130 spec layer to a `Valid` inner outcome. On success
    /// for AA txs, the AA-derived `state_nonce` (read from `NONCE_MANAGER`)
    /// overwrites the inner validator's ETH-style `state_nonce` field on the
    /// returned `Valid` variant. The side pool reads it back via the
    /// standard `TransactionValidationOutcome::Valid::state_nonce` field.
    fn apply_aa_layer(
        &self,
        outcome: TransactionValidationOutcome<V::Transaction>,
    ) -> TransactionValidationOutcome<V::Transaction> {
        let TransactionValidationOutcome::Valid {
            balance,
            state_nonce: inner_state_nonce,
            bytecode_hash,
            transaction: valid_tx,
            propagate,
            authorities,
        } = outcome
        else {
            return outcome;
        };

        // Only AA txs need the spec layer.
        if !self.should_apply_aa_layer(valid_tx.transaction()) {
            return TransactionValidationOutcome::Valid {
                balance,
                state_nonce: inner_state_nonce,
                bytecode_hash,
                transaction: valid_tx,
                propagate,
                authorities,
            };
        }

        // Gate on the XLayer V1 fork before opening state — the AA tx-type
        // is meaningless before activation. Mirrors the gate previously
        // inlined in `OpTransactionValidator::validate_one_with_state`.
        let chain_spec = self.client.chain_spec();
        let block_ts = self.tip_timestamp();
        if !chain_spec.is_xlayer_v1_active_at_timestamp(block_ts) {
            return TransactionValidationOutcome::Invalid(
                valid_tx.into_transaction(),
                InvalidTransactionError::TxTypeNotSupported.into(),
            );
        }

        let Some(aa_tx) = OpPooledTx::as_eip8130(valid_tx.transaction()) else {
            // tx-type byte claims AA but the wrapper does not expose the
            // variant — unreachable for `OpTransactionSigned` in practice;
            // reject defensively.
            return TransactionValidationOutcome::Invalid(
                valid_tx.into_transaction(),
                InvalidTransactionError::TxTypeNotSupported.into(),
            );
        };
        let encoded_len = valid_tx.transaction().encoded_2718().len();

        let chain_id = self.client().chain_spec().chain_id();
        let _span = tracing::debug_span!(
            target: "txpool::eip8130",
            "validate_aa",
            hash = %valid_tx.hash(),
            chain_id,
            block_ts,
        )
        .entered();

        // Compute the L1 data fee against the cached `OpL1BlockInfo`
        // snapshot so the AA balance check sees the same number the inner
        // OP validator would have used (and so the inner check can safely
        // skip AA — `apply_op_checks` is a no-op for AA on the OP wrapper).
        // `None` here means L1-data-fee accounting is disabled (dev mode);
        // pass `U256::ZERO` and let the caller's gas-only check stand.
        let l1_data_fee = match self.compute_l1_data_fee(valid_tx.transaction()) {
            Ok(fee) => fee,
            Err(err) => {
                return TransactionValidationOutcome::Error(*valid_tx.hash(), err);
            }
        };

        // Run validation against the borrow so we don't deep-clone
        // `Vec<Vec<Eip8130CallEntry>>` + `Vec<AccountChangeEntry>` per
        // admission (cf PERF-2). Scope the borrow so it's dropped before
        // the error arm calls `valid_tx.into_transaction()`.
        let validation_result = validate_eip8130_transaction_with_recovered_sender_and_parts(
            aa_tx,
            encoded_len,
            block_ts,
            chain_id,
            self.client.as_ref(),
            l1_data_fee,
            Some(valid_tx.transaction().sender()),
        );

        match validation_result {
            Ok(validated) => {
                let aa_state_nonce = validated.state_nonce;
                valid_tx.transaction().precompute_eip8130_tx_env(validated.parts);
                TransactionValidationOutcome::Valid {
                    balance,
                    // The AA spec layer is authoritative on `state_nonce` — it
                    // reads `aa_nonce_slot(sender, key)` from
                    // `NONCE_MANAGER_ADDRESS`, which is unrelated to the
                    // ETH-style `account.nonce` reported by the inner validator.
                    state_nonce: aa_state_nonce,
                    bytecode_hash,
                    transaction: valid_tx,
                    propagate,
                    authorities,
                }
            }
            Err(err) => {
                tracing::debug!(target: "txpool::eip8130", %err, "EIP-8130 mempool validation failed");
                if matches!(&err, Eip8130ValidationError::StateError(_)) {
                    return TransactionValidationOutcome::Error(*valid_tx.hash(), Box::new(err));
                }
                TransactionValidationOutcome::Invalid(
                    valid_tx.into_transaction(),
                    InvalidPoolTransactionError::Other(Box::new(err)),
                )
            }
        }
    }

    /// Validate a single transaction by running the inner validator first
    /// then layering AA-spec checks. Public so [`crate::OpDualPool`] can
    /// call it directly for AA admission instead of going through the
    /// protocol pool, while still sharing the same validator state.
    pub async fn validate_one(
        &self,
        origin: TransactionOrigin,
        transaction: V::Transaction,
    ) -> TransactionValidationOutcome<V::Transaction> {
        let outcome = self.inner.validate_transaction(origin, transaction).await;
        self.apply_aa_layer(outcome)
    }
}

impl<V, Client> TransactionValidator for OpAaTransactionValidator<V, Client>
where
    V: TransactionValidator,
    V::Transaction: Eip8130PoolTx + OpPooledTx,
    OpTx: FromTxWithEncoded<<V::Transaction as PoolTransaction>::Consensus>,
    Client: ChainSpecProvider<ChainSpec: OpHardforks + EthChainSpec>
        + StateProviderFactory
        + Send
        + Sync
        + 'static,
{
    type Transaction = V::Transaction;
    type Block = V::Block;

    async fn validate_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> TransactionValidationOutcome<Self::Transaction> {
        self.validate_one(origin, transaction).await
    }

    async fn validate_transactions(
        &self,
        transactions: impl IntoIterator<Item = (TransactionOrigin, Self::Transaction), IntoIter: Send>
        + Send,
    ) -> Vec<TransactionValidationOutcome<Self::Transaction>> {
        // Delegate per-tx so the AA layer fires on every Valid outcome.
        // Calling `inner.validate_transactions` and then mapping would skip
        // the layer in the trait's default join_all impl and risks losing
        // AA-spec rejections for batch admission paths.
        futures_util::future::join_all(
            transactions.into_iter().map(|(origin, tx)| self.validate_one(origin, tx)),
        )
        .await
    }

    fn on_new_head_block(&self, new_tip_block: &SealedBlock<Self::Block>) {
        self.inner.on_new_head_block(new_tip_block);
        self.tip_timestamp.store(new_tip_block.header().timestamp(), Ordering::Release);
    }
}

// `AA_TX_TYPE_ID` is referenced in the doc comment but not in code; pin
// the dependency so the doc link is checkable.
#[allow(dead_code)]
const _AA_TX_TYPE_ID_FOR_DOC: u8 = AA_TX_TYPE_ID;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OpPooledTransaction, OpTransactionValidator};
    use alloy_consensus::{Sealable, transaction::Recovered};
    use alloy_eips::eip2718::Encodable2718;
    use alloy_primitives::{Address, B256, Bytes, U256};
    use alloy_signer::SignerSync;
    use alloy_signer_local::PrivateKeySigner;
    use op_alloy_consensus::{Eip8130CallEntry, TxEip8130, sender_signature_hash};
    use op_revm::{constants::K1_VERIFIER_ADDRESS, precompiles_xlayer::NONCE_MANAGER_ADDRESS};
    use reth_optimism_chainspec::{OpChainSpec, OpChainSpecBuilder};
    use reth_optimism_evm::OpEvmConfig;
    use reth_optimism_primitives::{OpPrimitives, OpTransactionSigned};
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
    use reth_transaction_pool::{
        blobstore::InMemoryBlobStore, validate::EthTransactionValidatorBuilder,
    };
    use std::sync::Arc;

    /// Deterministic K1 signer keyed by a 1-byte seed; tests use the
    /// signer's `address()` as the AA sender so the resolver's K1
    /// strict-self-owner invariant holds at admission.
    fn signer_for(seed: u8) -> PrivateKeySigner {
        PrivateKeySigner::from_bytes(&B256::repeat_byte(seed)).expect("valid K1 key")
    }

    fn k1_sig_blob(signer: &PrivateKeySigner, hash: B256) -> Vec<u8> {
        let sig = signer.sign_hash_sync(&hash).expect("sign");
        let mut buf = Vec::with_capacity(65);
        buf.extend_from_slice(&sig.r().to_be_bytes::<32>());
        buf.extend_from_slice(&sig.s().to_be_bytes::<32>());
        buf.push(if sig.v() { 1 } else { 0 });
        buf
    }

    fn k1_explicit_auth(signer: &PrivateKeySigner, hash: B256) -> Bytes {
        let mut buf = Vec::with_capacity(85);
        buf.extend_from_slice(K1_VERIFIER_ADDRESS.as_slice());
        buf.extend_from_slice(&k1_sig_blob(signer, hash));
        Bytes::from(buf)
    }

    type TestProvider = MockEthProvider<OpPrimitives, Arc<OpChainSpec>>;
    type TestInnerValidator =
        OpTransactionValidator<TestProvider, OpPooledTransaction, OpEvmConfig>;
    type TestWrapper = OpAaTransactionValidator<TestInnerValidator, TestProvider>;

    /// Builds a self-pay AA tx whose validator path passes against a
    /// `fresh_client(sender)` provider.
    /// Chain id of the test chain spec (base mainnet).
    const TEST_CHAIN_ID: u64 = 8453;

    fn make_aa_tx(
        signer: &PrivateKeySigner,
        nonce_sequence: u64,
        expiry: u64,
    ) -> OpPooledTransaction {
        let mut tx = TxEip8130 {
            chain_id: TEST_CHAIN_ID,
            sender: Some(signer.address()),
            nonce_key: U256::ZERO,
            nonce_sequence,
            expiry,
            max_priority_fee_per_gas: 1,
            max_fee_per_gas: 2,
            gas_limit: 100_000,
            calls: vec![vec![Eip8130CallEntry {
                to: Address::repeat_byte(0xAA),
                data: Default::default(),
            }]],
            ..Default::default()
        };
        // Real K1 explicit-from sender_auth so the AA-spec layer's auth
        // dispatch lands in `Native` and admission proceeds.
        let hash = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(signer, hash);
        let signed: OpTransactionSigned = tx.seal_slow().into();
        let recovered = Recovered::new_unchecked(signed, signer.address());
        let len = recovered.encode_2718_len();
        OpPooledTransaction::new(recovered, len)
    }

    fn fresh_chain_spec() -> Arc<OpChainSpec> {
        // chain_id matches `make_aa_tx`'s chain_id; the AA-spec layer reasserts.
        Arc::new(
            OpChainSpecBuilder::base_mainnet().ecotone_activated().xlayer_v1_activated().build(),
        )
    }

    /// Builds a client seeded so `validate_eip8130_transaction` finds
    /// `aa_nonce_slot(sender, 0) == 0` and `balance > tx.cost`. The validator
    /// builder also reads the latest header at construction, so we install
    /// a genesis block.
    fn fresh_client(sender: Address, chain_spec: Arc<OpChainSpec>) -> TestProvider {
        let client =
            MockEthProvider::<OpPrimitives>::new().with_chain_spec(chain_spec).with_genesis_block();
        client.add_account(NONCE_MANAGER_ADDRESS, ExtendedAccount::new(0, U256::ZERO));
        client.add_account(sender, ExtendedAccount::new(0, U256::from(1_000_000_000_u64)));
        client
    }

    fn build_validator(client: TestProvider, chain_spec: Arc<OpChainSpec>) -> TestWrapper {
        let evm_config = OpEvmConfig::optimism(chain_spec.clone());
        let inner = EthTransactionValidatorBuilder::new(client.clone(), evm_config)
            .no_eip4844()
            .with_custom_tx_type(AA_TX_TYPE_ID)
            .build(InMemoryBlobStore::default());
        let inner = OpTransactionValidator::new(inner)
            // disable L1 data-fee gate (test fixtures don't drive an L1 info tx)
            .require_l1_data_gas_fee(false);
        OpAaTransactionValidator::new(inner, Arc::new(client))
    }

    fn fresh(sender: Address) -> TestWrapper {
        let chain_spec = fresh_chain_spec();
        let client = fresh_client(sender, chain_spec.clone());
        build_validator(client, chain_spec)
    }

    /// Non-AA path is identical to inner: wrapper does not interfere.
    #[tokio::test]
    async fn wrapper_delegates_non_aa() {
        use alloy_consensus::{Signed, TxLegacy};
        use alloy_primitives::Signature;

        let sender = Address::repeat_byte(0x11);
        let validator = fresh(sender);

        let tx = TxLegacy {
            chain_id: Some(10),
            nonce: 0,
            gas_price: 1,
            gas_limit: 21_000,
            to: alloy_primitives::TxKind::Call(Address::repeat_byte(0xBB)),
            value: U256::ZERO,
            input: Default::default(),
        };
        let sig = Signature::new(U256::from(1u64), U256::from(1u64), false);
        let signed: Signed<TxLegacy> = Signed::new_unchecked(tx, sig, B256::repeat_byte(0xCD));
        let consensus: OpTransactionSigned = signed.into();
        let recovered = Recovered::new_unchecked(consensus, sender);
        let len = recovered.encode_2718_len();
        let pooled: OpPooledTransaction = OpPooledTransaction::new(recovered, len);

        let outcome = validator.validate_transaction(TransactionOrigin::External, pooled).await;

        // legacy tx with this fixture either Valid or Invalid (signature dummy)
        // — what we care about is that `apply_aa_layer` did not transform it
        // into a TxTypeNotSupported error.
        if let TransactionValidationOutcome::Invalid(_, err) = &outcome {
            assert!(
                !err.to_string().contains("transaction type not supported"),
                "non-AA tx must not hit AA-layer rejection paths: {err}"
            );
        }
    }

    /// Inner Invalid is preserved — wrapper does not run AA layer when
    /// inner already rejected (no shadowing of error types).
    #[tokio::test]
    async fn wrapper_delegates_when_inner_invalid() {
        let sender = Address::repeat_byte(0x22);
        let validator = fresh(sender);

        // Build an AA tx but blow the gas-limit — inner's pre-state check
        // (`block_gas_limit`) will reject before our AA layer fires.
        let mut tx_inner = TxEip8130 {
            chain_id: 10,
            sender: Some(sender),
            nonce_key: U256::ZERO,
            nonce_sequence: 0,
            expiry: 0,
            max_priority_fee_per_gas: 1,
            max_fee_per_gas: 2,
            // Way over the genesis block gas limit; inner rejects via
            // `ExceedsGasLimit`.
            gas_limit: u64::MAX / 2,
            calls: vec![vec![Eip8130CallEntry {
                to: Address::repeat_byte(0xAA),
                data: Default::default(),
            }]],
            ..Default::default()
        };
        // Tiny perturbation to keep the hash distinct from the valid case.
        tx_inner.nonce_sequence = 7;
        let signed: OpTransactionSigned = tx_inner.seal_slow().into();
        let recovered = Recovered::new_unchecked(signed, sender);
        let len = recovered.encode_2718_len();
        let pooled: OpPooledTransaction = OpPooledTransaction::new(recovered, len);

        let outcome = validator.validate_transaction(TransactionOrigin::External, pooled).await;

        let err = match outcome {
            TransactionValidationOutcome::Invalid(_, e) => e,
            other => panic!("expected Invalid from inner gas-limit check, got {other:?}"),
        };
        // Should be the inner's gas-limit error, not an AA-spec error.
        let msg = err.to_string();
        assert!(
            msg.contains("gas") || msg.contains("Gas") || msg.contains("limit"),
            "expected inner gas-limit error, got: {msg}"
        );
        assert!(
            !msg.contains("EIP-8130"),
            "AA-spec layer must not run on inner-Invalid path: {msg}"
        );
    }

    /// Inner Valid + AA spec failure (expired) ⇒ wrapper rejects with the
    /// AA error. Pins that the spec layer is wired in.
    #[tokio::test]
    async fn wrapper_layers_aa_check_on_valid_inner() {
        let signer = signer_for(0x33);
        let sender = signer.address();
        let validator = fresh(sender);

        // expiry=1 with default block_ts=0 in our test wrapper means the
        // spec check sees block_ts=0 (no head block update yet), which is
        // *not* expired. So set tip_timestamp manually to drive expiry.
        validator.tip_timestamp.store(100, Ordering::Release);
        let tx = make_aa_tx(&signer, 0, /* expiry= */ 50);

        let outcome = validator.validate_transaction(TransactionOrigin::External, tx).await;

        let err = match outcome {
            TransactionValidationOutcome::Invalid(_, e) => e,
            other => panic!("expected Invalid for expired AA tx, got {other:?}"),
        };
        let msg = err.to_string();
        assert!(msg.contains("expired"), "expected AA expired error, got: {msg}");
    }

    /// Inner Valid + AA spec passes ⇒ wrapper returns Valid.
    #[tokio::test]
    async fn wrapper_passes_through_valid_aa() {
        let signer = signer_for(0x44);
        let sender = signer.address();
        let validator = fresh(sender);
        let tx = make_aa_tx(&signer, 0, /* expiry= */ 0);

        let outcome = validator.validate_transaction(TransactionOrigin::External, tx).await;

        match outcome {
            TransactionValidationOutcome::Valid { state_nonce, .. } => {
                // AA layer overrode state_nonce to the lane head value (0).
                assert_eq!(state_nonce, 0);
            }
            other => panic!("expected Valid for fresh AA tx, got {other:?}"),
        }
    }

    /// AA tx with `nonce_sequence=0` for an account whose
    /// `account.nonce > 0` admits. Pre-fix, reth's inner
    /// `validate_sender_nonce` rejected on `tx_nonce < account.nonce`,
    /// shadowing the AA-specific NONCE_MANAGER read.
    #[tokio::test]
    async fn aa_tx_with_nonce_below_account_nonce_admits() {
        let signer = signer_for(0x51);
        let sender = signer.address();
        let chain_spec = fresh_chain_spec();
        // Seed sender as a regular EOA with `account.nonce=5` (e.g. they
        // sent 5 legacy txs before flipping to AA). The 2D nonce sequence
        // for `(sender, key=0)` is still 0 because NONCE_MANAGER tracks it
        // separately.
        let client = MockEthProvider::<OpPrimitives>::new()
            .with_chain_spec(chain_spec.clone())
            .with_genesis_block();
        client.add_account(NONCE_MANAGER_ADDRESS, ExtendedAccount::new(0, U256::ZERO));
        client.add_account(sender, ExtendedAccount::new(5, U256::from(1_000_000_000_u64)));
        let validator = build_validator(client, chain_spec);

        let tx = make_aa_tx(&signer, /* nonce_sequence= */ 0, /* expiry= */ 0);
        let outcome = validator.validate_transaction(TransactionOrigin::External, tx).await;

        match outcome {
            TransactionValidationOutcome::Valid { state_nonce, .. } => {
                // The AA layer's authoritative `state_nonce` is the
                // NONCE_MANAGER read (0), NOT account.nonce (5).
                assert_eq!(state_nonce, 0);
            }
            other => panic!(
                "AA tx with nonce_sequence=0 must admit even when account.nonce>0, got {other:?}"
            ),
        }
    }

    /// AA tx with `nonce_key != 0` admits — lanes are keyed off
    /// NONCE_MANAGER storage and the lane head is independent of
    /// `account.nonce`. Pre-fix, the inner stateful path would have
    /// applied `tx.nonce() < account.nonce` against the (irrelevant)
    /// EOA nonce; the override returns a sentinel + `requires_nonce_check
    /// = false` so the predicate short-circuits.
    #[tokio::test]
    async fn aa_tx_with_high_nonce_key_admits() {
        let signer = signer_for(0x52);
        let sender = signer.address();
        let chain_spec = fresh_chain_spec();
        // Account.nonce > 0 to defend that the EOA nonce is irrelevant.
        let client = MockEthProvider::<OpPrimitives>::new()
            .with_chain_spec(chain_spec.clone())
            .with_genesis_block();
        client.add_account(NONCE_MANAGER_ADDRESS, ExtendedAccount::new(0, U256::ZERO));
        client.add_account(sender, ExtendedAccount::new(7, U256::from(1_000_000_000_u64)));
        let validator = build_validator(client, chain_spec);

        let mut tx = TxEip8130 {
            chain_id: TEST_CHAIN_ID,
            sender: Some(sender),
            nonce_key: U256::from(42_u64),
            nonce_sequence: 0,
            expiry: 0,
            max_priority_fee_per_gas: 1,
            max_fee_per_gas: 2,
            gas_limit: 100_000,
            calls: vec![vec![Eip8130CallEntry {
                to: Address::repeat_byte(0xAA),
                data: Default::default(),
            }]],
            ..Default::default()
        };
        let hash = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(&signer, hash);
        let signed: OpTransactionSigned = tx.seal_slow().into();
        let recovered = Recovered::new_unchecked(signed, sender);
        let len = recovered.encode_2718_len();
        let pooled: OpPooledTransaction = OpPooledTransaction::new(recovered, len);

        let outcome = validator.validate_transaction(TransactionOrigin::External, pooled).await;
        match outcome {
            TransactionValidationOutcome::Valid { state_nonce, .. } => {
                assert_eq!(state_nonce, 0, "AA layer reports lane head = 0");
            }
            other => panic!(
                "AA tx with nonce_key=42 must admit (lane independent of account.nonce), got {other:?}"
            ),
        }
    }

    /// Sponsored AA where the sender (envelope signer) has zero
    /// balance but the payer is well-funded must admit. Pre-fix, reth's
    /// inner `validate_sender_balance` rejected on `cost > sender.balance`
    /// (since `tx.cost()` returned the gas+value sum keyed on the signer).
    #[tokio::test]
    async fn sponsored_aa_with_empty_sender_admits_when_payer_funded() {
        use op_alloy_consensus::payer_signature_hash;
        let sender_signer = signer_for(0x53);
        let payer_signer = signer_for(0x54);
        let sender = sender_signer.address();
        let payer = payer_signer.address();

        let chain_spec = fresh_chain_spec();
        let client = MockEthProvider::<OpPrimitives>::new()
            .with_chain_spec(chain_spec.clone())
            .with_genesis_block();
        client.add_account(NONCE_MANAGER_ADDRESS, ExtendedAccount::new(0, U256::ZERO));
        // Sender has ZERO balance but `nonce=3` (regular EOA with prior
        // history). Payer has the funds.
        client.add_account(sender, ExtendedAccount::new(3, U256::ZERO));
        client.add_account(payer, ExtendedAccount::new(0, U256::from(10_000_000_000_u64)));
        let validator = build_validator(client, chain_spec);

        let mut tx = TxEip8130 {
            chain_id: TEST_CHAIN_ID,
            sender: Some(sender),
            payer: Some(payer),
            nonce_key: U256::ZERO,
            nonce_sequence: 0,
            expiry: 0,
            max_priority_fee_per_gas: 1,
            max_fee_per_gas: 2,
            gas_limit: 100_000,
            calls: vec![vec![Eip8130CallEntry {
                to: Address::repeat_byte(0xAA),
                data: Default::default(),
            }]],
            ..Default::default()
        };
        let s_hash = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(&sender_signer, s_hash);
        let p_hash = payer_signature_hash(&tx);
        tx.payer_auth = k1_explicit_auth(&payer_signer, p_hash);
        let signed: OpTransactionSigned = tx.seal_slow().into();
        let recovered = Recovered::new_unchecked(signed, sender);
        let len = recovered.encode_2718_len();
        let pooled: OpPooledTransaction = OpPooledTransaction::new(recovered, len);

        let outcome = validator.validate_transaction(TransactionOrigin::External, pooled).await;
        match outcome {
            TransactionValidationOutcome::Valid { .. } => {}
            other => {
                panic!("sponsored AA with empty-sender / funded-payer must admit, got {other:?}")
            }
        }
    }

    /// Sponsored AA where the payer balance is below `gas_limit *
    /// max_fee_per_gas` rejects with `InsufficientBalance` from the AA
    /// layer (not the inner). Pins that the AA-layer balance check is
    /// authoritative for AA txs.
    #[tokio::test]
    async fn sponsored_aa_rejects_when_payer_balance_below_total() {
        use op_alloy_consensus::payer_signature_hash;
        let sender_signer = signer_for(0x55);
        let payer_signer = signer_for(0x56);
        let sender = sender_signer.address();
        let payer = payer_signer.address();

        let chain_spec = fresh_chain_spec();
        let client = MockEthProvider::<OpPrimitives>::new()
            .with_chain_spec(chain_spec.clone())
            .with_genesis_block();
        client.add_account(NONCE_MANAGER_ADDRESS, ExtendedAccount::new(0, U256::ZERO));
        // Sender has plenty (irrelevant); payer is starved.
        client.add_account(sender, ExtendedAccount::new(0, U256::from(u128::MAX)));
        client.add_account(payer, ExtendedAccount::new(0, U256::from(1_u64)));
        let validator = build_validator(client, chain_spec);

        let mut tx = TxEip8130 {
            chain_id: TEST_CHAIN_ID,
            sender: Some(sender),
            payer: Some(payer),
            nonce_key: U256::ZERO,
            nonce_sequence: 0,
            expiry: 0,
            max_priority_fee_per_gas: 0,
            max_fee_per_gas: 1_000_000_000, // 1 gwei
            gas_limit: 100_000,
            calls: vec![vec![Eip8130CallEntry {
                to: Address::repeat_byte(0xAA),
                data: Default::default(),
            }]],
            ..Default::default()
        };
        let s_hash = sender_signature_hash(&tx);
        tx.sender_auth = k1_explicit_auth(&sender_signer, s_hash);
        let p_hash = payer_signature_hash(&tx);
        tx.payer_auth = k1_explicit_auth(&payer_signer, p_hash);
        let signed: OpTransactionSigned = tx.seal_slow().into();
        let recovered = Recovered::new_unchecked(signed, sender);
        let len = recovered.encode_2718_len();
        let pooled: OpPooledTransaction = OpPooledTransaction::new(recovered, len);

        let outcome = validator.validate_transaction(TransactionOrigin::External, pooled).await;
        let err = match outcome {
            TransactionValidationOutcome::Invalid(_, e) => e,
            other => panic!("expected Invalid for under-funded payer, got {other:?}"),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("EIP-8130 insufficient payer balance"),
            "expected AA-layer InsufficientBalance, got: {msg}"
        );
    }
}
