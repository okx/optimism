//! Flashblock metadata types.

use alloc::collections::BTreeMap;
use alloy_eip7928::BlockAccessList;
use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_rlp::Decodable;
use op_alloy_consensus::OpReceipt;

/// Provides metadata about the block that may be useful for indexing or analysis.
// Note: this uses mixed camel, snake case: <https://github.com/flashbots/rollup-boost/blob/dd12e8e8366004b4758bfa0cfa98efa6929b7e9f/crates/flashblocks-rpc/src/cache.rs#L31>
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct OpFlashblockPayloadMetadata {
    /// The number of the block in the L2 chain.
    pub block_number: u64,
    /// A map of addresses to their updated balances after the block execution.
    /// This represents balance changes due to transactions, rewards, or system transfers.
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub new_account_balances: Option<BTreeMap<Address, U256>>,
    /// Execution receipts for all transactions in the block.
    /// Contains logs, gas usage, and other EVM-level metadata.
    #[cfg_attr(
        feature = "serde",
        serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_flashblock_receipts"
        )
    )]
    pub receipts: Option<BTreeMap<B256, OpReceipt>>,
    /// Optional [EIP-7928](https://eips.ethereum.org/EIPS/eip-7928) RLP-encoded block access list
    /// containing per-account changes (storage reads/writes, balance changes, nonce changes, and
    /// code changes) reads and writes for this flashblock, indexed by transaction position.
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub access_list: Option<Bytes>,
}

impl OpFlashblockPayloadMetadata {
    /// Constructs the new flashblocks payload metadata and, RLP-encoding the flashblock access
    /// list (if any).
    pub fn new(
        block_number: u64,
        new_account_balances: Option<BTreeMap<Address, U256>>,
        receipts: Option<BTreeMap<B256, OpReceipt>>,
        access_list: Option<BlockAccessList>,
    ) -> Self {
        Self {
            block_number,
            new_account_balances,
            receipts,
            access_list: access_list.map(|list| Bytes::from(alloy_rlp::encode(&list))),
        }
    }

    /// Decodes the access list from its RLP-encoded form.
    pub fn block_access_list(&self) -> Option<Result<BlockAccessList, alloy_rlp::Error>> {
        self.access_list.as_ref().map(|raw| {
            let mut buf = raw.as_ref();
            let decoded = BlockAccessList::decode(&mut buf)?;
            if !buf.is_empty() {
                return Err(alloy_rlp::Error::UnexpectedLength);
            }
            Ok(decoded)
        })
    }
}

#[cfg(feature = "serde")]
/// Supports deserializing flashblocks with externally tag receipts for backwards compatibility.
fn deserialize_flashblock_receipts<'de, D>(
    deserializer: D,
) -> Result<Option<BTreeMap<B256, OpReceipt>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use alloy_consensus::Receipt;
    use op_alloy_consensus::OpDepositReceipt;
    use serde::Deserialize;

    #[derive(Deserialize)]
    enum ExternallyTagged {
        Legacy(Receipt),
        Eip2930(Receipt),
        Eip1559(Receipt),
        Eip7702(Receipt),
        Deposit(OpDepositReceipt),
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum MaybeExternallyTagged {
        ExternallyTagged(ExternallyTagged),
        InternallyTagged(OpReceipt),
    }

    impl From<MaybeExternallyTagged> for OpReceipt {
        fn from(value: MaybeExternallyTagged) -> Self {
            match value {
                MaybeExternallyTagged::ExternallyTagged(receipt) => match receipt {
                    ExternallyTagged::Legacy(receipt) => Self::Legacy(receipt),
                    ExternallyTagged::Eip2930(receipt) => Self::Eip2930(receipt),
                    ExternallyTagged::Eip1559(receipt) => Self::Eip1559(receipt),
                    ExternallyTagged::Eip7702(receipt) => Self::Eip7702(receipt),
                    ExternallyTagged::Deposit(receipt) => Self::Deposit(receipt),
                },
                MaybeExternallyTagged::InternallyTagged(receipt) => receipt,
            }
        }
    }

    Ok(Option::<BTreeMap<B256, MaybeExternallyTagged>>::deserialize(deserializer)?
        .map(|map| map.into_iter().map(|(hash, receipt)| (hash, receipt.into())).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use alloy_consensus::{Eip658Value, Receipt};
    use alloy_eip7928::{
        AccountChanges, BalanceChange, CodeChange, NonceChange, SlotChanges, StorageChange,
    };
    use alloy_primitives::{Log, address};

    fn sample_legacy_metadata() -> OpFlashblockPayloadMetadata {
        let mut balances = BTreeMap::new();
        balances.insert(address!("0000000000000000000000000000000000000001"), U256::from(1000));

        let mut receipts = BTreeMap::new();
        let receipt = OpReceipt::Legacy(Receipt {
            status: Eip658Value::Eip658(true),
            cumulative_gas_used: 21000,
            logs: Vec::new(),
        });
        receipts.insert(B256::ZERO, receipt);

        OpFlashblockPayloadMetadata::new(100, Some(balances), Some(receipts), None)
    }

    fn sample_block_access_list() -> BlockAccessList {
        alloc::vec![
            AccountChanges {
                address: address!("0000000000000000000000000000000000000042"),
                storage_changes: alloc::vec![SlotChanges {
                    slot: U256::from(1),
                    changes: alloc::vec![
                        StorageChange { block_access_index: 0, new_value: U256::from(100) },
                        StorageChange { block_access_index: 2, new_value: U256::from(200) },
                    ],
                }],
                storage_reads: alloc::vec![U256::from(5), U256::from(10)],
                balance_changes: alloc::vec![BalanceChange {
                    block_access_index: 0,
                    post_balance: U256::from(500),
                }],
                nonce_changes: alloc::vec![NonceChange { block_access_index: 0, new_nonce: 7 }],
                code_changes: alloc::vec![CodeChange {
                    block_access_index: 1,
                    new_code: Bytes::from_static(&[0x60, 0x00, 0x60, 0x00, 0xfd]),
                }],
            },
            AccountChanges {
                address: address!("0000000000000000000000000000000000000099"),
                storage_reads: alloc::vec![U256::from(42)],
                ..Default::default()
            },
        ]
    }

    fn sample_metadata_access_list() -> OpFlashblockPayloadMetadata {
        OpFlashblockPayloadMetadata::new(42, None, None, Some(sample_block_access_list()))
    }

    #[test]
    #[cfg(feature = "serde")]
    fn test_legacy_metadata_serde_roundtrip() {
        let metadata = sample_legacy_metadata();
        let json = serde_json::to_string(&metadata).unwrap();
        let decoded: OpFlashblockPayloadMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(metadata, decoded);
    }

    #[test]
    #[cfg(feature = "serde")]
    fn test_legacy_metadata_snake_case_serialization() {
        let metadata = sample_legacy_metadata();
        let json = serde_json::to_string(&metadata).unwrap();
        assert!(json.contains("block_number"));
        assert!(json.contains("new_account_balances"));
        assert!(json.contains("receipts"));
        assert!(!json.contains("access_list"));
    }

    #[test]
    #[cfg(feature = "serde")]
    fn test_legacy_address_balance_map_serialization() {
        let mut balances = BTreeMap::new();
        balances.insert(address!("0000000000000000000000000000000000000001"), U256::from(1000));
        balances.insert(address!("0000000000000000000000000000000000000002"), U256::from(2000));

        let metadata = OpFlashblockPayloadMetadata::new(1, Some(balances), None, None);

        let json = serde_json::to_value(&metadata).unwrap();
        let balances_obj = json.get("new_account_balances").unwrap();
        assert!(balances_obj.is_object());
        assert!(balances_obj.get("0x0000000000000000000000000000000000000001").is_some());
    }

    #[test]
    #[cfg(feature = "serde")]
    fn test_legacy_receipt_map_serialization() {
        let mut receipts = BTreeMap::new();
        let receipt1 = OpReceipt::Legacy(Receipt {
            status: Eip658Value::Eip658(true),
            cumulative_gas_used: 21000,
            logs: Vec::<Log>::new(),
        });
        receipts.insert(B256::ZERO, receipt1);

        let metadata = OpFlashblockPayloadMetadata::new(1, None, Some(receipts), None);

        let json = serde_json::to_value(&metadata).unwrap();
        let receipts_obj = json.get("receipts").unwrap();
        assert!(receipts_obj.is_object());
        assert!(
            receipts_obj
                .get("0x0000000000000000000000000000000000000000000000000000000000000000")
                .is_some()
        );
    }

    #[test]
    #[cfg(feature = "serde")]
    fn test_receipt_json_format() {
        let mut receipts = BTreeMap::new();
        let receipt = OpReceipt::Legacy(Receipt {
            status: Eip658Value::Eip658(true),
            cumulative_gas_used: 21000,
            logs: Vec::<Log>::new(),
        });
        receipts.insert(B256::ZERO, receipt);

        let metadata = OpFlashblockPayloadMetadata::new(1, None, Some(receipts), None);

        let json = serde_json::to_value(&metadata).unwrap();
        let receipts_obj = json.get("receipts").unwrap();
        let receipt_entry = receipts_obj
            .get("0x0000000000000000000000000000000000000000000000000000000000000000")
            .unwrap();
        assert_eq!(receipt_entry.get("type").unwrap().as_str().unwrap(), "0x0");
    }

    #[test]
    fn test_metadata_default() {
        let metadata = OpFlashblockPayloadMetadata::default();
        assert_eq!(metadata.block_number, 0);
        assert!(metadata.new_account_balances.is_none());
        assert!(metadata.receipts.is_none());
        assert!(metadata.access_list.is_none());
        assert!(metadata.block_access_list().is_none());
    }

    #[test]
    fn test_constructor_encodes_access_list_to_rlp() {
        let bal = sample_block_access_list();
        let metadata = OpFlashblockPayloadMetadata::new(42, None, None, Some(bal.clone()));

        // Field is populated with RLP bytes.
        assert!(metadata.access_list.is_some());
        let raw = metadata.access_list.as_ref().unwrap();
        // Sanity: RLP-encoded bytes round-trip back to the original list.
        let decoded = BlockAccessList::decode(&mut raw.as_ref()).expect("RLP decode succeeds");
        assert_eq!(decoded, bal);

        // The lazy getter returns the same value.
        let from_getter = metadata.block_access_list().expect("present").expect("decodes");
        assert_eq!(from_getter, bal);
    }

    #[test]
    fn test_constructor_with_no_access_list_leaves_field_none() {
        let metadata = OpFlashblockPayloadMetadata::new(1, None, None, None);
        assert!(metadata.access_list.is_none());
        assert!(metadata.block_access_list().is_none());
    }

    #[test]
    fn test_block_access_list_errors_on_malformed_rlp() {
        let metadata = OpFlashblockPayloadMetadata {
            block_number: 1,
            new_account_balances: None,
            receipts: None,
            access_list: Some(Bytes::from_static(&[0xff, 0xff, 0xff])),
        };
        let result = metadata.block_access_list().expect("present");
        assert!(result.is_err(), "malformed RLP must error");
    }

    #[test]
    fn test_block_access_list_errors_on_trailing_bytes() {
        // Encode a valid empty list, then append a junk byte.
        let empty: BlockAccessList = Vec::new();
        let mut bytes = alloy_rlp::encode(&empty);
        bytes.push(0x42);
        let metadata = OpFlashblockPayloadMetadata {
            block_number: 1,
            new_account_balances: None,
            receipts: None,
            access_list: Some(Bytes::from(bytes)),
        };
        let result = metadata.block_access_list().expect("present");
        assert!(matches!(result, Err(alloy_rlp::Error::UnexpectedLength)));
    }

    #[test]
    #[cfg(feature = "serde")]
    fn test_access_list_metadata_serde_roundtrip() {
        let metadata = sample_metadata_access_list();
        let json = serde_json::to_string(&metadata).unwrap();
        // Ensure no legacy fields present
        assert!(!json.contains("receipts"), "None receipts must be omitted from JSON");
        assert!(!json.contains("new_account_balances"), "None balances must be omitted from JSON");

        // Ensure access_list is present and rendered as a hex string (RLP bytes).
        assert!(json.contains("block_number"));
        assert!(json.contains("access_list"));
        assert!(json.contains("\"0x"));

        let decoded: OpFlashblockPayloadMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, metadata);
        // The structured form decodes cleanly on the receive side.
        let bal = decoded.block_access_list().expect("present").expect("decodes");
        assert_eq!(bal, sample_block_access_list());
    }

    #[test]
    #[cfg(feature = "serde")]
    fn test_backwards_compat_legacy_metadata_missing_access_list() {
        // Old payload JSON without access_list field should deserialize with access_list = None
        let json = r#"{"block_number":100,"new_account_balances":{},"receipts":{}}"#;
        let metadata: OpFlashblockPayloadMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(metadata.block_number, 100);
        assert_eq!(metadata.new_account_balances, Some(BTreeMap::new()));
        assert_eq!(metadata.receipts, Some(BTreeMap::new()));
        assert_eq!(metadata.access_list, None);
        assert!(metadata.block_access_list().is_none());
    }

    #[test]
    #[cfg(feature = "serde")]
    fn test_backwards_compat_minimal_payload_block_number_only() {
        // Minimal JSON: just block_number — all optional fields missing
        let json = r#"{"block_number":1}"#;
        let metadata: OpFlashblockPayloadMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(metadata.block_number, 1);
        assert_eq!(metadata.new_account_balances, None);
        assert_eq!(metadata.receipts, None);
        assert_eq!(metadata.access_list, None);
    }

    #[test]
    #[cfg(feature = "serde")]
    fn test_metadata_none_access_list_omitted_from_json() {
        let metadata =
            OpFlashblockPayloadMetadata::new(1, Some(BTreeMap::new()), Some(BTreeMap::new()), None);

        let json = serde_json::to_string(&metadata).unwrap();
        assert!(!json.contains("access_list"), "None access_list must be omitted from JSON");
        assert!(
            json.contains("new_account_balances"),
            "Some new_account_balances must be included in JSON"
        );
        assert!(json.contains("receipts"), "Some receipts must be included in JSON");
    }

    #[test]
    #[cfg(feature = "serde")]
    fn test_metadata_none_balances_and_receipts_omitted_from_json() {
        let metadata =
            OpFlashblockPayloadMetadata::new(1, None, None, Some(sample_block_access_list()));

        let json = serde_json::to_string(&metadata).unwrap();
        assert!(
            !json.contains("new_account_balances"),
            "None new_account_balances must be omitted from JSON"
        );
        assert!(!json.contains("receipts"), "None receipts must be omitted from JSON");
        assert!(json.contains("access_list"), "Some access_list must be included in JSON");
    }
}
