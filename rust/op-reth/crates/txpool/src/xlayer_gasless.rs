//! Gasless transaction support for the OP transaction pool.
//!
//! When gasless transactions are enabled, zero-priced transactions (legacy `gas_price == 0`, or
//! EIP-1559 `max_fee_per_gas == 0 && max_priority_fee_per_gas == 0`) are accepted into the pool
//! (see the pool's `minimal_protocol_basefee` override in the node builder) and assigned a *mock*
//! gas price for ordering, so they compete with normal transactions by (gas price, then enqueue
//! time). This module provides:
//!
//! - [`GaslessMockPrice`]: the shared mock gas price.
//! - [`XLayerGaslessOrdering`]: a [`TransactionOrdering`] that assigns the mock price to gasless
//!   txs and otherwise behaves exactly like
//!   [`CoinbaseTipOrdering`](reth_transaction_pool::CoinbaseTipOrdering).
//! - [`maintain_gasless_mock_price`]: a maintenance task that, on every new canonical block,
//!   records a gas-price metric and recomputes the mock price as a configured percentile of the
//!   block's transaction gas prices.
//!
//! The (gas price, enqueue time) total order is completed by the pending sub-pool's existing
//! `submission_id` tiebreak (priority first, earlier submission second) — see
//! `reth_transaction_pool`'s `PendingTransaction` ordering — so no change to that logic is needed.

use alloy_consensus::{BlockHeader, Transaction};
use futures_util::{Stream, StreamExt};
use metrics::Histogram;
use reth_chain_state::CanonStateNotification;
use reth_metrics::Metrics;
use reth_primitives_traits::{BlockBody, NodePrimitives};
use reth_transaction_pool::{PoolTransaction, Priority, TransactionOrdering};
use std::{
    fmt,
    marker::PhantomData,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering as AtomicOrdering},
    },
};

/// Default mock gas price (wei) assigned to gasless (zero-priced) transactions before the first
/// canonical block establishes a real percentile: 0.02 GWEI (`20_000_000` wei).
///
/// Acts as the startup floor so gasless txs never order at price 0 — a mock price of 0 would order
/// every gasless tx at priority 0, and the percentile sample (which excludes zero-priced txs) could
/// never lift it back up. After the first block with paid txs, [`maintain_gasless_mock_price`]
/// tracks the last non-zero percentile and only overrides it when a newer one is available, so the
/// value behaves as a persisted "last non-zero gas price".
pub const GASLESS_DEFAULT_MOCK_PRICE_WEI: u64 = 20_000_000;

/// Shared mock gas price (in wei) assigned to gasless (zero-priced) transactions for pool ordering.
///
/// Updated on every new canonical block by [`maintain_gasless_mock_price`] and read by
/// [`XLayerGaslessOrdering::priority`]. Cheap to clone (`Arc`-backed).
#[derive(Clone, Debug, Default)]
pub struct GaslessMockPrice(Arc<AtomicU64>);

impl GaslessMockPrice {
    /// Creates a new shared mock price initialized to `initial` wei.
    #[inline]
    pub fn new(initial: u64) -> Self {
        Self(Arc::new(AtomicU64::new(initial)))
    }

    /// Returns the current mock gas price (wei).
    #[inline]
    pub fn get(&self) -> u64 {
        self.0.load(AtomicOrdering::Relaxed)
    }

    /// Sets the mock gas price (wei).
    #[inline]
    pub fn set(&self, price: u64) {
        self.0.store(price, AtomicOrdering::Relaxed);
    }
}

/// Returns the index into an ascending-sorted slice of length `len` for `percentile` in `[0.0,
/// 1.0]`.
fn percentile_index(len: usize, percentile: f64) -> usize {
    if len == 0 {
        return 0;
    }
    let p = percentile.clamp(0.0, 1.0);
    ((p * len as f64).floor() as usize).min(len - 1)
}

/// Returns the `percentile` (in `[0.0, 1.0]`) gas price of `prices`, or `None` if empty.
///
/// Sorts ascending and picks the element at `floor(percentile * len)`, clamped to the last index.
/// For example, with prices `[0, 1, .., 9]`: `0.1` → `1`, `0.9` → `9`.
pub fn percentile_gas_price(mut prices: Vec<u128>, percentile: f64) -> Option<u128> {
    if prices.is_empty() {
        return None;
    }
    prices.sort_unstable();
    Some(prices[percentile_index(prices.len(), percentile)])
}

/// Returns the percentile across `prices` with zero-priced entries excluded.
///
/// Gasless txs land in canonical blocks with `effective_gas_price == 0`. If they were left in the
/// sample, even a small share would drag `mock_price` toward 0 at low percentiles, creating a
/// positive-feedback trap (mock_price=0 → all gasless ordered at priority 0 → next block samples 0
/// again). Returns `None` when no paid tx remains so the caller keeps the previous mock price.
fn percentile_paid_gas_price(prices: Vec<u128>, percentile: f64) -> Option<u128> {
    let paid: Vec<u128> = prices.into_iter().filter(|p| *p > 0).collect();
    percentile_gas_price(paid, percentile)
}

/// Transaction ordering that assigns a configurable mock gas price to zero-priced ("gasless")
/// transactions.
///
/// Gasless txs are ordered by the shared [`GaslessMockPrice`]; every other transaction is ordered
/// exactly like [`CoinbaseTipOrdering`](reth_transaction_pool::CoinbaseTipOrdering)
/// (`effective_tip_per_gas`), so the rest of the pool ordering is unchanged.
pub struct XLayerGaslessOrdering<T> {
    mock_price: GaslessMockPrice,
    // `T` is bound to `TransactionOrdering::Transaction` in the impl below; no field holds it.
    _pd: PhantomData<T>,
}

impl<T> XLayerGaslessOrdering<T> {
    /// Creates a new ordering backed by the shared `mock_price`.
    pub fn new(mock_price: GaslessMockPrice) -> Self {
        Self { mock_price, _pd: PhantomData }
    }
}

impl<T> Clone for XLayerGaslessOrdering<T> {
    fn clone(&self) -> Self {
        Self { mock_price: self.mock_price.clone(), _pd: PhantomData }
    }
}

impl<T> Default for XLayerGaslessOrdering<T> {
    fn default() -> Self {
        Self::new(GaslessMockPrice::default())
    }
}

impl<T> fmt::Debug for XLayerGaslessOrdering<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("XLayerGaslessOrdering").field("mock_price", &self.mock_price).finish()
    }
}

impl<T> TransactionOrdering for XLayerGaslessOrdering<T>
where
    T: PoolTransaction + 'static,
{
    type PriorityValue = u128;
    type Transaction = T;

    #[inline]
    fn priority(&self, transaction: &Self::Transaction, base_fee: u64) -> Priority<u128> {
        // A zero-priced (gasless) tx is assigned the configured mock price so it competes with
        // normal txs by gas price; the (gas price, enqueue time) total order is completed by the
        // pending pool's `submission_id` tiebreak. `max_fee_per_gas() == 0` covers both a legacy
        // `gas_price == 0` and a 1559 `max_fee == 0 && max_priority == 0`.
        if transaction.max_fee_per_gas() == 0 {
            return Priority::Value(u128::from(self.mock_price.get()));
        }
        // Identical to `CoinbaseTipOrdering`.
        transaction.effective_tip_per_gas(base_fee).into()
    }
}

/// Metrics recorded for each new canonical block's transactions.
#[derive(Metrics)]
#[metrics(scope = "optimism_transaction_pool.gasless")]
struct GaslessBlockMetrics {
    /// Effective gas price (wei) of each transaction in the latest canonical block.
    block_tx_gas_price: Histogram,
}

/// Maintenance task: on each new canonical block, record a gas-price metric for every transaction
/// and update `mock_price` to the configured `percentile` of the block's paid transaction gas
/// prices (ascending). Zero-priced (gasless) txs are excluded from the percentile sample so that
/// blocks carrying gasless traffic don't drag `mock_price` toward 0 (see
/// [`percentile_paid_gas_price`]). Empty blocks, or blocks with only gasless txs, leave the
/// previous mock price unchanged.
pub async fn maintain_gasless_mock_price<N, St>(
    mock_price: GaslessMockPrice,
    percentile: f64,
    mut events: St,
) where
    N: NodePrimitives,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
{
    let metrics = GaslessBlockMetrics::default();
    loop {
        let Some(event) = events.next().await else { break };
        if let CanonStateNotification::Commit { new } = event {
            let block = new.tip().sealed_block();
            let base_fee = block.header().base_fee_per_gas();
            let mut prices: Vec<u128> = Vec::new();
            for tx in block.body().transactions() {
                let price = tx.effective_gas_price(base_fee);
                metrics.block_tx_gas_price.record(price as f64);
                prices.push(price);
            }
            if let Some(price) = percentile_paid_gas_price(prices, percentile) {
                // Clamp into u64 (wei mock price); real gas prices never approach u64::MAX.
                mock_price.set(price.min(u128::from(u64::MAX)) as u64);
            }
        }
    }
}

#[cfg(test)]
mod xlayer_test {
    use super::*;

    #[test]
    fn percentile_examples() {
        let prices: Vec<u128> = (0..10).collect(); // [0, 1, .., 9], len 10
        assert_eq!(percentile_gas_price(prices.clone(), 0.1), Some(1)); // idx floor(1.0) = 1
        assert_eq!(percentile_gas_price(prices.clone(), 0.9), Some(9)); // idx floor(9.0) = 9
        assert_eq!(percentile_gas_price(prices.clone(), 0.0), Some(0));
        assert_eq!(percentile_gas_price(prices, 1.0), Some(9)); // clamped to last index
        assert_eq!(percentile_gas_price(vec![], 0.5), None);
    }

    #[test]
    fn percentile_unsorted_input() {
        assert_eq!(percentile_gas_price(vec![9, 1, 5, 3, 7], 0.5), Some(5));
    }

    #[test]
    fn percentile_clamps_out_of_range() {
        assert_eq!(percentile_gas_price(vec![10, 20, 30], 2.0), Some(30));
        assert_eq!(percentile_gas_price(vec![10, 20, 30], -1.0), Some(10));
    }

    #[test]
    fn percentile_paid_excludes_zero_priced_gasless() {
        // Mixed block: 4 gasless (0) + 3 paid (10, 20, 30). With the raw sample a low percentile
        // would hit 0; after excluding gasless the sample is [10, 20, 30] and the percentile
        // reflects the real fee market.
        let mixed = vec![0u128, 0, 0, 0, 10, 20, 30];
        assert_eq!(percentile_paid_gas_price(mixed.clone(), 0.0), Some(10));
        assert_eq!(percentile_paid_gas_price(mixed.clone(), 0.5), Some(20));
        assert_eq!(percentile_paid_gas_price(mixed, 1.0), Some(30));

        // All-gasless block: nothing to sample; `None` tells the caller to keep the prior price.
        assert_eq!(percentile_paid_gas_price(vec![0, 0, 0], 0.5), None);

        // No gasless: identical to `percentile_gas_price`.
        assert_eq!(percentile_paid_gas_price(vec![10, 20, 30], 0.5), Some(20));

        // Sanity-check the regression that motivated this filter: without the filter, a low
        // percentile on a mostly-gasless block would return 0.
        assert_eq!(percentile_gas_price(vec![0, 0, 0, 0, 10, 20, 30], 0.1), Some(0));
    }

    #[test]
    fn mock_price_get_set() {
        let p = GaslessMockPrice::new(7);
        assert_eq!(p.get(), 7);
        let q = p.clone();
        q.set(42);
        assert_eq!(p.get(), 42); // shared
    }

    #[test]
    fn ordering_assigns_mock_price_to_gasless_tx() {
        use reth_transaction_pool::test_utils::MockTransaction;

        let mock = GaslessMockPrice::new(500);
        let ordering = XLayerGaslessOrdering::<MockTransaction>::new(mock.clone());

        // Zero-priced (gasless) tx -> the configured mock price.
        let gasless_tx = MockTransaction::legacy().with_gas_price(0);
        assert_eq!(ordering.priority(&gasless_tx, 0), Priority::Value(500));

        // Normal tx -> effective tip, identical to `CoinbaseTipOrdering` (base_fee 0 => tip ==
        // price).
        let normal_tx = MockTransaction::legacy().with_gas_price(100);
        assert_eq!(ordering.priority(&normal_tx, 0), Priority::Value(100));

        // Mock-price updates are reflected immediately (shared state).
        mock.set(777);
        assert_eq!(ordering.priority(&gasless_tx, 0), Priority::Value(777));
    }

    /// Minimal contract bytecode returning ABI `(true, 0xffffff)` for any call — approves with a
    /// gas allowance far above the test tx's gas limit. Layout: `mem[0..32]=1` (allowed),
    /// `mem[32..64]=0xffffff` (gasLimit), `return mem[0..64]`.
    const ALLOW_HIGH_GAS_BYTECODE: [u8; 17] = [
        0x60, 0x01, 0x60, 0x00, 0x52, 0x62, 0xff, 0xff, 0xff, 0x60, 0x20, 0x52, 0x60, 0x40, 0x60,
        0x00, 0xf3,
    ];
    /// Minimal contract bytecode returning ABI `(false, 0)` for any call (64 zero bytes) — denies.
    const DENY_BYTECODE: [u8; 5] = [0x60, 0x40, 0x60, 0x00, 0xf3];
    /// Minimal contract bytecode returning ABI `(true, 1)` for any call — approves but with a gas
    /// allowance (1) below any real tx gas limit. Layout: `mem[0..32]=1`, `mem[32..64]=1`,
    /// `return mem[0..64]`.
    const ALLOW_LOW_GAS_BYTECODE: [u8; 15] =
        [0x60, 0x01, 0x60, 0x00, 0x52, 0x60, 0x01, 0x60, 0x20, 0x52, 0x60, 0x40, 0x60, 0x00, 0xf3];

    /// Builds an [`OpTransactionValidator`] (gasless enabled) over a `MockEthProvider` on the
    /// XLayer devnet chain (id 195, so the validator derives `XLAYER_DEVNET_GASLESS_CONTRACT`),
    /// deploys the given gasless-whitelist stub bytecode there, then validates a zero-priced
    /// (`max_fee_per_gas == 0`) EIP-1559 transfer and returns the outcome.
    async fn validate_zero_priced_tx(
        stub_bytecode: &[u8],
    ) -> reth_transaction_pool::TransactionValidationOutcome<crate::OpPooledTransaction> {
        use crate::{OpPooledTransaction, OpTransactionValidator};
        use alloy_consensus::{SignableTransaction, TxEip1559};
        use alloy_eips::Encodable2718;
        use alloy_primitives::{Address, Bytes, Signature, TxKind, U256};
        use reth_chainspec::Chain;
        use reth_optimism_chainspec::OpChainSpecBuilder;
        use reth_optimism_evm::{OpEvmConfig, XLAYER_DEVNET_GASLESS_CONTRACT};
        use reth_optimism_primitives::{OpPrimitives, OpTransactionSigned};
        use reth_primitives_traits::Recovered;
        use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
        use reth_transaction_pool::{
            TransactionOrigin, blobstore::InMemoryBlobStore,
            validate::EthTransactionValidatorBuilder,
        };
        use std::sync::Arc;

        let mut spec = OpChainSpecBuilder::base_mainnet().canyon_activated().build();
        // XLayer devnet chain id so `xlayer_gasless_contract(195) ==
        // XLAYER_DEVNET_GASLESS_CONTRACT`.
        spec.inner.chain = Chain::from_id(195);
        let chain_spec = Arc::new(spec);

        let client = MockEthProvider::<OpPrimitives>::new()
            .with_chain_spec(chain_spec.clone())
            .with_genesis_block();
        // Deploy the gasless whitelist stub at the chain's gasless predeploy address.
        client.add_account(
            XLAYER_DEVNET_GASLESS_CONTRACT,
            ExtendedAccount::new(0, U256::ZERO)
                .with_bytecode(Bytes::copy_from_slice(stub_bytecode)),
        );

        // Zero-priced EIP-1559 transfer.
        let tx = TxEip1559 {
            chain_id: 195,
            nonce: 0,
            gas_limit: 21_000,
            max_fee_per_gas: 0,
            max_priority_fee_per_gas: 0,
            to: TxKind::Call(Address::ZERO),
            value: U256::ZERO,
            ..Default::default()
        };
        let signed: OpTransactionSigned = tx.into_signed(Signature::test_signature()).into();
        // The pooled tx is pre-recovered with a fixed sender (the validator trusts the recovered
        // signer; it does not re-recover). Fund it so the balance check passes — gasless still
        // requires the sender to afford `value`.
        let sender = Address::from([0x11u8; 20]);
        client.add_account(sender, ExtendedAccount::new(0, U256::MAX));

        let signed_recovered = Recovered::new_unchecked(signed, sender);
        let len = signed_recovered.encode_2718_len();
        let pooled = OpPooledTransaction::new(signed_recovered, len);

        let evm_config = OpEvmConfig::optimism(chain_spec);
        let inner = EthTransactionValidatorBuilder::new(client, evm_config)
            .build(InMemoryBlobStore::default());
        let validator = OpTransactionValidator::new(inner)
            // Skip the L1-data-fee balance check; not relevant to the gasless gate under test.
            .require_l1_data_gas_fee(false)
            .with_gasless(true);

        validator.validate_one(TransactionOrigin::External, pooled).await
    }

    /// A zero-priced tx is admitted when the on-chain gasless contract approves it (allowed, with a
    /// gas allowance the tx does not exceed).
    #[tokio::test]
    async fn gasless_validator_accepts_whitelisted_zero_price() {
        let outcome = validate_zero_priced_tx(&ALLOW_HIGH_GAS_BYTECODE).await;
        assert!(
            matches!(outcome, reth_transaction_pool::TransactionValidationOutcome::Valid { .. }),
            "whitelisted zero-priced tx must be accepted, got {outcome:?}",
        );
    }

    /// A zero-priced tx is rejected when the gasless contract denies it (not whitelisted).
    #[tokio::test]
    async fn gasless_validator_rejects_unwhitelisted_zero_price() {
        let outcome = validate_zero_priced_tx(&DENY_BYTECODE).await;
        assert!(
            matches!(outcome, reth_transaction_pool::TransactionValidationOutcome::Invalid(..)),
            "non-whitelisted zero-priced tx must be rejected, got {outcome:?}",
        );
    }

    /// A whitelisted zero-priced tx is rejected when its gas limit exceeds the contract's per-tx
    /// gas allowance, surfaced as `MaxTxGasLimitExceeded`.
    #[tokio::test]
    async fn gasless_validator_rejects_over_gas_limit_zero_price() {
        use reth_transaction_pool::error::InvalidPoolTransactionError;
        let outcome = validate_zero_priced_tx(&ALLOW_LOW_GAS_BYTECODE).await;
        assert!(
            matches!(
                outcome,
                reth_transaction_pool::TransactionValidationOutcome::Invalid(
                    _,
                    InvalidPoolTransactionError::MaxTxGasLimitExceeded(..)
                )
            ),
            "zero-priced tx over the gas allowance must be rejected with MaxTxGasLimitExceeded, got {outcome:?}",
        );
    }
}
