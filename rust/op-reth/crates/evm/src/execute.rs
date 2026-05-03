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

    pub(super) fn create_op_state_provider() -> StateProviderTest {
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

    pub(super) fn evm_config(chain_spec: Arc<OpChainSpec>) -> OpEvmConfig {
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

#[cfg(test)]
mod xlayer_tests {
    use crate::execute::tests::{create_op_state_provider, evm_config};
    use alloc::sync::Arc;
    use alloy_consensus::{Block, BlockBody, Header, Sealable};
    use alloy_eips::eip2718::{Decodable2718, Encodable2718};
    use alloy_primitives::{Address, B256, U256};
    use alloy_primitives::Bytes;
    use alloy_signer::SignerSync;
    use alloy_signer_local::PrivateKeySigner;
    use op_alloy_consensus::{Eip8130CallEntry, OpTxEnvelope, TxEip8130, sender_signature_hash};
    use op_revm::{
        constants::{K1_VERIFIER_ADDRESS, L1_BLOCK_CONTRACT},
        precompiles_xlayer::{NONCE_MANAGER_ADDRESS, TX_CONTEXT_ADDRESS},
        transaction::eip8130::phase_statuses_log_topic,
    };
    use reth_evm::execute::{BasicBlockExecutor, Executor};
    use reth_optimism_chainspec::OpChainSpecBuilder;
    use reth_optimism_primitives::{OpReceipt, OpTransactionSigned};
    use reth_primitives_traits::{Account, RecoveredBlock};
    use reth_revm::database::StateProviderDatabase;
    use std::collections::HashMap;

    #[test]
    fn single_phase_executes_post_xlayer_aa() {
        let header = Header {
            timestamp: 2,
            number: 1,
            gas_limit: 1_000_000,
            base_fee_per_gas: Some(0),
            parent_beacon_block_root: Some(B256::ZERO),
            ..Default::default()
        };

        let mut db = create_op_state_provider();
        // Deterministic signer so the K1 sig over `sender_signature_hash(tx)`
        // recovers to a known address. Required since the new validator-side
        // auth gate rejects empty / malformed `sender_auth`.
        let signer = {
            let bytes = B256::repeat_byte(0x11);
            PrivateKeySigner::from_bytes(&bytes).expect("valid private key")
        };
        let sender = signer.address();
        let target = Address::from([0x22; 20]);

        db.insert_account(
            sender,
            Account { balance: U256::MAX, ..Account::default() },
            None,
            HashMap::default(),
        );
        db.insert_account(target, Account::default(), None, HashMap::default());
        db.insert_account(NONCE_MANAGER_ADDRESS, Account::default(), None, HashMap::default());

        let chain_spec = Arc::new(OpChainSpecBuilder::base_mainnet().karst_activated().build());

        let mut tx = TxEip8130 {
            chain_id: chain_spec.chain.id(),
            from: Some(sender),
            nonce_key: U256::ZERO,
            nonce_sequence: 0,
            expiry: 0,
            max_priority_fee_per_gas: 0,
            max_fee_per_gas: 1,
            gas_limit: 100_000,
            calls: vec![vec![Eip8130CallEntry { to: target, data: Default::default() }]],
            ..Default::default()
        };
        // Sign over the canonical preimage and embed the 85-byte explicit-from
        // K1 auth blob: `[K1_VERIFIER_ADDRESS(20) || r || s || v(1)]`.
        let sig_hash = sender_signature_hash(&tx);
        let sig = signer.sign_hash_sync(&sig_hash).expect("sign");
        let mut blob = Vec::with_capacity(85);
        blob.extend_from_slice(K1_VERIFIER_ADDRESS.as_slice());
        blob.extend_from_slice(&sig.r().to_be_bytes::<32>());
        blob.extend_from_slice(&sig.s().to_be_bytes::<32>());
        blob.push(if sig.v() { 1 } else { 0 });
        tx.sender_auth = Bytes::from(blob);

        let envelope = OpTxEnvelope::Eip8130(tx.seal_slow());
        let mut encoded = Vec::new();
        envelope.encode_2718(&mut encoded);
        let tx = OpTransactionSigned::decode_2718(&mut encoded.as_slice()).expect("valid 8130 tx");

        let provider = evm_config(chain_spec);
        let mut executor = BasicBlockExecutor::new(provider, StateProviderDatabase::new(&db));

        // make sure the L1 block contract state is preloaded.
        executor.with_state_mut(|state| {
            state.load_cache_account(L1_BLOCK_CONTRACT).unwrap();
        });

        let output = executor
            .execute(&RecoveredBlock::new_unhashed(
                Block { header, body: BlockBody { transactions: vec![tx], ..Default::default() } },
                vec![sender],
            ))
            .expect("executing an EIP-8130 tx after XLayer AA activation should not fail");

        let receipts = &output.receipts;
        assert_eq!(receipts.len(), 1);

        // #TODO(xlayer-eip8130): This assertion uses the current plain Receipt placeholder.
        // Update once EIP-8130 payer/phaseStatuses receipt semantics are modeled.
        let OpReceipt::Eip8130(receipt) = &receipts[0] else { panic!("expected EIP-8130 receipt") };
        assert!(receipt.status.coerce_status());
        assert_eq!(receipt.logs.len(), 1);
        assert_eq!(receipt.logs[0].address, TX_CONTEXT_ADDRESS);
        assert_eq!(receipt.logs[0].data.topics()[0], phase_statuses_log_topic());
    }
}
