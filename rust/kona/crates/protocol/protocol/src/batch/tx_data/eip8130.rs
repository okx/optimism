//! Span-batch transaction data for EIP-8130 (XLayerAA) transactions.

use alloc::vec::Vec;

use alloy_primitives::{Address, Bytes, U256};
use alloy_rlp::{RlpDecodable, RlpEncodable};
use op_alloy_consensus::eip8130::{AccountChangeEntry, Call, TxEip8130};

use crate::{SpanBatchError, SpanDecodingError};

/// Span-batch encoding of an EIP-8130 (XLayerAA) transaction.
///
/// The fields that are shared across all txs in a span batch — `nonce_sequence`
/// (stored in the `tx_nonces` column) and `gas_limit` (stored in `tx_gases`) —
/// are **not** stored here; they are supplied by the caller when reconstructing
/// the full [`TxEip8130`] via [`to_tx`][Self::to_tx].
///
/// # Fee model
///
/// XLayer does not run EIP-1559 dynamic base-fee accounting — it uses flat
/// legacy-style `gas_price`. This diverges from Base's `(max_priority_fee_per_gas,
/// max_fee_per_gas)` pair; cross-chain tooling must be configured per deployment.
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
pub struct SpanBatchEip8130TransactionData {
    /// Sender address. [`Address::ZERO`] indicates EOA-mode sender recovery.
    pub from: Address,
    /// 2D nonce channel selector.
    pub nonce_key: U256,
    /// Block-timestamp expiry. `0` disables expiry for sequenced txs.
    pub expiry: u64,
    /// Flat gas price (wei per gas). XLayer legacy-style; no tip/base-fee split.
    pub gas_price: U256,
    /// CREATE2 deployments, config changes, and/or delegation entries.
    pub account_changes: Vec<AccountChangeEntry>,
    /// Phased call batches.
    pub calls: Vec<Vec<Call>>,
    /// Sponsor address. [`Address::ZERO`] indicates self-pay.
    pub payer: Address,
    /// Sender authentication blob.
    pub sender_auth: Bytes,
    /// Payer authentication blob. Empty when self-pay.
    pub payer_auth: Bytes,
}

impl SpanBatchEip8130TransactionData {
    /// Reconstructs a [`TxEip8130`] from the span-batch fields plus the
    /// per-column values supplied by the caller.
    ///
    /// `nonce` is the `nonce_sequence` (from the batch `tx_nonces` column).
    /// `gas` is the `gas_limit` (from the batch `tx_gases` column).
    pub fn to_tx(&self, nonce: u64, gas: u64, chain_id: u64) -> Result<TxEip8130, SpanBatchError> {
        let gas_price =
            u128::from_be_bytes(self.gas_price.to_be_bytes::<32>()[16..].try_into().map_err(
                |_| SpanBatchError::Decoding(SpanDecodingError::InvalidTransactionData),
            )?);

        Ok(TxEip8130 {
            chain_id,
            from: (self.from != Address::ZERO).then_some(self.from),
            nonce_key: self.nonce_key,
            nonce_sequence: nonce,
            expiry: self.expiry,
            gas_price,
            gas_limit: gas,
            account_changes: self.account_changes.clone(),
            calls: self.calls.clone(),
            payer: (self.payer != Address::ZERO).then_some(self.payer),
            sender_auth: self.sender_auth.clone(),
            payer_auth: self.payer_auth.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use alloc::{vec, vec::Vec};

    use alloy_rlp::{Decodable, Encodable};
    use op_alloy_consensus::eip8130::Call;

    use super::*;
    use crate::SpanBatchTransactionData;

    #[test]
    fn encode_eip8130_tx_data_roundtrip() {
        let aa_tx = SpanBatchEip8130TransactionData {
            from: Address::repeat_byte(0xAA),
            nonce_key: U256::from(0xBBu64),
            expiry: 123,
            gas_price: U256::from(1_000_000_000u64),
            account_changes: vec![],
            calls: vec![vec![Call {
                to: Address::repeat_byte(0xEE),
                data: Bytes::from(vec![0x01, 0x02, 0x03]),
            }]],
            payer: Address::ZERO,
            sender_auth: Bytes::from(vec![0x04, 0x05]),
            payer_auth: Bytes::new(),
        };

        let mut encoded = Vec::new();
        SpanBatchTransactionData::Eip8130(aa_tx.clone()).encode(&mut encoded);
        let decoded = SpanBatchTransactionData::decode(&mut encoded.as_slice()).unwrap();
        let SpanBatchTransactionData::Eip8130(got) = decoded else {
            panic!("expected Eip8130, got {decoded:?}");
        };
        assert_eq!(aa_tx, got);
    }

    #[test]
    fn to_tx_roundtrip() {
        let data = SpanBatchEip8130TransactionData {
            from: Address::ZERO,
            nonce_key: U256::from(0u64),
            expiry: 0,
            gas_price: U256::from(1_000_000_000u64),
            account_changes: vec![],
            calls: vec![vec![Call { to: Address::repeat_byte(1), data: Bytes::new() }]],
            payer: Address::ZERO,
            sender_auth: Bytes::from(vec![0u8; 65]),
            payer_auth: Bytes::new(),
        };
        let tx = data.to_tx(42, 200_000, 1).unwrap();
        assert_eq!(tx.nonce_sequence, 42);
        assert_eq!(tx.gas_limit, 200_000);
        assert_eq!(tx.chain_id, 1);
        assert_eq!(tx.gas_price, 1_000_000_000);
        assert!(tx.from.is_none()); // ZERO → None
        assert!(tx.payer.is_none()); // ZERO → None
    }
}
