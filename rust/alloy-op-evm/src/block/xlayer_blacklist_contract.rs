//! On-chain deposit blacklist contract handle and deposit classification.

use alloy_eips::eip4788::SYSTEM_ADDRESS;
use alloy_evm::Evm;
use alloy_primitives::{Address, B256, Bytes, address};
use op_revm::transaction::{OpTxTr, deposit::DEPOSIT_TRANSACTION_TYPE};
use revm::context_interface::{
    Transaction,
    result::{ExecutionResult, Output},
};

use crate::OpTx;

/// X Layer devnet chain ID.
pub const XLAYER_DEVNET_CHAIN_ID: u64 = 195;
/// X Layer testnet chain ID.
pub const XLAYER_TESTNET_CHAIN_ID: u64 = 1952;
/// X Layer mainnet chain ID.
pub const XLAYER_MAINNET_CHAIN_ID: u64 = 196;

/// Devnet blacklist proxy.
pub const XLAYER_DEVNET_BLACKLIST_CONTRACT: Option<Address> =
    Some(address!("0xb1ac000000000000000000000000000000000001"));
/// Testnet blacklist proxy.
pub const XLAYER_TESTNET_BLACKLIST_CONTRACT: Option<Address> =
    Some(address!("0x59055c0ef0be92018b33f877a3eB816355791727"));
/// Mainnet blacklist proxy.
pub const XLAYER_MAINNET_BLACKLIST_CONTRACT: Option<Address> =
    Some(address!("0x59055c0ef0be92018b33f877a3eB816355791727"));

/// L1 attributes depositor account. The system flag is false after Regolith, so the caller must
/// also be checked explicitly.
pub const L1_ATTRIBUTES_DEPOSITOR: Address = address!("DeaDDEaDDeAdDeAdDEAdDEaddeAddEAdDEAd0001");

/// `keccak256("isBlacklisted(bytes32)")[..4]`.
const IS_BLACKLISTED_SELECTOR: [u8; 4] = [0x6b, 0x62, 0x3b, 0xbe];

/// Returns the configured blacklist proxy for `chain_id`.
///
/// Unknown chains never inherit a supported network's address.
#[inline]
pub const fn xlayer_blacklist_contract(chain_id: u64) -> Option<Address> {
    match chain_id {
        XLAYER_DEVNET_CHAIN_ID => XLAYER_DEVNET_BLACKLIST_CONTRACT,
        XLAYER_TESTNET_CHAIN_ID => XLAYER_TESTNET_BLACKLIST_CONTRACT,
        XLAYER_MAINNET_CHAIN_ID => XLAYER_MAINNET_BLACKLIST_CONTRACT,
        _ => None,
    }
}

/// Handle to a deployed `TxBlacklist` proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TxBlacklistContract {
    contract: Address,
}

impl TxBlacklistContract {
    /// Creates a handle for `contract`.
    #[inline]
    pub const fn new(contract: Address) -> Self {
        Self { contract }
    }

    /// Returns the configured proxy address.
    #[inline]
    pub const fn contract(self) -> Address {
        self.contract
    }

    /// Calls `isBlacklisted(sourceHash)`.
    ///
    /// Deterministic EVM outcomes and decoding failures are fail-open. Database and other local
    /// infrastructure errors are propagated because interpreting them as `false` could make two
    /// nodes execute the same deposit differently.
    pub fn is_blacklisted<E: Evm>(&self, evm: &mut E, source_hash: B256) -> Result<bool, E::Error> {
        let result = evm.transact_system_call(
            SYSTEM_ADDRESS,
            self.contract,
            encode_is_blacklisted(source_hash),
        )?;

        Ok(decode_is_blacklisted(result.result))
    }
}

/// Returns the source hash only when `tx` is an L1 user deposit eligible for interception.
#[inline]
pub fn interceptable_user_deposit_source(tx: &OpTx) -> Option<B256> {
    if tx.tx_type() != DEPOSIT_TRANSACTION_TYPE ||
        tx.is_system_transaction() ||
        tx.caller() == L1_ATTRIBUTES_DEPOSITOR
    {
        return None;
    }

    tx.source_hash()
}

fn encode_is_blacklisted(source_hash: B256) -> Bytes {
    let mut calldata = [0u8; 36];
    calldata[..4].copy_from_slice(&IS_BLACKLISTED_SELECTOR);
    calldata[4..].copy_from_slice(source_hash.as_slice());
    Bytes::copy_from_slice(&calldata)
}

fn decode_is_blacklisted<H>(result: ExecutionResult<H>) -> bool {
    match result {
        ExecutionResult::Success { output: Output::Call(data), .. } => {
            decode_is_blacklisted_word(data.as_ref())
        }
        _ => false,
    }
}

fn decode_is_blacklisted_word(data: &[u8]) -> bool {
    data.len() >= 32 && data[..31].iter().all(|byte| *byte == 0) && data[31] == 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OpEvmFactory;
    use alloy_evm::{EvmEnv, EvmFactory};
    use alloy_primitives::U256;
    use op_revm::{OpTransaction, transaction::deposit::DepositTransactionParts};
    use revm::{
        Database,
        context::{BlockEnv, CfgEnv, TxEnv},
        database::{DBErrorMarker, InMemoryDB},
        state::{AccountInfo, Bytecode},
    };

    const TEST_CONTRACT: Address = Address::repeat_byte(0x77);

    fn database_with_contract(code: &'static [u8]) -> InMemoryDB {
        let mut db = InMemoryDB::default();
        let code = Bytecode::new_raw(Bytes::from_static(code));
        db.insert_account_info(
            TEST_CONTRACT,
            AccountInfo { code_hash: code.hash_slow(), code: Some(code), ..Default::default() },
        );
        db
    }

    fn test_env() -> EvmEnv<op_revm::OpSpecId, BlockEnv> {
        EvmEnv::new(
            CfgEnv::new_with_spec(op_revm::OpSpecId::REGOLITH),
            BlockEnv { gas_limit: 30_000_000, ..Default::default() },
        )
    }

    fn deposit(caller: Address, source_hash: B256, is_system_transaction: bool) -> OpTx {
        OpTx(OpTransaction {
            base: TxEnv { tx_type: DEPOSIT_TRANSACTION_TYPE, caller, ..Default::default() },
            deposit: DepositTransactionParts { source_hash, mint: None, is_system_transaction },
            ..Default::default()
        })
    }

    #[test]
    fn user_deposit_is_interceptable() {
        let hash = B256::repeat_byte(0x42);
        assert_eq!(
            interceptable_user_deposit_source(&deposit(Address::repeat_byte(0x11), hash, false)),
            Some(hash)
        );
    }

    #[test]
    fn system_deposit_is_not_interceptable() {
        let tx = deposit(Address::repeat_byte(0x11), B256::repeat_byte(0x42), true);
        assert_eq!(interceptable_user_deposit_source(&tx), None);
    }

    #[test]
    fn post_regolith_attributes_is_not_interceptable() {
        let tx = deposit(L1_ATTRIBUTES_DEPOSITOR, B256::repeat_byte(0x42), false);
        assert_eq!(interceptable_user_deposit_source(&tx), None);
    }

    #[test]
    fn zero_address_user_deposit_is_interceptable() {
        let hash = B256::repeat_byte(0x42);
        let tx = deposit(Address::ZERO, hash, false);
        assert_eq!(interceptable_user_deposit_source(&tx), Some(hash));
    }

    #[test]
    fn normal_transaction_is_not_candidate() {
        let mut tx = OpTx::default();
        tx.base.tx_type = 2;
        assert_eq!(interceptable_user_deposit_source(&tx), None);
    }

    #[test]
    fn encodes_is_blacklisted_calldata() {
        let hash = B256::repeat_byte(0x42);
        let calldata = encode_is_blacklisted(hash);
        assert_eq!(calldata.len(), 36);
        assert_eq!(&calldata[..4], &IS_BLACKLISTED_SELECTOR);
        assert_eq!(&calldata[4..], hash.as_slice());
    }

    #[test]
    fn decodes_canonical_true() {
        let mut word = [0u8; 32];
        word[31] = 1;
        assert!(decode_is_blacklisted_word(&word));
    }

    #[test]
    fn decodes_canonical_false() {
        assert!(!decode_is_blacklisted_word(&[0u8; 32]));
    }

    #[test]
    fn rejects_noncanonical_bool() {
        let mut word = [0u8; 32];
        word[31] = 2;
        assert!(!decode_is_blacklisted_word(&word));
        assert!(!decode_is_blacklisted_word(&[0xff; 32]));
    }

    #[test]
    fn rejects_empty_or_short_output() {
        for len in 0..32 {
            assert!(!decode_is_blacklisted_word(&[0u8; 31][..len]));
        }
    }

    #[test]
    fn abi_system_call_true_false_and_no_code() {
        // `mstore(0, 1); return(0, 32)`.
        let mut evm = OpEvmFactory::<OpTx>::default().create_evm(
            database_with_contract(&[0x60, 0x01, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xf3]),
            test_env(),
        );
        assert!(
            TxBlacklistContract::new(TEST_CONTRACT)
                .is_blacklisted(&mut evm, B256::repeat_byte(0x42))
                .unwrap()
        );

        // `mstore(0, 0); return(0, 32)`.
        let mut evm = OpEvmFactory::<OpTx>::default().create_evm(
            database_with_contract(&[0x60, 0x00, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xf3]),
            test_env(),
        );
        assert!(
            !TxBlacklistContract::new(TEST_CONTRACT)
                .is_blacklisted(&mut evm, B256::repeat_byte(0x42))
                .unwrap()
        );

        let mut evm = OpEvmFactory::<OpTx>::default().create_evm(InMemoryDB::default(), test_env());
        assert!(
            !TxBlacklistContract::new(TEST_CONTRACT)
                .is_blacklisted(&mut evm, B256::repeat_byte(0x42))
                .unwrap()
        );
    }

    #[test]
    fn revert_and_malformed_system_call_fail_open() {
        for code in [
            &[0x60, 0x00, 0x60, 0x00, 0xfd][..],
            // Return one byte containing 1 instead of a complete ABI word.
            &[0x60, 0x01, 0x60, 0x00, 0x53, 0x60, 0x01, 0x60, 0x00, 0xf3][..],
            // Return non-canonical word 2.
            &[0x60, 0x02, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xf3][..],
        ] {
            let mut evm = OpEvmFactory::<OpTx>::default()
                .create_evm(database_with_contract(code), test_env());
            assert!(
                !TxBlacklistContract::new(TEST_CONTRACT)
                    .is_blacklisted(&mut evm, B256::repeat_byte(0x42))
                    .unwrap()
            );
        }
    }

    #[test]
    fn it_query_discards_side_effects() {
        // `sstore(0, 1); mstore(0, 1); return(0, 32)`.
        let mut evm = OpEvmFactory::<OpTx>::default().create_evm(
            database_with_contract(&[
                0x60, 0x01, 0x60, 0x00, 0x55, 0x60, 0x01, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00,
                0xf3,
            ]),
            test_env(),
        );
        assert!(
            TxBlacklistContract::new(TEST_CONTRACT)
                .is_blacklisted(&mut evm, B256::repeat_byte(0x42))
                .unwrap()
        );
        assert_eq!(evm.components_mut().0.storage(TEST_CONTRACT, U256::ZERO).unwrap(), U256::ZERO);
    }

    #[test]
    fn it_proxy_abi_true_false() {
        fn proxy_code(implementation: Address) -> Bytecode {
            let mut code = alloc::vec![0x36, 0x3d, 0x3d, 0x37, 0x3d, 0x3d, 0x3d, 0x36, 0x3d, 0x73,];
            code.extend_from_slice(implementation.as_slice());
            code.extend_from_slice(&[
                0x5a, 0xf4, 0x3d, 0x82, 0x80, 0x3e, 0x90, 0x3d, 0x91, 0x60, 0x2b, 0x57, 0xfd, 0x5b,
                0xf3,
            ]);
            Bytecode::new_raw(Bytes::from(code))
        }

        let implementation = Address::repeat_byte(0x31);
        let blacklisted_hash = B256::repeat_byte(0x42);
        let mut db = InMemoryDB::default();
        let proxy = proxy_code(implementation);
        db.insert_account_info(
            TEST_CONTRACT,
            AccountInfo { code_hash: proxy.hash_slow(), code: Some(proxy), ..Default::default() },
        );
        // The implementation compares calldata word 0 (`sourceHash`) with proxy storage slot 0,
        // then ABI-encodes the equality result. DELEGATECALL makes SLOAD read proxy storage.
        let implementation_code = Bytecode::new_raw(Bytes::from_static(&[
            0x60, 0x04, 0x35, 0x60, 0x00, 0x54, 0x14, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00,
            0xf3,
        ]));
        db.insert_account_info(
            implementation,
            AccountInfo {
                code_hash: implementation_code.hash_slow(),
                code: Some(implementation_code),
                ..Default::default()
            },
        );
        db.insert_account_storage(
            TEST_CONTRACT,
            U256::ZERO,
            U256::from_be_slice(blacklisted_hash.as_slice()),
        )
        .unwrap();

        let mut evm = OpEvmFactory::<OpTx>::default().create_evm(db, test_env());
        let contract = TxBlacklistContract::new(TEST_CONTRACT);
        assert!(contract.is_blacklisted(&mut evm, blacklisted_hash).unwrap());
        assert!(!contract.is_blacklisted(&mut evm, B256::repeat_byte(0x43)).unwrap());
    }

    #[derive(Debug)]
    struct FailingDbError;

    impl core::fmt::Display for FailingDbError {
        fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            formatter.write_str("failing db")
        }
    }

    impl core::error::Error for FailingDbError {}
    impl DBErrorMarker for FailingDbError {}

    #[derive(Debug)]
    struct FailingDb;

    impl Database for FailingDb {
        type Error = FailingDbError;

        fn basic(&mut self, _: Address) -> Result<Option<AccountInfo>, Self::Error> {
            Err(FailingDbError)
        }

        fn code_by_hash(&mut self, _: B256) -> Result<Bytecode, Self::Error> {
            Err(FailingDbError)
        }

        fn storage(&mut self, _: Address, _: U256) -> Result<U256, Self::Error> {
            Err(FailingDbError)
        }

        fn block_hash(&mut self, _: u64) -> Result<B256, Self::Error> {
            Err(FailingDbError)
        }
    }

    #[test]
    fn database_error_propagates() {
        let mut evm = OpEvmFactory::<OpTx>::default().create_evm(FailingDb, test_env());
        assert!(matches!(
            TxBlacklistContract::new(TEST_CONTRACT)
                .is_blacklisted(&mut evm, B256::repeat_byte(0x42)),
            Err(revm::context::result::EVMError::Database(FailingDbError))
        ));
    }

    #[test]
    fn rejects_revert_and_halt() {
        for code in [&[0x60, 0x00, 0x60, 0x00, 0xfd][..], &[0xfe][..]] {
            let mut evm = OpEvmFactory::<OpTx>::default()
                .create_evm(database_with_contract(code), test_env());
            assert!(
                !TxBlacklistContract::new(TEST_CONTRACT)
                    .is_blacklisted(&mut evm, B256::repeat_byte(0x42))
                    .unwrap()
            );
        }
    }

    #[test]
    fn no_code_fails_open() {
        let mut evm = OpEvmFactory::<OpTx>::default().create_evm(InMemoryDB::default(), test_env());
        assert!(
            !TxBlacklistContract::new(TEST_CONTRACT)
                .is_blacklisted(&mut evm, B256::repeat_byte(0x42))
                .unwrap()
        );
    }

    #[test]
    fn configured_addresses_match_deployments() {
        assert_eq!(
            XLAYER_DEVNET_BLACKLIST_CONTRACT,
            Some(address!("0xb1ac000000000000000000000000000000000001"))
        );
        assert_eq!(
            XLAYER_TESTNET_BLACKLIST_CONTRACT,
            Some(address!("0x59055c0ef0be92018b33f877a3eB816355791727"))
        );
        assert_eq!(XLAYER_MAINNET_BLACKLIST_CONTRACT, XLAYER_TESTNET_BLACKLIST_CONTRACT);
    }

    #[test]
    fn chain_id_mapping_uses_configured_addresses() {
        for (chain_id, contract) in [
            (XLAYER_DEVNET_CHAIN_ID, XLAYER_DEVNET_BLACKLIST_CONTRACT),
            (XLAYER_TESTNET_CHAIN_ID, XLAYER_TESTNET_BLACKLIST_CONTRACT),
            (XLAYER_MAINNET_CHAIN_ID, XLAYER_MAINNET_BLACKLIST_CONTRACT),
        ] {
            assert_eq!(xlayer_blacklist_contract(chain_id), contract);
        }
        assert_eq!(xlayer_blacklist_contract(1), None);
    }

    #[test]
    fn configured_addresses_are_nonzero() {
        for address in [
            XLAYER_DEVNET_BLACKLIST_CONTRACT,
            XLAYER_TESTNET_BLACKLIST_CONTRACT,
            XLAYER_MAINNET_BLACKLIST_CONTRACT,
        ]
        .into_iter()
        .flatten()
        {
            assert!(!address.is_zero());
        }
    }
}
