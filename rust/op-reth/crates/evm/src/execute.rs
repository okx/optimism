//! Optimism block execution strategy.

/// Helper type with backwards compatible methods to obtain executor providers.
pub type OpExecutorProvider = crate::OpEvmConfig;

#[cfg(test)]
mod tests {
    use crate::{OpEvmConfig, OpRethReceiptBuilder};
    use alloc::sync::Arc;
    use alloy_consensus::{Block, BlockBody, Header, SignableTransaction, TxEip1559};
    use alloy_primitives::{Address, Signature, StorageKey, StorageValue, U256, b256};
    use op_alloy_consensus::TxDeposit;
    use op_revm::constants::L1_BLOCK_CONTRACT;
    use reth_chainspec::MIN_TRANSACTION_GAS;
    use reth_evm::execute::{BasicBlockExecutor, Executor};
    use reth_optimism_chainspec::{OpChainSpec, OpChainSpecBuilder};
    use reth_optimism_primitives::{OpReceipt, OpTransactionSigned};
    use reth_primitives_traits::{Account, RecoveredBlock};
    use reth_revm::{database::StateProviderDatabase, test_utils::StateProviderTest};
    use std::{collections::HashMap, str::FromStr};

    pub(crate) fn create_op_state_provider() -> StateProviderTest {
        let mut db = StateProviderTest::default();

        let l1_block_contract_account =
            Account { balance: U256::ZERO, bytecode_hash: None, nonce: 1 };

        let mut l1_block_storage = HashMap::default();
        // base fee
        l1_block_storage.insert(StorageKey::with_last_byte(1), StorageValue::from(1000000000));
        // l1 fee overhead
        l1_block_storage.insert(StorageKey::with_last_byte(5), StorageValue::from(188));
        // l1 fee scalar
        l1_block_storage.insert(StorageKey::with_last_byte(6), StorageValue::from(684000));
        // l1 free scalars post ecotone
        l1_block_storage.insert(
            StorageKey::with_last_byte(3),
            StorageValue::from_str(
                "0x0000000000000000000000000000000000001db0000d27300000000000000005",
            )
            .unwrap(),
        );

        db.insert_account(L1_BLOCK_CONTRACT, l1_block_contract_account, None, l1_block_storage);

        db
    }

    pub(crate) fn evm_config(chain_spec: Arc<OpChainSpec>) -> OpEvmConfig {
        OpEvmConfig::new(chain_spec, OpRethReceiptBuilder::default())
    }

    #[test]
    fn op_deposit_fields_pre_canyon() {
        let header = Header {
            timestamp: 1,
            number: 1,
            gas_limit: 1_000_000,
            gas_used: 42_000,
            receipts_root: b256!(
                "0x83465d1e7d01578c0d609be33570f91242f013e9e295b0879905346abbd63731"
            ),
            ..Default::default()
        };

        let mut db = create_op_state_provider();

        let addr = Address::ZERO;
        let account = Account { balance: U256::MAX, ..Account::default() };
        db.insert_account(addr, account, None, HashMap::default());

        let chain_spec = Arc::new(OpChainSpecBuilder::base_mainnet().regolith_activated().build());

        let tx: OpTransactionSigned = TxEip1559 {
            chain_id: chain_spec.chain.id(),
            nonce: 0,
            gas_limit: MIN_TRANSACTION_GAS,
            to: addr.into(),
            ..Default::default()
        }
        .into_signed(Signature::test_signature())
        .into();

        let tx_deposit: OpTransactionSigned = TxDeposit {
            from: addr,
            to: addr.into(),
            gas_limit: MIN_TRANSACTION_GAS,
            ..Default::default()
        }
        .into();

        let provider = evm_config(chain_spec);
        let mut executor = BasicBlockExecutor::new(provider, StateProviderDatabase::new(&db));

        // make sure the L1 block contract state is preloaded.
        executor.with_state_mut(|state| {
            state.load_cache_account(L1_BLOCK_CONTRACT).unwrap();
        });

        // Attempt to execute a block with one deposit and one non-deposit transaction
        let output = executor
            .execute(&RecoveredBlock::new_unhashed(
                Block {
                    header,
                    body: BlockBody { transactions: vec![tx, tx_deposit], ..Default::default() },
                },
                vec![addr, addr],
            ))
            .unwrap();

        let receipts = &output.receipts;
        let tx_receipt = &receipts[0];
        let deposit_receipt = &receipts[1];

        assert!(!matches!(tx_receipt, OpReceipt::Deposit(_)));
        // deposit_nonce is present only in deposit transactions
        let OpReceipt::Deposit(deposit_receipt) = deposit_receipt else {
            panic!("expected deposit")
        };
        assert!(deposit_receipt.deposit_nonce.is_some());
        // deposit_receipt_version is not present in pre canyon transactions
        assert!(deposit_receipt.deposit_receipt_version.is_none());
    }

    #[test]
    fn op_deposit_fields_post_canyon() {
        // ensure_create2_deployer will fail if timestamp is set to less than 2
        let header = Header {
            timestamp: 2,
            number: 1,
            gas_limit: 1_000_000,
            gas_used: 42_000,
            receipts_root: b256!(
                "0xfffc85c4004fd03c7bfbe5491fae98a7473126c099ac11e8286fd0013f15f908"
            ),
            ..Default::default()
        };

        let mut db = create_op_state_provider();
        let addr = Address::ZERO;
        let account = Account { balance: U256::MAX, ..Account::default() };

        db.insert_account(addr, account, None, HashMap::default());

        let chain_spec = Arc::new(OpChainSpecBuilder::base_mainnet().canyon_activated().build());

        let tx: OpTransactionSigned = TxEip1559 {
            chain_id: chain_spec.chain.id(),
            nonce: 0,
            gas_limit: MIN_TRANSACTION_GAS,
            to: addr.into(),
            ..Default::default()
        }
        .into_signed(Signature::test_signature())
        .into();

        let tx_deposit: OpTransactionSigned = TxDeposit {
            from: addr,
            to: addr.into(),
            gas_limit: MIN_TRANSACTION_GAS,
            ..Default::default()
        }
        .into();

        let provider = evm_config(chain_spec);
        let mut executor = BasicBlockExecutor::new(provider, StateProviderDatabase::new(&db));

        // make sure the L1 block contract state is preloaded.
        executor.with_state_mut(|state| {
            state.load_cache_account(L1_BLOCK_CONTRACT).unwrap();
        });

        // attempt to execute an empty block with parent beacon block root, this should not fail
        let output = executor
            .execute(&RecoveredBlock::new_unhashed(
                Block {
                    header,
                    body: BlockBody { transactions: vec![tx, tx_deposit], ..Default::default() },
                },
                vec![addr, addr],
            ))
            .expect("Executing a block while canyon is active should not fail");

        let receipts = &output.receipts;
        let tx_receipt = &receipts[0];
        let deposit_receipt = &receipts[1];

        // deposit_receipt_version is set to 1 for post canyon deposit transactions
        assert!(!matches!(tx_receipt, OpReceipt::Deposit(_)));
        let OpReceipt::Deposit(deposit_receipt) = deposit_receipt else {
            panic!("expected deposit")
        };
        assert_eq!(deposit_receipt.deposit_receipt_version, Some(1));

        // deposit_nonce is present only in deposit transactions
        assert!(deposit_receipt.deposit_nonce.is_some());
    }
}

// ---------------------------------------------------------------------------
// XLayer gasless tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod xlayer_test {
    use super::tests::{create_op_state_provider, evm_config};
    use crate::GaslessContract;
    use alloc::sync::Arc;
    use alloy_consensus::{Block, BlockBody, Header, SignableTransaction, TxEip1559};
    use alloy_primitives::{Address, Bytes, Signature, U256};
    use op_revm::constants::L1_BLOCK_CONTRACT;
    use reth_chainspec::{ForkCondition, MIN_TRANSACTION_GAS};
    use reth_evm::execute::{BasicBlockExecutor, Executor};
    use reth_optimism_chainspec::{OpChainSpecBuilder, OpHardfork};
    use reth_optimism_primitives::OpTransactionSigned;
    use reth_primitives_traits::{Account, RecoveredBlock};
    use reth_revm::database::StateProviderDatabase;
    use std::collections::HashMap;

    /// Minimal contract bytecode that returns ABI `true` (a 32-byte word == 1) for any call:
    /// `PUSH1 1, PUSH1 0, MSTORE, PUSH1 32, PUSH1 0, RETURN`.
    const ALWAYS_TRUE_BYTECODE: [u8; 10] =
        [0x60, 0x01, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xf3];

    /// Minimal contract bytecode that returns ABI `false` (32 zero bytes) for any call:
    /// `PUSH1 0, PUSH1 0, MSTORE, PUSH1 32, PUSH1 0, RETURN`.
    const ALWAYS_FALSE_BYTECODE: [u8; 10] =
        [0x60, 0x00, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xf3];

    /// A zero-priced tx (`max_fee_per_gas == 0`) executes gaslessly once `XLayerV1` is active
    /// **and**
    /// the configured on-chain gasless contract whitelists it (`isGaslessEnabled()` and
    /// `isWhitelisted(to, input)` both return true): the executor marks it `is_gasless`, the
    /// base-fee check is disabled, and it produces a receipt even though the block base fee is
    /// non-zero.
    #[test]
    fn gasless_zero_price_tx_whitelisted_succeeds() {
        let header = Header {
            timestamp: 2,
            number: 1,
            gas_limit: 1_000_000,
            base_fee_per_gas: Some(1_000_000_000),
            ..Default::default()
        };

        let mut db = create_op_state_provider();
        let addr = Address::ZERO;
        db.insert_account(
            addr,
            Account { balance: U256::MAX, ..Account::default() },
            None,
            HashMap::default(),
        );

        // Deploy a gasless contract that approves everything (`isGaslessEnabled()` and
        // `isWhitelisted(..)` both return true).
        let gasless_addr = Address::from([0x42; 20]);
        db.insert_account(
            gasless_addr,
            Account { nonce: 1, ..Account::default() },
            Some(Bytes::from_static(&ALWAYS_TRUE_BYTECODE)),
            HashMap::default(),
        );

        // XLayerV1 active from genesis.
        let chain_spec = Arc::new(
            OpChainSpecBuilder::base_mainnet()
                .canyon_activated()
                .with_fork(OpHardfork::XLayerV1, ForkCondition::Timestamp(0))
                .build(),
        );

        // Default `TxEip1559` has `max_fee_per_gas == 0 && max_priority_fee_per_gas == 0`.
        let tx: OpTransactionSigned = TxEip1559 {
            chain_id: chain_spec.chain.id(),
            nonce: 0,
            gas_limit: MIN_TRANSACTION_GAS,
            to: addr.into(),
            ..Default::default()
        }
        .into_signed(Signature::test_signature())
        .into();

        let provider =
            evm_config(chain_spec).with_gasless_contract(Some(GaslessContract::new(gasless_addr)));
        let mut executor = BasicBlockExecutor::new(provider, StateProviderDatabase::new(&db));
        executor.with_state_mut(|state| {
            state.load_cache_account(L1_BLOCK_CONTRACT).unwrap();
        });

        let output = executor
            .execute(&RecoveredBlock::new_unhashed(
                Block { header, body: BlockBody { transactions: vec![tx], ..Default::default() } },
                vec![addr],
            ))
            .expect("zero-priced whitelisted tx should execute gaslessly when XLayerV1 is active");

        assert_eq!(output.receipts.len(), 1);
    }

    /// With `XLayerV1` active and a zero-priced tx, but the gasless contract denies it
    /// (`isGaslessEnabled()` / `isWhitelisted(..)` return false — e.g. the `to` is not
    /// whitelisted), the tx is not gasless, so base-fee validation rejects it when the block base
    /// fee is non-zero and block execution fails.
    #[test]
    fn gasless_zero_price_tx_not_whitelisted_rejected() {
        let header = Header {
            timestamp: 2,
            number: 1,
            gas_limit: 1_000_000,
            base_fee_per_gas: Some(1_000_000_000),
            ..Default::default()
        };

        let mut db = create_op_state_provider();
        let addr = Address::ZERO;
        db.insert_account(
            addr,
            Account { balance: U256::MAX, ..Account::default() },
            None,
            HashMap::default(),
        );

        // Gasless contract that denies everything.
        let gasless_addr = Address::from([0x42; 20]);
        db.insert_account(
            gasless_addr,
            Account { nonce: 1, ..Account::default() },
            Some(Bytes::from_static(&ALWAYS_FALSE_BYTECODE)),
            HashMap::default(),
        );

        let chain_spec = Arc::new(
            OpChainSpecBuilder::base_mainnet()
                .canyon_activated()
                .with_fork(OpHardfork::XLayerV1, ForkCondition::Timestamp(0))
                .build(),
        );

        let tx: OpTransactionSigned = TxEip1559 {
            chain_id: chain_spec.chain.id(),
            nonce: 0,
            gas_limit: MIN_TRANSACTION_GAS,
            to: addr.into(),
            ..Default::default()
        }
        .into_signed(Signature::test_signature())
        .into();

        let provider =
            evm_config(chain_spec).with_gasless_contract(Some(GaslessContract::new(gasless_addr)));
        let mut executor = BasicBlockExecutor::new(provider, StateProviderDatabase::new(&db));
        executor.with_state_mut(|state| {
            state.load_cache_account(L1_BLOCK_CONTRACT).unwrap();
        });

        let result = executor.execute(&RecoveredBlock::new_unhashed(
            Block { header, body: BlockBody { transactions: vec![tx], ..Default::default() } },
            vec![addr],
        ));

        assert!(result.is_err(), "non-whitelisted zero-priced tx must be rejected");
    }

    /// Without `XLayerV1`, a zero-priced tx is rejected by base-fee validation when the block base
    /// fee is non-zero (it is not gasless), so block execution fails.
    #[test]
    fn gasless_zero_price_tx_no_xlayer_v1_rejected() {
        let header = Header {
            timestamp: 2,
            number: 1,
            gas_limit: 1_000_000,
            base_fee_per_gas: Some(1_000_000_000),
            ..Default::default()
        };

        let mut db = create_op_state_provider();
        let addr = Address::ZERO;
        db.insert_account(
            addr,
            Account { balance: U256::MAX, ..Account::default() },
            None,
            HashMap::default(),
        );

        // No XLayerV1 -> zero-priced txs are not gasless.
        let chain_spec = Arc::new(OpChainSpecBuilder::base_mainnet().canyon_activated().build());

        let tx: OpTransactionSigned = TxEip1559 {
            chain_id: chain_spec.chain.id(),
            nonce: 0,
            gas_limit: MIN_TRANSACTION_GAS,
            to: addr.into(),
            ..Default::default()
        }
        .into_signed(Signature::test_signature())
        .into();

        let provider = evm_config(chain_spec);
        let mut executor = BasicBlockExecutor::new(provider, StateProviderDatabase::new(&db));
        executor.with_state_mut(|state| {
            state.load_cache_account(L1_BLOCK_CONTRACT).unwrap();
        });

        let result = executor.execute(&RecoveredBlock::new_unhashed(
            Block { header, body: BlockBody { transactions: vec![tx], ..Default::default() } },
            vec![addr],
        ));

        assert!(result.is_err(), "zero-priced tx must be rejected without XLayerV1");
    }
}
