//! XLayer-specific batch-derivation tests.
//!
//! The XLayer-fork tx-type gates themselves (currently: EIP-8130 pre-XLayerV1)
//! are inlined next to the upstream EIP-7702/isthmus gate in
//! [`crate::SingleBatch::check_batch`] and [`crate::SpanBatch::check_batch`] —
//! one-line `is_<fork>_active(timestamp) && tx[0] == <type>` shape, matching
//! the existing pattern. This file holds the cross-batch-type tests in
//! `mod xlayer_tests` so both batch types are exercised pre- and post-fork in
//! one place.
//!
//! See [`base/crates/consensus/protocol/src/batch/{single,span}.
//! rs::test_check_batch_with_eip8130_tx_post_base_v1`] for the reference impl
//! this mirrors (their fork is named `base_v1`; ours is `xlayer_v1`).
//!
//! # Adding a new XLayer-fork tx-type gate
//!
//! 1. Add a `Eip<NEW>Pre<Fork>` variant to [`crate::BatchDropReason`].
//! 2. Add a `is_<fork>_active(timestamp)` method to [`kona_genesis::RollupConfig`].
//! 3. Inline the gate in `SingleBatch::check_batch` and `SpanBatch::check_batch`, mirroring the
//!    existing `Eip8130 / xlayer_v1` block.
//! 4. Add coverage to `mod xlayer_tests` below.

#[cfg(test)]
mod xlayer_tests {
    //! XLayer-specific batch-derivation gating tests.
    //!
    //! Mirrors `base/crates/consensus/protocol/src/batch/{single,span}.
    //! rs::test_check_batch_with_eip8130_tx_*_base_v1` 1:1 against XLayer's `xlayer_v1_time`
    //! schedule. Both batch types are exercised pre- and post-fork.

    use crate::{
        BatchDropReason, BatchValidity, BlockInfo, L2BlockInfo, SingleBatch, SpanBatch,
        SpanBatchElement, SpanBatchTransactions, test_utils::TestBatchValidator,
    };
    use alloc::{vec, vec::Vec};
    use alloy_eips::BlockNumHash;
    use alloy_primitives::{B256, BlockHash, Bytes, FixedBytes, b256};
    use kona_genesis::{HardForkConfig, RollupConfig};
    use op_alloy_consensus::OpTxType;

    /// Builds a minimal `BlockInfo` list for span-batch tests.
    fn gen_l1_blocks(start_num: u64, count: u64, start_ts: u64, interval: u64) -> Vec<BlockInfo> {
        (0..count)
            .map(|i| BlockInfo {
                number: start_num + i,
                timestamp: start_ts + i * interval,
                hash: B256::left_padding_from(&i.to_be_bytes()),
                ..Default::default()
            })
            .collect()
    }

    /// A 1-byte placeholder "transaction" — just the EIP-8130 type byte. Sufficient
    /// for the gate check, which inspects only `tx[0]`.
    fn aa_type_byte_tx() -> Bytes {
        Bytes::copy_from_slice(&[OpTxType::Eip8130 as u8])
    }

    // ----------- SingleBatch ------------------------------------------------

    #[test]
    fn test_check_batch_drop_8130_pre_xlayer_v1() {
        // XLayerV1 not scheduled => Drop.
        let cfg = RollupConfig { max_sequencer_drift: 1, ..Default::default() };
        let single_batch = SingleBatch {
            parent_hash: BlockHash::ZERO,
            epoch_num: 1,
            epoch_hash: BlockHash::ZERO,
            timestamp: 1,
            transactions: vec![aa_type_byte_tx()],
        };
        let l1_blocks = vec![BlockInfo::default(), BlockInfo::default()];
        let l2_safe_head = L2BlockInfo {
            block_info: BlockInfo { timestamp: 1, ..Default::default() },
            ..Default::default()
        };
        let inclusion_block = BlockInfo::default();
        assert_eq!(
            single_batch.check_batch(&cfg, &l1_blocks, l2_safe_head, &inclusion_block),
            BatchValidity::Drop(BatchDropReason::Eip8130PreXLayerV1)
        );
    }

    #[test]
    fn test_check_batch_accept_8130_post_xlayer_v1() {
        // XLayerV1 active at genesis (timestamp 0) => Accept.
        let cfg = RollupConfig {
            max_sequencer_drift: 1,
            hardforks: HardForkConfig { xlayer_v1_time: Some(0), ..Default::default() },
            ..Default::default()
        };
        let single_batch = SingleBatch {
            parent_hash: BlockHash::ZERO,
            epoch_num: 1,
            epoch_hash: BlockHash::ZERO,
            timestamp: 1,
            transactions: vec![aa_type_byte_tx()],
        };
        let l1_blocks = vec![BlockInfo::default(), BlockInfo::default()];
        let l2_safe_head = L2BlockInfo {
            block_info: BlockInfo { timestamp: 1, ..Default::default() },
            ..Default::default()
        };
        let inclusion_block = BlockInfo::default();
        assert_eq!(
            single_batch.check_batch(&cfg, &l1_blocks, l2_safe_head, &inclusion_block),
            BatchValidity::Accept
        );
    }

    // ----------- SpanBatch --------------------------------------------------
    //
    // The span-batch fixture: three SpanBatchElements (filler / AA /
    // filler) so the AA gate hits at the second element and surfaces a Drop. The
    // filler txs (EIP-1559 type byte) keep the rest of the batch shape valid.

    #[tokio::test]
    async fn test_check_batch_with_eip8130_tx_pre_xlayer_v1() {
        let cfg = RollupConfig {
            seq_window_size: 100,
            max_sequencer_drift: 100,
            hardforks: HardForkConfig { delta_time: Some(0), ..Default::default() },
            block_time: 10,
            ..Default::default()
        };
        let l1_blocks = gen_l1_blocks(9, 3, 0, 10);
        let parent_hash = b256!("1111111111111111111111111111111111111111000000000000000000000000");
        let l2_safe_head = L2BlockInfo {
            block_info: BlockInfo {
                number: 41,
                timestamp: 10,
                hash: parent_hash,
                ..Default::default()
            },
            l1_origin: BlockNumHash { number: 9, ..Default::default() },
            ..Default::default()
        };
        let inclusion_block = BlockInfo { number: 50, ..Default::default() };
        let l2_block = L2BlockInfo {
            block_info: BlockInfo { number: 40, ..Default::default() },
            ..Default::default()
        };
        let mut fetcher = TestBatchValidator { blocks: vec![l2_block], ..Default::default() };

        let filler = Bytes::copy_from_slice(&[alloy_consensus::TxType::Eip1559 as u8]);
        let batch = SpanBatch {
            batches: vec![
                SpanBatchElement {
                    epoch_num: 10,
                    timestamp: 20,
                    transactions: vec![filler.clone()],
                },
                SpanBatchElement {
                    epoch_num: 10,
                    timestamp: 20,
                    transactions: vec![aa_type_byte_tx()],
                },
                SpanBatchElement { epoch_num: 11, timestamp: 20, transactions: vec![filler] },
            ],
            parent_check: FixedBytes::<20>::from_slice(&parent_hash[..20]),
            l1_origin_check: FixedBytes::<20>::from_slice(&l1_blocks[0].hash[..20]),
            txs: SpanBatchTransactions::default(),
            ..Default::default()
        };
        assert_eq!(
            batch.check_batch(&cfg, &l1_blocks, l2_safe_head, &inclusion_block, &mut fetcher).await,
            BatchValidity::Drop(BatchDropReason::Eip8130PreXLayerV1)
        );
    }

    #[tokio::test]
    async fn test_check_batch_with_eip8130_tx_post_xlayer_v1() {
        let cfg = RollupConfig {
            seq_window_size: 100,
            max_sequencer_drift: 100,
            hardforks: HardForkConfig {
                delta_time: Some(0),
                xlayer_v1_time: Some(0),
                ..Default::default()
            },
            block_time: 10,
            ..Default::default()
        };
        let l1_blocks = gen_l1_blocks(9, 3, 0, 10);
        let parent_hash = b256!("1111111111111111111111111111111111111111000000000000000000000000");
        let l2_safe_head = L2BlockInfo {
            block_info: BlockInfo {
                number: 41,
                timestamp: 10,
                hash: parent_hash,
                ..Default::default()
            },
            l1_origin: BlockNumHash { number: 9, ..Default::default() },
            ..Default::default()
        };
        let inclusion_block = BlockInfo { number: 50, ..Default::default() };
        let l2_block = L2BlockInfo {
            block_info: BlockInfo { number: 40, ..Default::default() },
            ..Default::default()
        };
        let mut fetcher = TestBatchValidator { blocks: vec![l2_block], ..Default::default() };

        let filler = Bytes::copy_from_slice(&[alloy_consensus::TxType::Eip1559 as u8]);
        let batch = SpanBatch {
            batches: vec![
                SpanBatchElement {
                    epoch_num: 10,
                    timestamp: 20,
                    transactions: vec![filler.clone()],
                },
                SpanBatchElement {
                    epoch_num: 10,
                    timestamp: 20,
                    transactions: vec![aa_type_byte_tx()],
                },
                SpanBatchElement { epoch_num: 11, timestamp: 20, transactions: vec![filler] },
            ],
            parent_check: FixedBytes::<20>::from_slice(&parent_hash[..20]),
            l1_origin_check: FixedBytes::<20>::from_slice(&l1_blocks[0].hash[..20]),
            txs: SpanBatchTransactions::default(),
            ..Default::default()
        };
        assert_eq!(
            batch.check_batch(&cfg, &l1_blocks, l2_safe_head, &inclusion_block, &mut fetcher).await,
            BatchValidity::Accept
        );
    }
}
