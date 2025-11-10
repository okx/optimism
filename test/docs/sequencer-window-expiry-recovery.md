# Sequencer Window Expiry Recovery Test: Technical Deep Dive

## Overview

This document provides a detailed technical analysis of a test scenario that demonstrates the sequencer window expiry recovery mechanism in Optimism. The test simulates a scenario where `op-batcher` is down for an extended period, causing the sequencer window to expire, and then explores the recovery process.

## Test Flow Summary

1. **Start mock L1** - Initialize a local L1 chain
2. **Start op-seq without op-batcher** - Sequencer produces blocks (unsafe head advances), but no batches are submitted to L1, so safe head remains unchanged
3. **Sequencer window expires** - When the gap between unsafe and safe head exceeds `SeqWindowSize`, the sequencer window expires, and safe head starts advancing via empty batch derivation
4. **Start op-batcher** - Batcher submits batches, but they are rejected due to sequence window expiry
5. **Restart op-seq** - System resets and recovers by finding valid batches from earlier L1 blocks
6. **Start op-rpc** - RPC node syncs and may fork from sequencer

---

## Phase 1: Initial Setup - Mock L1

The test starts by initializing a mock L1 chain. This is a standard setup step and doesn't require detailed code analysis.

---

## Phase 2: Sequencer Window Expiry - No Batcher Running

### What Happens

When `op-seq` and `op-geth-seq` are started but `op-batcher` is not running:
- The sequencer continues to produce L2 blocks (unsafe head advances)
- No batches are submitted to L1
- Safe head remains unchanged (no new batches to derive from)
- The gap between unsafe and safe head keeps growing
- **When the gap exceeds `SeqWindowSize`**: The sequencer window expires, and `op-node` starts deriving empty batches to advance the safe head

### Code Analysis: Empty Batch Derivation

When the sequencer window expires, `op-node` automatically derives empty batches to advance the safe head. This logic is implemented in `op-node/rollup/derive/base_batch_stage.go`:

```162:206:op-node/rollup/derive/base_batch_stage.go
// deriveNextEmptyBatch may derive an empty batch if the sequencing window is expired
func (bs *baseBatchStage) deriveNextEmptyBatch(ctx context.Context, outOfData bool, parent eth.L2BlockRef) (*SingularBatch, error) {
	epoch := bs.l1Blocks[0]
	// If the current epoch is too old compared to the L1 block we are at,
	// i.e. if the sequence window expired, we create empty batches for the current epoch
	expiryEpoch := epoch.Number + bs.config.SeqWindowSize
	forceEmptyBatches := (expiryEpoch == bs.origin.Number && outOfData) || expiryEpoch < bs.origin.Number
	firstOfEpoch := epoch.Number == parent.L1Origin.Number+1
	nextTimestamp := parent.Time + bs.config.BlockTime

	bs.log.Trace("Potentially generating an empty batch",
		"expiryEpoch", expiryEpoch, "forceEmptyBatches", forceEmptyBatches, "nextTimestamp", nextTimestamp,
		"epoch_time", epoch.Time, "len_l1_blocks", len(bs.l1Blocks), "firstOfEpoch", firstOfEpoch)

	if !forceEmptyBatches {
		// sequence window did not expire yet, still room to receive batches for the current epoch,
		// no need to force-create empty batch(es) towards the next epoch yet.
		return nil, io.EOF
	}
	if len(bs.l1Blocks) < 2 {
		// need next L1 block to proceed towards
		return nil, io.EOF
	}

	nextEpoch := bs.l1Blocks[1]
	// Fill with empty L2 blocks of the same epoch until we meet the time of the next L1 origin,
	// to preserve that L2 time >= L1 time. If this is the first block of the epoch, always generate a
	// batch to ensure that we at least have one batch per epoch.
	if nextTimestamp < nextEpoch.Time || firstOfEpoch {
		bs.log.Info("Generating next batch", "epoch", epoch, "timestamp", nextTimestamp, "parent", parent)
		return &SingularBatch{
			ParentHash:   parent.Hash,
			EpochNum:     rollup.Epoch(epoch.Number),
			EpochHash:    epoch.Hash,
			Timestamp:    nextTimestamp,
			Transactions: nil,
		}, nil
	}

	// At this point we have auto generated every batch for the current epoch
	// that we can, so we can advance to the next epoch.
	bs.log.Trace("Advancing internal L1 blocks", "next_timestamp", nextTimestamp, "next_epoch_time", nextEpoch.Time)
	bs.l1Blocks = bs.l1Blocks[1:]
	return nil, io.EOF
}
```

**Key Points:**
- `forceEmptyBatches` is true when `expiryEpoch <= bs.origin.Number`, meaning the current epoch's sequence window has closed
- Empty batches are generated one at a time (each `SingularBatch` contains one L2 block) until the L2 timestamp catches up to the next L1 epoch's time
- The condition `nextTimestamp < nextEpoch.Time || firstOfEpoch` ensures batches are generated until `nextTimestamp >= nextEpoch.Time`

**Why 12 Empty Blocks Are Generated Together?**

In the test environment, L1 block time is 2 seconds and L2 block time is 1 second. According to the code logic, each L1 epoch should generate 2 empty L2 blocks (since `nextEpoch.Time - epoch.Time = 2 seconds`). However, the system generates 12 empty blocks together. This requires further investigation into the actual behavior.

Looking at the code in `deriveNextEmptyBatch`:

```186:199:op-node/rollup/derive/base_batch_stage.go
	nextEpoch := bs.l1Blocks[1]
	// Fill with empty L2 blocks of the same epoch until we meet the time of the next L1 origin,
	// to preserve that L2 time >= L1 time. If this is the first block of the epoch, always generate a
	// batch to ensure that we at least have one batch per epoch.
	if nextTimestamp < nextEpoch.Time || firstOfEpoch {
		bs.log.Info("Generating next batch", "epoch", epoch, "timestamp", nextTimestamp, "parent", parent)
		return &SingularBatch{
			ParentHash:   parent.Hash,
			EpochNum:     rollup.Epoch(epoch.Number),
			EpochHash:    epoch.Hash,
			Timestamp:    nextTimestamp,
			Transactions: nil,
		}, nil
	}
```

The function generates empty batches in a loop:
1. Each iteration: `nextTimestamp = parent.Time + cfg.BlockTime` (increments by L2 block time, which is 1 second)
2. Condition check: `nextTimestamp < nextEpoch.Time || firstOfEpoch`
3. If true: generate one empty batch (one L2 block) and return
4. The function is called again with the updated `parent` (the newly generated L2 block)
5. This continues until `nextTimestamp >= nextEpoch.Time`, at which point the epoch advances

**The Key**: The number of empty blocks generated per epoch depends on `nextEpoch.Time - epoch.Time`, which is the **actual timestamp difference** between consecutive L1 blocks.

If L1 blocks have a 2-second timestamp difference:
- `epoch.Time = T`
- `nextEpoch.Time = T + 2`
- L2 blocks should be generated with timestamps: `T, T+1` (2 blocks)
- When `nextTimestamp = T + 2 >= nextEpoch.Time`, the epoch advances

**Why 12 blocks are observed**: Even though L1 blocks have a 2-second timestamp difference, the observation of 12 empty blocks being generated together requires further investigation. Possible explanations:

1. **Fast derivation loop**: When the sequencer window expires, the derivation pipeline may rapidly process multiple epochs in quick succession. If 6 epochs are processed together, each generating 2 blocks, this would result in 12 blocks (6 × 2 = 12).

2. **First-of-epoch behavior**: The `firstOfEpoch` condition in the code ensures at least one batch per epoch. Combined with rapid epoch processing, this might contribute to the observed pattern.

The exact mechanism requires further investigation through log analysis to understand when and why 12 blocks are generated together, and whether this is a consistent pattern or specific to certain conditions.

### Test Script Logic

The test waits for the safe height to exceed a target value:

```94:111:test/4-op-start-service.sh
TARGET_SAFE_HEIGHT=8593921
EXPECTED_WAIT_TIME=200
START_TIME=$(date +%s)
echo "⏳ Waiting for sequencer window expired and safe height to exceed $TARGET_SAFE_HEIGHT... (expected wait time: ~${EXPECTED_WAIT_TIME}s)"
while true; do
    CURRENT_SAFE=$(cast bn -r http://localhost:8123 safe 2>/dev/null || echo "0")
    if [ "$CURRENT_SAFE" -gt "$TARGET_SAFE_HEIGHT" ]; then
        echo "✅ Safe height reached: $CURRENT_SAFE (target: $TARGET_SAFE_HEIGHT)"
        break
    fi
    ELAPSED_TIME=$(($(date +%s) - START_TIME))
    REMAINING_TIME=$((EXPECTED_WAIT_TIME - ELAPSED_TIME))
    if [ "$REMAINING_TIME" -lt 0 ]; then
        REMAINING_TIME=0
    fi
    echo "   Current safe height: $CURRENT_SAFE, waiting for safe height > $TARGET_SAFE_HEIGHT... (elapsed: ${ELAPSED_TIME}s, remaining: ~${REMAINING_TIME}s)"
    sleep 5
done
```

---

## Phase 3: Batcher Starts - Batch Rejection Cascade

### What Happens

When `op-batcher` starts after the sequencer window has expired:
1. **First batch is rejected** - The batch contains blocks with L1 origins that are too old (exceeding `SeqWindowSize`)
2. **Subsequent batches are rejected** - Because the first batch was dropped, safe head didn't advance, causing timestamp mismatches
3. **System deadlocks** - `op-seq` and `op-batcher` cannot coordinate properly

### Code Analysis: Sequence Window Expiry Check

The sequence window expiry check is performed in `op-node/rollup/derive/batches.go`:

```268:272:op-node/rollup/derive/batches.go
	// Filter out batches that were included too late.
	if startEpochNum+cfg.SeqWindowSize < l1InclusionBlock.Number {
		log.Warn("batch was included too late, sequence window expired")
		return BatchDrop, parentBlock
	}
```

**Why the first batch fails:**
- The batch's `startEpochNum` (L1 origin of the first block in the batch) is old
- The batch was included in L1 block `l1InclusionBlock.Number`
- If `startEpochNum + SeqWindowSize < l1InclusionBlock.Number`, the batch is dropped

### Code Analysis: Future Batch Check

After the first batch is dropped, subsequent batches fail the "future batch" check:

```225:232:op-node/rollup/derive/batches.go
	if batch.GetTimestamp() > nextTimestamp {
		if cfg.IsHolocene(l1InclusionBlock.Time) {
			log.Warn("dropping future span batch", "next_timestamp", nextTimestamp)
			return BatchDrop, eth.L2BlockRef{}
		}
		log.Trace("received out-of-order batch for future processing after next batch", "next_timestamp", nextTimestamp)
		return BatchFuture, eth.L2BlockRef{}
	}
```

**Why subsequent batches fail:**
- `nextTimestamp = l2SafeHead.Time + cfg.BlockTime` (the expected timestamp of the next block)
- Because the first batch was dropped, `l2SafeHead` didn't advance
- The second batch's first block has a timestamp that is ahead of `nextTimestamp`
- This causes the batch to be rejected as a "future batch"

### The Cascade Effect

1. **Batch 1**: Contains blocks from L1 origin 6584, included in L1 block 6789
   - Check: `6584 + 200 < 6789` → **Dropped** (sequence window expired)
   - Safe head remains at old position

2. **Batch 2**: Contains blocks starting from where Batch 1 ended
   - Check: `batch.GetTimestamp() > nextTimestamp` → **Dropped** (future batch)
   - Safe head still hasn't advanced

3. **Batch 3+**: All subsequent batches fail the same future batch check

### Test Script Logic

The test waits for the "decoded" keyword in logs, indicating a batch was processed:

```115:124:test/4-op-start-service.sh
# Wait for "decoded" keyword in op-seq logs
echo "⏳ Waiting for 'decoded' keyword in op-seq logs..."
while true; do
    if docker logs op-seq 2>&1 | grep -q "decoded"; then
        echo "✅ Found 'decoded' keyword in op-seq logs"
        break
    fi
    echo "   Waiting for 'decoded' keyword in op-seq logs..."
    sleep 5
done
```

However, in this scenario, batches are being dropped, so the system cannot recover without intervention.

---

## Phase 4: Sequencer Restart - Recovery Mechanism

### What Happens

Restarting `op-seq` triggers a reset process:
1. **FindL2Heads** - Recalculates safe/unsafe heads by walking back the L2 chain
2. **Initial Reset** - Rewinds L1 traversal to an earlier point to start buffering channel data
3. **Batch Processing** - Processes batches from earlier L1 blocks, skipping invalid ones
4. **Recovery** - Eventually finds valid batches and resumes normal operation

### Code Analysis: FindL2Heads on Startup

When `op-node` starts, it calls `FindL2Heads` to determine the safe and unsafe heads:

```103:261:op-node/rollup/sync/start.go
// FindL2Heads walks back from `start` (the previous unsafe L2 block) and finds
// the finalized, unsafe and safe L2 blocks.
//
//   - The *unsafe L2 block*: This is the highest L2 block whose L1 origin is a *plausible*
//     extension of the canonical L1 chain (as known to the op-node).
//   - The *safe L2 block*: This is the highest L2 block whose epoch's sequencing window is
//     complete within the canonical L1 chain (as known to the op-node).
//   - The *finalized L2 block*: This is the L2 block which is known to be fully derived from
//     finalized L1 block data.
//
// Plausible: meaning that the blockhash of the L2 block's L1 origin
// (as reported in the L1 Attributes deposit within the L2 block) is not canonical at another height in the L1 chain,
// and the same holds for all its ancestors.
func FindL2Heads(ctx context.Context, cfg *rollup.Config, l1 L1Chain, l2 L2Chain, lgr log.Logger, syncCfg *Config) (result *FindHeadsResult, err error) {
	// Fetch current L2 forkchoice state
	result, err = currentHeads(ctx, cfg, l2)
	if err != nil {
		return nil, fmt.Errorf("failed to fetch current L2 forkchoice state: %w", err)
	}

	lgr.Info("Loaded current L2 heads", "unsafe", result.Unsafe, "safe", result.Safe, "finalized", result.Finalized,
		"unsafe_origin", result.Unsafe.L1Origin, "safe_origin", result.Safe.L1Origin)

	// ... validation checks ...

	// Current L2 block.
	n := result.Unsafe

	var highestL2WithCanonicalL1Origin eth.L2BlockRef // the highest L2 block with confirmed canonical L1 origin
	var l1Block eth.L1BlockRef                        // the L1 block at the height of the L1 origin of the current L2 block n.
	var ahead bool                                    // when "n", the L2 block, has a L1 origin that is not visible in our L1 chain source yet

	ready := false // when we found the block after the safe head, and we just need to return the parent block.
	bOff := retry.Exponential()

	// Each loop iteration we traverse further from the unsafe head towards the finalized head.
	// Once we pass the previous safe head and we have seen at least a full sequence window worth of L1 blocks to confirm,
	// then we return the last L2 block of the epoch before that as safe head.
	// Each loop iteration we traverse a single L2 block, and we check if the L1 origins are consistent.
	for {
		// Fetch L1 information if we never had it, or if we do not have it for the current origin.
		// Optimization: as soon as we have a previous L1 block, try to traverse L1 by hash instead of by number, to fill the cache.
		if n.L1Origin.Hash == l1Block.ParentHash {
			b, err := retry.Do(ctx, 5, bOff, func() (eth.L1BlockRef, error) { return l1.L1BlockRefByHash(ctx, n.L1Origin.Hash) })
			if err != nil {
				// Exit, find-sync start should start over, to move to an available L1 chain with block-by-number / not-found case.
				return nil, fmt.Errorf("failed to retrieve L1 block: %w", err)
			}
			lgr.Info("Walking back L1Block by hash", "curr", l1Block, "next", b, "l2block", n)
			l1Block = b
			ahead = false
		} else if l1Block == (eth.L1BlockRef{}) || n.L1Origin.Hash != l1Block.Hash {
			b, err := retry.Do(ctx, 5, bOff, func() (eth.L1BlockRef, error) { return l1.L1BlockRefByNumber(ctx, n.L1Origin.Number) })
			// if L2 is ahead of L1 view, then consider it a "plausible" head
			notFound := errors.Is(err, ethereum.NotFound)
			if err != nil && !notFound {
				return nil, fmt.Errorf("failed to retrieve block %d from L1 for comparison against %s: %w", n.L1Origin.Number, n.L1Origin.Hash, err)
			}
			l1Block = b
			ahead = notFound
			lgr.Info("Walking back L1Block by number", "curr", l1Block, "next", b, "l2block", n)
		}

		// ... validation and safe head determination logic ...

		// If the L2 block is at least as old as the previous safe head, and we have seen at least a full sequence window worth of L1 blocks to confirm
		if n.Number <= result.Safe.Number && n.L1Origin.Number+cfg.SyncLookback() < highestL2WithCanonicalL1Origin.L1Origin.Number && n.SequenceNumber == 0 {
			ready = true
		}

		// ... continue traversal ...
	}
}
```

**Key Points:**
- `FindL2Heads` walks back from the unsafe head, verifying each L2 block's L1 origin against the canonical L1 chain
- The safe head is determined as the highest L2 block whose L1 origin's sequence window has closed
- This process is logged as "Walking back L1Block" in the logs

### Code Analysis: Initial Reset

After determining the safe head, the pipeline performs an initial reset:

```228:265:op-node/rollup/derive/pipeline.go
// initialReset does the initial reset work of finding the L1 point to rewind back to
func (dp *DerivationPipeline) initialReset(ctx context.Context, resetL2Safe eth.L2BlockRef) error {
	dp.log.Info("Rewinding derivation-pipeline L1 traversal to handle reset")

	dp.metrics.RecordPipelineReset()
	spec := rollup.NewChainSpec(dp.rollupCfg)

	// Walk back L2 chain to find the L1 origin that is old enough to start buffering channel data from.
	pipelineL2 := resetL2Safe
	l1Origin := resetL2Safe.L1Origin

	pipelineOrigin, err := dp.l1Fetcher.L1BlockRefByHash(ctx, l1Origin.Hash)
	if err != nil {
		return NewTemporaryError(fmt.Errorf("failed to fetch the new L1 progress: origin: %s; err: %w", pipelineL2.L1Origin, err))
	}

	for {
		afterL2Genesis := pipelineL2.Number > dp.rollupCfg.Genesis.L2.Number
		afterL1Genesis := pipelineL2.L1Origin.Number > dp.rollupCfg.Genesis.L1.Number
		afterChannelTimeout := pipelineL2.L1Origin.Number+spec.ChannelTimeout(pipelineOrigin.Time) > l1Origin.Number
		if afterL2Genesis && afterL1Genesis && afterChannelTimeout {
			parent, err := dp.l2.L2BlockRefByHash(ctx, pipelineL2.ParentHash)
			if err != nil {
				return NewResetError(fmt.Errorf("failed to fetch L2 parent block %s", pipelineL2.L2BlockID()))
			}
			pipelineL2 = parent
			pipelineOrigin, err = dp.l1Fetcher.L1BlockRefByHash(ctx, pipelineL2.L1Origin.Hash)
			if err != nil {
				return NewTemporaryError(fmt.Errorf("failed to fetch the new L1 progress: origin: %s; err: %w", pipelineL2.L1Origin, err))
			}
		} else {
			break
		}
	}

	sysCfg, err := dp.l2.SystemConfigByL2Hash(ctx, pipelineL2.Hash)
	if err != nil {
		return NewTemporaryError(fmt.Errorf("failed to fetch L1 config of L2 block %s: %w", pipelineL2.L2BlockID(), err))
	}

	dp.origin = pipelineOrigin
	dp.resetSysConfig = sysCfg
	dp.resetL2Safe = resetL2Safe
	return nil
}
```

**Key Points:**
- The reset walks back the L2 chain to find an L1 origin that is old enough (considering `ChannelTimeout`) to start buffering channel data from
- This ensures the pipeline can read all necessary L2 data from L1 to construct batches after the safe head
- The `dp.origin` is set to this earlier L1 block, which becomes the starting point for L1 traversal

### Code Analysis: Batch Processing During Recovery

During recovery, the pipeline processes batches from L1 blocks, skipping invalid ones:

```233:239:op-node/rollup/derive/batches.go
	if batch.GetBlockTimestamp(batch.GetBlockCount()-1) < nextTimestamp {
		log.Warn("span batch has no new blocks after safe head")
		if cfg.IsHolocene(l1InclusionBlock.Time) {
			return BatchPast, eth.L2BlockRef{}
		}
		return BatchDrop, eth.L2BlockRef{}
	}
```

**Recovery Process:**
1. Pipeline starts from an earlier L1 block (determined by `initialReset`)
2. As it advances through L1 blocks, it checks each block for batch data
3. Invalid batches (too old, future, etc.) are dropped with warnings
4. Eventually, a valid batch is found that can be applied to the current safe head
5. Once a valid batch is processed, the system resumes normal operation

### Test Script Logic

The test waits for `unsafe - safe < 200` and periodically restarts the sequencer:

```128:139:test/4-op-start-service.sh
# Wait for unsafe - safe < (seq window size * L1 blocktime)
echo "⏳ Waiting for unsafe - safe < 200..."
while true; do
    CURRENT_SAFE=$(cast bn -r http://localhost:8123 safe 2>/dev/null || echo "0")
    CURRENT_UNSAFE=$(cast bn -r http://localhost:8123 2>/dev/null || echo "0")
    if [ "$CURRENT_SAFE" != "0" ] && [ "$CURRENT_UNSAFE" != "0" ] && [ $((CURRENT_UNSAFE - CURRENT_SAFE)) -lt 200 ]; then
        echo "✅ Unsafe - safe < 200: unsafe=$CURRENT_UNSAFE, safe=$CURRENT_SAFE"
        break
    fi
    $SCRIPTS_DIR/restart-op-seq.sh
    sleep 5
done
```

The restart allows the system to reset and find valid batches from earlier L1 blocks.

---

## Phase 5: RPC Node Startup - Fork Detection

### What Happens

When `op-rpc` starts after the sequencer has recovered:
- RPC node quickly syncs by deriving blocks from L1
- At some point, the RPC node and sequencer may derive different blocks at the same height
- This causes a fork between the two nodes

### Potential Causes of Fork

1. **Different L1 Traversal Origins**: The RPC node's `initialReset` may determine a different L1 traversal origin than the sequencer
2. **Different Safe Heads at Startup**: If the RPC node starts with a different safe head, it may derive different empty batches
3. **Timing Differences**: The RPC node may see different L1 blocks or batches at different times

### Code Analysis: L1 Traversal Origin Determination

The L1 traversal origin is determined by `initialReset`, which walks back considering `ChannelTimeout`:

```244:261:op-node/rollup/derive/pipeline.go
	for {
		afterL2Genesis := pipelineL2.Number > dp.rollupCfg.Genesis.L2.Number
		afterL1Genesis := pipelineL2.L1Origin.Number > dp.rollupCfg.Genesis.L1.Number
		afterChannelTimeout := pipelineL2.L1Origin.Number+spec.ChannelTimeout(pipelineOrigin.Time) > l1Origin.Number
		if afterL2Genesis && afterL1Genesis && afterChannelTimeout {
			parent, err := dp.l2.L2BlockRefByHash(ctx, pipelineL2.ParentHash)
			if err != nil {
				return NewResetError(fmt.Errorf("failed to fetch L2 parent block %s", pipelineL2.L2BlockID()))
			}
			pipelineL2 = parent
			pipelineOrigin, err = dp.l1Fetcher.L1BlockRefByHash(ctx, pipelineL2.L1Origin.Hash)
			if err != nil {
				return NewTemporaryError(fmt.Errorf("failed to fetch the new L1 progress: origin: %s; err: %w", pipelineL2.L1Origin, err))
			}
		} else {
			break
		}
	}
```

**Why forks can occur:**
- If the RPC node's safe head at startup differs from the sequencer's safe head, `initialReset` may walk back to a different L1 origin
- This different origin leads to different `bs.l1Blocks[0]` values in `deriveNextEmptyBatch`
- Different epochs result in different empty batch generation, causing a fork

### Code Analysis: Empty Batch Epoch Selection

Empty batches use `bs.l1Blocks[0]` as the epoch:

```162:189:op-node/rollup/derive/base_batch_stage.go
// deriveNextEmptyBatch may derive an empty batch if the sequencing window is expired
func (bs *baseBatchStage) deriveNextEmptyBatch(ctx context.Context, outOfData bool, parent eth.L2BlockRef) (*SingularBatch, error) {
	epoch := bs.l1Blocks[0]
	// If the current epoch is too old compared to the L1 block we are at,
	// i.e. if the sequence window expired, we create empty batches for the current epoch
	expiryEpoch := epoch.Number + bs.config.SeqWindowSize
	forceEmptyBatches := (expiryEpoch == bs.origin.Number && outOfData) || expiryEpoch < bs.origin.Number
	firstOfEpoch := epoch.Number == parent.L1Origin.Number+1
	nextTimestamp := parent.Time + bs.config.BlockTime

	bs.log.Trace("Potentially generating an empty batch",
		"expiryEpoch", expiryEpoch, "forceEmptyBatches", forceEmptyBatches, "nextTimestamp", nextTimestamp,
		"epoch_time", epoch.Time, "len_l1_blocks", len(bs.l1Blocks), "firstOfEpoch", firstOfEpoch)

	if !forceEmptyBatches {
		// sequence window did not expire yet, still room to receive batches for the current epoch,
		// no need to force-create empty batch(es) towards the next epoch yet.
		return nil, io.EOF
	}
	if len(bs.l1Blocks) < 2 {
		// need next L1 block to proceed towards
		return nil, io.EOF
	}

	nextEpoch := bs.l1Blocks[1]
	// Fill with empty L2 blocks of the same epoch until we meet the time of the next L1 origin,
	// to preserve that L2 time >= L1 time. If this is the first block of the epoch, always generate a
	// batch to ensure that we at least have one batch per epoch.
	if nextTimestamp < nextEpoch.Time || firstOfEpoch {
		bs.log.Info("Generating next batch", "epoch", epoch, "timestamp", nextTimestamp, "parent", parent)
		return &SingularBatch{
			ParentHash:   parent.Hash,
			EpochNum:     rollup.Epoch(epoch.Number),
			EpochHash:    epoch.Hash,
			Timestamp:    nextTimestamp,
			Transactions: nil,
		}, nil
	}
```

**Fork Mechanism:**
- If `bs.l1Blocks[0]` differs between sequencer and RPC, they will generate empty batches with different `EpochNum` and `EpochHash`
- This causes blocks at the same L2 height to have different hashes, resulting in a fork

### Test Script Logic

The test checks for forks at the safe height:

```143:159:test/4-op-start-service.sh
# Wait for op-rpc to be ready
echo "⏳ Waiting for op-rpc to be ready..."
while true; do
    SAFE_8124=$(cast bn -r http://localhost:8124 safe 2>/dev/null || echo "0")
    if [ "$SAFE_8124" != "0" ]; then
        # Check for fork at safe height
        if ! $SCRIPTS_DIR/check-fork.sh "$SAFE_8124" 2>/dev/null; then
            echo "❌ Fork detected at safe height $SAFE_8124, breaking loop"
            break
        fi
    fi
    echo "   Waiting for op-rpc to be ready..."
    sleep 5
done

$SCRIPTS_DIR/find-fork.sh
```

The `find-fork.sh` script performs a binary search to locate the exact fork point.

---

## Summary

This test demonstrates a critical recovery scenario in Optimism:

1. **Sequencer Window Expiry**: When the batcher is down, the sequencer window expires, and safe head advances via empty batch derivation
2. **Batch Rejection Cascade**: When the batcher resumes, batches are rejected due to sequence window expiry and future batch checks
3. **Recovery via Reset**: Restarting the sequencer triggers a reset that finds valid batches from earlier L1 blocks
4. **Fork Potential**: Different nodes may derive different blocks due to different L1 traversal origins, causing forks

The recovery mechanism relies on the `initialReset` function walking back the L2 chain to find an appropriate L1 origin to start from, allowing the system to skip invalid batches and eventually find valid ones.

