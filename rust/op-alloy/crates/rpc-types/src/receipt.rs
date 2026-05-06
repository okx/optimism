//! Receipt types for RPC

use alloc::vec::Vec;
use alloy_consensus::{Receipt, ReceiptWithBloom, TxReceipt};
use alloy_primitives::Address;
use alloy_rpc_types_eth::Log;
use alloy_serde::OtherFields;
use op_alloy_consensus::{
    OpDepositReceipt, OpDepositReceiptWithBloom, OpReceipt, OpReceiptEnvelope,
};
use serde::{Deserialize, Serialize};

/// OP Transaction Receipt type
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[doc(alias = "OpTxReceipt")]
pub struct OpTransactionReceipt {
    /// Regular eth transaction receipt including deposit receipts
    #[serde(flatten)]
    pub inner: alloy_rpc_types_eth::TransactionReceipt<ReceiptWithBloom<OpReceipt<Log>>>,
    /// L1 block info of the transaction.
    #[serde(flatten)]
    pub l1_block_info: L1BlockInfo,
    /// Per-transaction gas refund from post-exec block-level warming.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "alloy_serde::quantity::opt")]
    pub op_gas_refund: Option<u64>,
    /// XLayerAA (EIP-8130, tx type `0x7B`) extension fields. `None` for
    /// non-AA receipts; flattened into the JSON for AA receipts.
    #[serde(default, flatten, skip_serializing_if = "Option::is_none")]
    pub eip8130_fields: Option<Eip8130ReceiptFields>,
}

/// Extension fields for XLayerAA (tx type `0x7B`) receipts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Eip8130ReceiptFields {
    /// Address that actually paid gas (equal to `tx.from` unless a sponsor pays).
    pub payer: Address,
    /// Per-phase execution status, one entry per phase in `tx.calls`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_statuses: Option<Vec<bool>>,
}

impl alloy_network_primitives::ReceiptResponse for OpTransactionReceipt {
    fn contract_address(&self) -> Option<alloy_primitives::Address> {
        self.inner.contract_address
    }

    fn status(&self) -> bool {
        self.inner.inner.status()
    }

    fn block_hash(&self) -> Option<alloy_primitives::BlockHash> {
        self.inner.block_hash
    }

    fn block_number(&self) -> Option<u64> {
        self.inner.block_number
    }

    fn transaction_hash(&self) -> alloy_primitives::TxHash {
        self.inner.transaction_hash
    }

    fn transaction_index(&self) -> Option<u64> {
        self.inner.transaction_index()
    }

    fn gas_used(&self) -> u64 {
        self.inner.gas_used()
    }

    fn effective_gas_price(&self) -> u128 {
        self.inner.effective_gas_price()
    }

    fn blob_gas_used(&self) -> Option<u64> {
        self.inner.blob_gas_used()
    }

    fn blob_gas_price(&self) -> Option<u128> {
        self.inner.blob_gas_price()
    }

    fn from(&self) -> alloy_primitives::Address {
        self.inner.from()
    }

    fn to(&self) -> Option<alloy_primitives::Address> {
        self.inner.to()
    }

    fn cumulative_gas_used(&self) -> u64 {
        self.inner.cumulative_gas_used()
    }

    fn state_root(&self) -> Option<alloy_primitives::B256> {
        self.inner.state_root()
    }
}

/// Additional fields for Optimism transaction receipts: <https://github.com/ethereum-optimism/op-geth/blob/f2e69450c6eec9c35d56af91389a1c47737206ca/core/types/receipt.go#L87-L87>
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[doc(alias = "OptimismTxReceiptFields")]
pub struct OpTransactionReceiptFields {
    /// L1 block info.
    #[serde(flatten)]
    pub l1_block_info: L1BlockInfo,
    /// Per-transaction gas refund from post-exec block-level warming.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "alloy_serde::quantity::opt")]
    pub op_gas_refund: Option<u64>,
    /* --------------------------------------- Regolith --------------------------------------- */
    /// Deposit nonce for deposit transactions.
    ///
    /// Always null prior to the Regolith hardfork.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "alloy_serde::quantity::opt")]
    pub deposit_nonce: Option<u64>,
    /* ---------------------------------------- Canyon ---------------------------------------- */
    /// Deposit receipt version for deposit transactions.
    ///
    /// Always null prior to the Canyon hardfork.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "alloy_serde::quantity::opt")]
    pub deposit_receipt_version: Option<u64>,
}

/// Serialize/Deserialize l1FeeScalar to/from string
mod l1_fee_scalar_serde {
    use serde::{Deserialize, de};

    pub(super) fn serialize<S>(value: &Option<f64>, s: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use alloc::string::ToString;
        if let Some(v) = value {
            return s.serialize_str(&v.to_string());
        }
        s.serialize_none()
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use alloc::string::String;
        let s: Option<String> = Option::deserialize(deserializer)?;
        if let Some(s) = s {
            return Ok(Some(s.parse::<f64>().map_err(de::Error::custom)?));
        }

        Ok(None)
    }
}

impl From<OpTransactionReceiptFields> for OtherFields {
    fn from(value: OpTransactionReceiptFields) -> Self {
        serde_json::to_value(value).unwrap().try_into().unwrap()
    }
}

/// L1 block info extracted from input of first transaction in every block.
///
/// The subset of [`OpTransactionReceiptFields`], that encompasses L1 block
/// info:
/// <https://github.com/ethereum-optimism/op-geth/blob/f2e69450c6eec9c35d56af91389a1c47737206ca/core/types/receipt.go#L87-L87>
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct L1BlockInfo {
    /// L1 base fee is the minimum price per unit of gas.
    ///
    /// Present from pre-bedrock as de facto L1 price per unit of gas. L1 base fee after Bedrock.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "alloy_serde::quantity::opt")]
    pub l1_gas_price: Option<u128>,
    /// L1 gas used.
    ///
    /// Present from pre-bedrock, deprecated as of Fjord.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "alloy_serde::quantity::opt")]
    pub l1_gas_used: Option<u128>,
    /// L1 fee for the transaction.
    ///
    /// Present from pre-bedrock.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "alloy_serde::quantity::opt")]
    pub l1_fee: Option<u128>,
    /// L1 fee scalar for the transaction
    ///
    /// Present from pre-bedrock to Ecotone. Null after Ecotone.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "l1_fee_scalar_serde")]
    pub l1_fee_scalar: Option<f64>,
    /* ---------------------------------------- Ecotone ---------------------------------------- */
    /// L1 base fee scalar. Applied to base fee to compute weighted gas price multiplier.
    ///
    /// Always null prior to the Ecotone hardfork.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "alloy_serde::quantity::opt")]
    pub l1_base_fee_scalar: Option<u128>,
    /// L1 blob base fee.
    ///
    /// Always null prior to the Ecotone hardfork.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "alloy_serde::quantity::opt")]
    pub l1_blob_base_fee: Option<u128>,
    /// L1 blob base fee scalar. Applied to blob base fee to compute weighted gas price multiplier.
    ///
    /// Always null prior to the Ecotone hardfork.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "alloy_serde::quantity::opt")]
    pub l1_blob_base_fee_scalar: Option<u128>,
    /* ---------------------------------------- Isthmus ---------------------------------------- */
    /// Operator fee scalar.
    ///
    /// Always null prior to the Isthmus hardfork.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "alloy_serde::quantity::opt")]
    pub operator_fee_scalar: Option<u128>,
    /// Operator fee constant.
    ///
    /// Always null prior to the Isthmus hardfork.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "alloy_serde::quantity::opt")]
    pub operator_fee_constant: Option<u128>,
    /* ---------------------------------------- Jovian ---------------------------------------- */
    /// DA footprint gas scalar. Used to set the DA footprint block limit on the L2.
    ///
    /// Always null prior to the Jovian hardfork.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "alloy_serde::quantity::opt")]
    pub da_footprint_gas_scalar: Option<u16>,
}

impl Eq for L1BlockInfo {}

impl From<OpTransactionReceipt> for OpReceiptEnvelope<alloy_primitives::Log> {
    fn from(value: OpTransactionReceipt) -> Self {
        let inner_envelope = value.inner.inner.into();

        /// Helper function to convert the inner logs within a [`ReceiptWithBloom`] from RPC to
        /// consensus types.
        #[inline(always)]
        fn convert_standard_receipt(
            receipt: ReceiptWithBloom<Receipt<alloy_rpc_types_eth::Log>>,
        ) -> ReceiptWithBloom<Receipt<alloy_primitives::Log>> {
            let ReceiptWithBloom { logs_bloom, receipt } = receipt;

            let consensus_logs = receipt.logs.into_iter().map(|log| log.inner).collect();
            ReceiptWithBloom {
                receipt: Receipt {
                    status: receipt.status,
                    cumulative_gas_used: receipt.cumulative_gas_used,
                    logs: consensus_logs,
                },
                logs_bloom,
            }
        }

        match inner_envelope {
            OpReceiptEnvelope::Legacy(receipt) => Self::Legacy(convert_standard_receipt(receipt)),
            OpReceiptEnvelope::Eip2930(receipt) => Self::Eip2930(convert_standard_receipt(receipt)),
            OpReceiptEnvelope::Eip1559(receipt) => Self::Eip1559(convert_standard_receipt(receipt)),
            OpReceiptEnvelope::Eip7702(receipt) => Self::Eip7702(convert_standard_receipt(receipt)),
            // #TODO(xlayer-eip8130): RPC conversion currently drops EIP-8130 extensions required
            // by eth_getTransactionReceipt: payer and phaseStatuses.
            OpReceiptEnvelope::Eip8130(receipt) => Self::Eip8130(convert_standard_receipt(receipt)),
            OpReceiptEnvelope::PostExec(receipt) => {
                Self::PostExec(convert_standard_receipt(receipt))
            }
            OpReceiptEnvelope::Deposit(OpDepositReceiptWithBloom { logs_bloom, receipt }) => {
                let consensus_logs = receipt.inner.logs.into_iter().map(|log| log.inner).collect();
                let consensus_receipt = OpDepositReceiptWithBloom {
                    receipt: OpDepositReceipt {
                        inner: Receipt {
                            status: receipt.inner.status,
                            cumulative_gas_used: receipt.inner.cumulative_gas_used,
                            logs: consensus_logs,
                        },
                        deposit_nonce: receipt.deposit_nonce,
                        deposit_receipt_version: receipt.deposit_receipt_version,
                    },
                    logs_bloom,
                };
                Self::Deposit(consensus_receipt)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use serde_json::{Value, json};

    // <https://github.com/alloy-rs/op-alloy/issues/18>
    #[test]
    fn parse_rpc_receipt() {
        let s = r#"{
        "blockHash": "0x9e6a0fb7e22159d943d760608cc36a0fb596d1ab3c997146f5b7c55c8c718c67",
        "blockNumber": "0x6cfef89",
        "contractAddress": null,
        "cumulativeGasUsed": "0xfa0d",
        "depositNonce": "0x8a2d11",
        "effectiveGasPrice": "0x0",
        "from": "0xdeaddeaddeaddeaddeaddeaddeaddeaddead0001",
        "gasUsed": "0xfa0d",
        "logs": [],
        "logsBloom": "0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
        "status": "0x1",
        "to": "0x4200000000000000000000000000000000000015",
        "transactionHash": "0xb7c74afdeb7c89fb9de2c312f49b38cb7a850ba36e064734c5223a477e83fdc9",
        "transactionIndex": "0x0",
        "type": "0x7e",
        "l1GasPrice": "0x3ef12787",
        "l1GasUsed": "0x1177",
        "l1Fee": "0x5bf1ab43d",
        "l1BaseFeeScalar": "0x1",
        "l1BlobBaseFee": "0x600ab8f05e64",
        "l1BlobBaseFeeScalar": "0x1",
        "operatorFeeScalar": "0x1",
        "operatorFeeConstant": "0x1",
        "daFootprintGasScalar": "0x1"
    }"#;

        let receipt: OpTransactionReceipt = serde_json::from_str(s).unwrap();
        let value = serde_json::to_value(&receipt).unwrap();
        let expected_value = serde_json::from_str::<serde_json::Value>(s).unwrap();
        assert_eq!(value, expected_value);
    }

    #[test]
    fn serialize_empty_optimism_transaction_receipt_fields_struct() {
        let op_fields = OpTransactionReceiptFields::default();

        let json = serde_json::to_value(op_fields).unwrap();
        assert_eq!(json, json!({}));
    }

    #[test]
    fn serialize_l1_fee_scalar() {
        let op_fields = OpTransactionReceiptFields {
            l1_block_info: L1BlockInfo { l1_fee_scalar: Some(0.678), ..Default::default() },
            ..Default::default()
        };

        let json = serde_json::to_value(op_fields).unwrap();

        assert_eq!(json["l1FeeScalar"], serde_json::Value::String("0.678".to_string()));
    }

    #[test]
    fn deserialize_l1_fee_scalar() {
        let json = json!({
            "l1FeeScalar": "0.678"
        });

        let op_fields: OpTransactionReceiptFields = serde_json::from_value(json).unwrap();
        assert_eq!(op_fields.l1_block_info.l1_fee_scalar, Some(0.678f64));

        let json = json!({
            "l1FeeScalar": Value::Null
        });

        let op_fields: OpTransactionReceiptFields = serde_json::from_value(json).unwrap();
        assert_eq!(op_fields.l1_block_info.l1_fee_scalar, None);

        let json = json!({});

        let op_fields: OpTransactionReceiptFields = serde_json::from_value(json).unwrap();
        assert_eq!(op_fields.l1_block_info.l1_fee_scalar, None);
    }

    /// Baseline deposit receipt JSON used as a starting point for the EIP-8130 wire-shape
    /// tests below. Mirrors the body in `parse_rpc_receipt`; kept inline so each test is
    /// self-contained.
    fn baseline_receipt_json() -> &'static str {
        r#"{
        "blockHash": "0x9e6a0fb7e22159d943d760608cc36a0fb596d1ab3c997146f5b7c55c8c718c67",
        "blockNumber": "0x6cfef89",
        "contractAddress": null,
        "cumulativeGasUsed": "0xfa0d",
        "effectiveGasPrice": "0x0",
        "from": "0xdeaddeaddeaddeaddeaddeaddeaddeaddead0001",
        "gasUsed": "0xfa0d",
        "logs": [],
        "logsBloom": "0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
        "status": "0x1",
        "to": "0x4200000000000000000000000000000000000015",
        "transactionHash": "0xb7c74afdeb7c89fb9de2c312f49b38cb7a850ba36e064734c5223a477e83fdc9",
        "transactionIndex": "0x0",
        "type": "0x7b"
    }"#
    }

    /// Pin the wire shape: AA receipts surface `payer` and `phaseStatuses` as **flattened
    /// top-level keys** (matches Base byte-for-byte). A serde regression that nests them
    /// under `eip8130Fields` would silently break wallets without this pin.
    #[test]
    fn eip8130_fields_serialize_as_flattened_top_level_keys() {
        let mut receipt: OpTransactionReceipt =
            serde_json::from_str(baseline_receipt_json()).unwrap();
        receipt.eip8130_fields = Some(Eip8130ReceiptFields {
            payer: alloy_primitives::address!("0x9999999999999999999999999999999999999999"),
            phase_statuses: Some(alloc::vec![true, false]),
        });

        let json = serde_json::to_value(&receipt).unwrap();
        let obj = json.as_object().expect("receipt is a JSON object");

        assert!(obj.contains_key("payer"), "payer must be at top level, not nested");
        assert!(
            obj.contains_key("phaseStatuses"),
            "phaseStatuses must be at top level (camelCased)"
        );
        assert!(
            !obj.contains_key("eip8130Fields"),
            "eip8130_fields must be flattened, not appear as a nested object"
        );
        assert!(
            !obj.contains_key("phase_statuses"),
            "snake_case phase_statuses must not leak into wire format"
        );
        assert_eq!(
            obj["payer"],
            Value::String("0x9999999999999999999999999999999999999999".to_string()),
        );
        assert_eq!(obj["phaseStatuses"], json!([true, false]));
    }

    /// `eip8130_fields == None` must produce **no** AA-related keys in the wire output —
    /// otherwise non-AA receipts gain phantom fields that confuse generic OP tooling.
    #[test]
    fn eip8130_fields_skipped_when_none() {
        let receipt: OpTransactionReceipt =
            serde_json::from_str(baseline_receipt_json()).unwrap();
        assert!(receipt.eip8130_fields.is_none(), "baseline parses with no AA fields");

        let json = serde_json::to_value(&receipt).unwrap();
        let obj = json.as_object().expect("receipt is a JSON object");

        assert!(!obj.contains_key("payer"), "payer must not appear when no AA fields");
        assert!(
            !obj.contains_key("phaseStatuses"),
            "phaseStatuses must not appear when no AA fields"
        );
    }

    /// `phase_statuses == None` (the indeterminate-multi-phase-success case) must drop
    /// the `phaseStatuses` key entirely while still surfacing `payer`. Without this,
    /// clients would have to distinguish "no AA receipt" vs. "AA receipt with unknown
    /// per-phase outcome" via something other than key presence.
    #[test]
    fn aa_receipt_with_indeterminate_phase_statuses_drops_just_phase_statuses_key() {
        let mut receipt: OpTransactionReceipt =
            serde_json::from_str(baseline_receipt_json()).unwrap();
        let payer = alloy_primitives::address!("0x9999999999999999999999999999999999999999");
        receipt.eip8130_fields = Some(Eip8130ReceiptFields { payer, phase_statuses: None });

        let json = serde_json::to_value(&receipt).unwrap();
        let obj = json.as_object().expect("receipt is a JSON object");

        assert!(obj.contains_key("payer"), "payer must still appear");
        assert!(
            !obj.contains_key("phaseStatuses"),
            "phaseStatuses must be omitted when None"
        );
    }

    /// Round-trip parity: an AA receipt with both fields populated must
    /// deserialize-then-serialize to the same JSON object (modulo key ordering),
    /// guaranteeing the flattened layout survives a full wire cycle.
    #[test]
    fn aa_receipt_json_round_trip() {
        let mut input: OpTransactionReceipt =
            serde_json::from_str(baseline_receipt_json()).unwrap();
        input.eip8130_fields = Some(Eip8130ReceiptFields {
            payer: alloy_primitives::address!("0x9999999999999999999999999999999999999999"),
            phase_statuses: Some(alloc::vec![true, true, false]),
        });

        let serialized = serde_json::to_value(&input).unwrap();
        let reparsed: OpTransactionReceipt = serde_json::from_value(serialized.clone()).unwrap();

        assert_eq!(reparsed.eip8130_fields, input.eip8130_fields);
        assert_eq!(serde_json::to_value(&reparsed).unwrap(), serialized);
    }
}
