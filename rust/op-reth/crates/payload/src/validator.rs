//! Validates execution payload wrt Optimism consensus rules

use alloc::sync::Arc;
use alloy_consensus::Block;
use alloy_rpc_types_engine::PayloadError;
use derive_more::{Constructor, Deref};
use op_alloy_rpc_types_engine::{OpExecutionData, OpPayloadError};
use reth_optimism_forks::OpHardforks;
use reth_payload_validator::{cancun, prague, shanghai};
use reth_primitives_traits::{Block as _, SealedBlock, SignedTransaction};

/// Execution payload validator.
#[derive(Clone, Debug, Deref, Constructor)]
pub struct OpExecutionPayloadValidator<ChainSpec> {
    /// Chain spec to validate against.
    #[deref]
    inner: Arc<ChainSpec>,
}

impl<ChainSpec> OpExecutionPayloadValidator<ChainSpec>
where
    ChainSpec: OpHardforks,
{
    /// Returns reference to chain spec.
    pub fn chain_spec(&self) -> &ChainSpec {
        &self.inner
    }

    /// Ensures that the given payload does not violate any consensus rules that concern the block's
    /// layout.
    ///
    /// See also [`ensure_well_formed_payload`].
    pub fn ensure_well_formed_payload<T: SignedTransaction>(
        &self,
        payload: OpExecutionData,
    ) -> Result<SealedBlock<Block<T>>, OpPayloadError> {
        ensure_well_formed_payload(self.chain_spec(), payload)
    }
}

/// Ensures that the given payload does not violate any consensus rules that concern the block's
/// layout, like:
///    - missing or invalid base fee
///    - invalid extra data
///    - invalid transactions
///    - incorrect hash
///    - block contains blob transactions or blob versioned hashes
///    - block contains l1 withdrawals
///
/// The checks are done in the order that conforms with the engine-API specification.
///
/// This is intended to be invoked after receiving the payload from the CLI.
/// The additional fields, starting with [`MaybeCancunPayloadFields`](alloy_rpc_types_engine::MaybeCancunPayloadFields), are not part of the payload, but are additional fields starting in the `engine_newPayloadV3` RPC call, See also <https://specs.optimism.io/protocol/exec-engine.html#engine_newpayloadv3>
///
/// If the cancun fields are provided this also validates that the versioned hashes in the block
/// are empty as well as those passed in the sidecar. If the payload fields are not provided.
///
/// Validation according to specs <https://specs.optimism.io/protocol/exec-engine.html#engine-api>.
pub fn ensure_well_formed_payload<ChainSpec, T>(
    chain_spec: ChainSpec,
    payload: OpExecutionData,
) -> Result<SealedBlock<Block<T>>, OpPayloadError>
where
    ChainSpec: OpHardforks,
    T: SignedTransaction,
{
    let OpExecutionData { payload, sidecar } = payload;

    let expected_hash = payload.block_hash();

    // First parse the block
    let sealed_block = payload.try_into_block_with_sidecar(&sidecar)?.seal_slow();

    // Ensure the hash included in the payload matches the block hash
    if expected_hash != sealed_block.hash() {
        Err(PayloadError::BlockHash { execution: sealed_block.hash(), consensus: expected_hash })?;
    }

    shanghai::ensure_well_formed_fields(
        sealed_block.body(),
        chain_spec.is_shanghai_active_at_timestamp(sealed_block.timestamp),
    )?;

    cancun::ensure_well_formed_header_and_sidecar_fields(
        &sealed_block,
        sidecar.ecotone(),
        chain_spec.is_cancun_active_at_timestamp(sealed_block.timestamp),
    )?;

    prague::ensure_well_formed_fields(
        sealed_block.body(),
        sidecar.isthmus(),
        chain_spec.is_prague_active_at_timestamp(sealed_block.timestamp),
    )?;

    Ok(sealed_block)
}

#[cfg(test)]
mod tests {
    //! Block-level round-trip tests probing the builder→engine path
    //! (`from_block_unchecked` → `try_into_block_with_sidecar` → `seal_slow`).
    //!
    //! `eip8130_block_round_trip_preserves_hash` confirms the codec/header path
    //! is sound when *all* Cancun-side header fields are set consistently: a
    //! header with `parent_beacon_block_root: Some(_)` AND `blob_gas_used:
    //! Some(_)` round-trips through V3 cleanly.
    //!
    //! `v3_payload_drops_blob_gas_when_builder_sets_none` is the direct repro
    //! for the live `engine::tree: Invalid payload ... block hash mismatch` we
    //! see in the EIP-8130 e2e test (and would surface for any tx type under
    //! the same config). It captures the asymmetry inside V3: when the builder
    //! produces a header with `parent_beacon_block_root: Some(_)` but
    //! `blob_gas_used: None` (e.g. a chain spec where the block timestamp
    //! falls before Ecotone but the CL still passes a parent_beacon_block_root
    //! attribute), `OpExecutionPayload::from_block_unchecked` selects the V3
    //! variant on the strength of `parent_beacon_block_root` alone, then the
    //! V3 round-trip rewrites `blob_gas_used = Some(0)` and `excess_blob_gas =
    //! Some(0)` on the way back. The header hash drifts because `None ≠
    //! Some(0)` under header hashing. Mitigation lives at the call-site: align
    //! payload attributes with the chain spec's actual fork schedule (don't
    //! pass `parent_beacon_block_root` for a pre-Ecotone block), or activate
    //! Ecotone alongside whatever fork the test is actually exercising.

    use alloc::vec;
    use alloy_consensus::{
        Block, BlockBody, Header, Sealable,
        constants::{EMPTY_OMMER_ROOT_HASH, EMPTY_ROOT_HASH, EMPTY_WITHDRAWALS},
        proofs,
    };
    use alloy_eips::{eip4895::Withdrawals, merge::BEACON_NONCE};
    use alloy_primitives::{Address, B256, Bytes, U256};
    use op_alloy_consensus::{Eip8130CallEntry, OpTxEnvelope, TxEip8130};
    use op_alloy_rpc_types_engine::OpExecutionPayload;

    /// Builds a single EIP-8130 envelope shaped like the e2e test does.
    fn aa_envelope() -> OpTxEnvelope {
        let tx = TxEip8130 {
            chain_id: 8453,
            from: Some(Address::repeat_byte(0x01)),
            nonce_key: U256::ZERO,
            nonce_sequence: 42,
            expiry: 0,
            max_priority_fee_per_gas: 1_000_000_000,
            max_fee_per_gas: 20_000_000_000,
            gas_limit: 100_000,
            calls: vec![vec![Eip8130CallEntry {
                to: Address::repeat_byte(0xBB),
                data: Bytes::from_static(&[0xDE, 0xAD]),
            }]],
            sender_auth: Bytes::from_static(&[0xFF; 65]),
            ..Default::default()
        };
        OpTxEnvelope::Eip8130(tx.seal_slow())
    }

    /// Mints a Block<OpTxEnvelope> with one EIP-8130 tx and post-Canyon, post-Cancun
    /// header fields populated so `OpExecutionPayload::from_block_unchecked` selects V3.
    fn build_block(transactions: vec::Vec<OpTxEnvelope>) -> Block<OpTxEnvelope> {
        let transactions_root = proofs::calculate_transaction_root(&transactions);
        let header = Header {
            parent_hash: B256::repeat_byte(0x11),
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            beneficiary: Address::repeat_byte(0x22),
            state_root: B256::repeat_byte(0x33),
            transactions_root,
            receipts_root: B256::repeat_byte(0x44),
            withdrawals_root: Some(EMPTY_WITHDRAWALS),
            logs_bloom: Default::default(),
            difficulty: U256::ZERO,
            number: 1,
            gas_limit: 30_000_000,
            gas_used: 100_000,
            timestamp: 1_700_000_000,
            extra_data: Bytes::new(),
            mix_hash: B256::ZERO,
            nonce: BEACON_NONCE.into(),
            base_fee_per_gas: Some(1_000_000_000),
            blob_gas_used: Some(0),
            excess_blob_gas: Some(0),
            parent_beacon_block_root: Some(B256::ZERO),
            // V3-shape (pre-Isthmus): no requests_hash. With this set, the payload
            // builder selects V4, which exercises a different sidecar path.
            requests_hash: None,
            ..Default::default()
        };
        Block::new(
            header,
            BlockBody {
                transactions,
                ommers: Default::default(),
                withdrawals: Some(Withdrawals::default()),
            },
        )
    }

    /// `Block → ExecutionPayload (V3) → try_into_block_with_sidecar → seal_slow`
    /// preserves the block hash. If this fails, the codec/header path is the
    /// drift source. If it passes (expected), the live BlockHash error must
    /// originate elsewhere (see module docs).
    #[test]
    fn eip8130_block_round_trip_preserves_hash() {
        let txs = vec![aa_envelope()];
        let block = build_block(txs);
        let block_hash_a = block.header.hash_slow();

        let (payload, sidecar) = OpExecutionPayload::from_block_unchecked(block_hash_a, &block);

        // Sanity: V3 path (post-Canyon, pre-Isthmus).
        assert!(matches!(payload, OpExecutionPayload::V3(_)), "expected V3 payload variant");

        // Engine-side reconstruction (the same call ensure_well_formed_payload makes).
        let parsed: Block<OpTxEnvelope> =
            payload.try_into_block_with_sidecar(&sidecar).expect("payload → block");
        let block_hash_b = parsed.header.hash_slow();

        assert_eq!(
            block_hash_a, block_hash_b,
            "block hash drifted across builder→engine round trip — codec path is the drift source"
        );

        // Also prove `transactions_root` survived the recomputation from raw bytes.
        assert_eq!(
            block.header.transactions_root, parsed.header.transactions_root,
            "transactions_root recomputation diverged from builder's value"
        );
    }

    /// Empty-body smoke test — confirms the harness itself is sane (no AA tx,
    /// hashes must match trivially). If this fails, something non-AA is wrong.
    #[test]
    fn empty_block_round_trip_preserves_hash() {
        let block = build_block(vec![]);
        // Empty txs list trie root matches alloy's expected EMPTY_ROOT_HASH.
        assert_eq!(block.header.transactions_root, EMPTY_ROOT_HASH);
        let block_hash_a = block.header.hash_slow();

        let (payload, sidecar) = OpExecutionPayload::from_block_unchecked(block_hash_a, &block);
        let parsed: Block<OpTxEnvelope> =
            payload.try_into_block_with_sidecar(&sidecar).expect("payload → block");

        assert_eq!(block_hash_a, parsed.header.hash_slow());
    }

    /// Direct repro for the e2e `engine::tree: Invalid payload ... block hash
    /// mismatch` log: when the builder produces a header with
    /// `parent_beacon_block_root: Some(_)` but `blob_gas_used: None`,
    /// `OpExecutionPayload::from_block_unchecked` routes through the V3 variant
    /// (parent_beacon_block_root is the V3 selector at line 328 of payload/mod.rs),
    /// and V3's `into_block_raw_with_transactions_root_opt` then rewrites the
    /// reconstructed header with `blob_gas_used = Some(0)` and
    /// `excess_blob_gas = Some(0)`. The hash drifts, the validator emits the
    /// `BlockHash` error, the test still passes downstream because the FCU step
    /// runs against the locally-built block.
    ///
    /// AA is incidental — the same drift would surface for any tx type. Driving
    /// this with an empty body keeps the repro minimal and AA-independent.
    #[test]
    fn v3_payload_drops_blob_gas_when_builder_sets_none() {
        let mut block = build_block(vec![]);
        // The builder-side mismatch we observe in the e2e: parent_beacon_block_root
        // is plumbed through from payload attributes, but Cancun-side blob-gas fields
        // are left None because the chain spec's Ecotone activation is past this
        // block's timestamp.
        block.header.blob_gas_used = None;
        block.header.excess_blob_gas = None;

        let block_hash_a = block.header.hash_slow();

        let (payload, sidecar) = OpExecutionPayload::from_block_unchecked(block_hash_a, &block);
        assert!(matches!(payload, OpExecutionPayload::V3(_)), "V3 selection requires the bug");

        let parsed: Block<OpTxEnvelope> =
            payload.try_into_block_with_sidecar(&sidecar).expect("payload → block");

        // The V3 round-trip "fixes up" the reconstructed header to Some(0) — that's
        // the precise asymmetry that breaks the hash.
        assert_eq!(parsed.header.blob_gas_used, Some(0));
        assert_eq!(parsed.header.excess_blob_gas, Some(0));

        // And so the hash drifts. This is the same condition the engine validator
        // hits at runtime; pinning it as `assert_ne!` keeps the test red until the
        // call-site config (or upstream V3 plumbing) is corrected — at which point
        // flip the assertion.
        assert_ne!(
            block_hash_a,
            parsed.header.hash_slow(),
            "blob-gas drift no longer reproduces — flip this assertion to assert_eq!",
        );
    }
}
