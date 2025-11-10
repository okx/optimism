# Sequencer Restart Fork Analysis

## Problem Description

When `op-seq` is restarted, it causes forks with `op-rpc` nodes. The root cause is related to how `FindL2Heads` recalculates the safe head and how `initialReset` determines the L1 traversal starting point, which always rewinds to a very old L1 block regardless of safe head advancement.

## Key Observations from Logs

### op-seq First Startup (Line 30)
- **Initial State**: `safe=8593921`, `safe_origin=22`
- **Reset Origin**: `origin=22` (line 164)
- **L1 Traversal**: Starts from L1 block 22, advances through 23, 24, 25... until L1 block 131
- **First Batch Found** (line 1558): Found at L1 block 131
  - `stage_origin=131` (batch included in L1 block 131)
  - `start_epoch_number=26 end_epoch_number=66` (batch covers L1 blocks 26-66)
  - `safe num=8593938` (current safe head when batch is checked)
  - **Result**: Batch rejected as "sequence window expired"
    - The check is: `startEpochNum + SeqWindowSize < l1InclusionBlock.Number`
    - With `startEpochNum=26`, `SeqWindowSize=100` (from config), `l1InclusionBlock.Number=131`:
      - `26 + 100 = 126 < 131` → **true** (batch IS expired)
    - The batch is correctly rejected because its L1 origin (26) plus the sequence window size (100) is less than the L1 inclusion block (131), meaning the batch was included too late

### op-rpc Startup (Line 25)
- **Initial State**: `safe=8593921`, `safe_origin=22`
- **Reset Origin**: `origin=22` (line 35)
- **L1 Traversal**: Starts from L1 block 22, advances through 23, 24, 25... until L1 block 131
- **First Batch Found** (line 200): Found at L1 block 131
  - `stage_origin=131` (batch included in L1 block 131)
  - `start_epoch_number=26 end_epoch_number=66` (batch covers L1 blocks 26-66)
  - `safe num=8593938` (current safe head when batch is checked)
  - **Result**: Batch rejected as "sequence window expired" (same as op-seq)

- **Second Batch** (line 1563): Found at L1 block 131
  - `start_epoch_number=67 end_epoch_number=120` (batch covers L1 blocks 67-120)
  - `batch_timestamp=1762751684`, `nextTimestamp=1762751593` (current safe head's next timestamp)
  - **Result**: Batch rejected as "dropping future span batch"
    - The check is: `batch.GetTimestamp() > nextTimestamp`
    - `1762751684 > 1762751593` → **true** (batch timestamp is in the future)
    - This happens because the first batch was dropped, so safe head didn't advance, leaving a gap

- **Third and Subsequent Batches** (lines 1573, 3181, etc.): All found at L1 blocks 131-134
  - All have `start_epoch_number=121` or higher (covering L1 blocks 121+)
  - All have `batch_timestamp` values (1762751792, 1762751793, etc.) greater than `nextTimestamp=1762751593`
  - **Result**: All rejected as "dropping future span batch"
    - Because the first batch was dropped, safe head remains at 8593938
    - All subsequent batches have timestamps that are too far in the future relative to the current safe head
    - This creates a deadlock: batches can't be applied because they're "future", but safe head can't advance because no batches are being applied

- **Empty Batch Generation** (line 1424): After all batches are dropped
  - System starts generating empty batches via `deriveNextEmptyBatch`
  - `epoch=22`, `timestamp=1762751576` (matches the expected next timestamp)
  - Safe head advances through empty batch derivation (lines 1426, 1432, etc.)
  - This allows the system to eventually catch up, but causes forks because `op-rpc` continues processing from its position without restarting

### op-rpc Processing (No Restart)
- **op-rpc** starts at the same time as `op-seq` first startup (line 25)
- **Initial State**: `safe=8593921`, `safe_origin=22`
- **Reset Origin**: `origin=22` (line 35)
- **L1 Traversal**: Starts from L1 block 22, advances through 23, 24, 25...
- **Batch Processing**: `op-rpc` processes batches continuously without restarting
  - First batch found at L1 block 131 (line 200): Same as `op-seq`, rejected as "sequence window expired"
  - Second batch found at L1 block 131 (line 205): Same as `op-seq`, rejected as "dropping future span batch"
  - **Key Difference**: `op-rpc` continues processing from L1 block 22 onwards, while `op-seq` restarts and rewinds to L1 block 47

## Root Cause Analysis

### 1. FindL2Heads Logic

When `op-seq` restarts, `FindL2Heads` is called to determine the safe head:

```go
// From op-node/rollup/sync/start.go:237
if n.Number <= result.Safe.Number &&
   n.L1Origin.Number+cfg.SyncLookback() < highestL2WithCanonicalL1Origin.L1Origin.Number &&
   n.SequenceNumber == 0 {
    ready = true
}
```

**Behavior on restart:**
When `op-seq` restarts, it calls `FindL2Heads` to determine the safe head. This function performs the following steps:

1. **Read current forkchoice state from Engine** (line 118 in `start.go`):
   The function first queries the execution engine to get the current unsafe, safe, and finalized heads. This initial state is logged as "Loaded current L2 heads".

2. **Walk back and recalculate safe head** (lines 148-287 in `start.go`):
   Starting from the unsafe head, the function walks backward through the L2 chain, verifying each block's L1 origin against the canonical L1 chain. It identifies the first L2 block whose L1 origin is sufficiently old (i.e., its sequence window has closed) and whose sequence number is 0. The parent of this block becomes the recalculated safe head.

The recalculation is necessary because the L1 chain may have advanced during downtime. As a result, more L2 blocks may now satisfy the sequence window requirement, potentially yielding a higher safe head than what was stored in the Engine.

**From the logs (op-seq restart, line 5160):**
- **Initial state (read from Engine):**
  - unsafe: 8594297, L1 origin: 199
  - safe: 8594084, L1 origin: 103
- **After FindL2Heads walk-back (line 5487):**
  - FindL2Heads finds L2 block 8594073 with L1 origin 98, sequence number 0
  - The condition `n.Number <= result.Safe.Number && n.L1Origin.Number+cfg.SyncLookback() < highestL2WithCanonicalL1Origin.L1Origin.Number && n.SequenceNumber == 0` becomes true
  - Safe head is set to parent of this block: 8594072
- **After recalculation (line 5489):**
  - unsafe: 8594297, L1 origin: 199 (unchanged)
  - safe: 8594072, L1 origin: 97 (recalculated)

The walk-back process verified L2 blocks from unsafe head (8594297) down to the recalculated safe head (8594072), confirming that the safe head's L1 origin (97) is sufficiently old relative to the current L1 chain state (199).

### 2. initialReset Logic

After `FindL2Heads` determines the new safe head, `initialReset` is called to determine the L1 traversal starting point:

```go
// From op-node/rollup/derive/pipeline.go:247
afterChannelTimeout := pipelineL2.L1Origin.Number+spec.ChannelTimeout(pipelineOrigin.Time) > l1Origin.Number
```

**Process:**
1. `initialReset` starts from the new safe head (`resetL2Safe`)
2. It walks back the L2 chain while checking: `pipelineL2.L1Origin.Number + ChannelTimeout > safe_head.L1Origin.Number`
3. The walk continues until the condition becomes false or reaches genesis

**The Logic:**
With `ChannelTimeout = 50` (hardcoded `ChannelTimeoutGranite` since `granite_time: 0`), `initialReset` walks back from the recalculated safe head while the following condition is true:
```
pipelineL2.L1Origin.Number + 50 > safe_head.L1Origin.Number
```

The loop stops when this condition becomes false, i.e., when:
```
pipelineL2.L1Origin.Number + 50 <= safe_head.L1Origin.Number
```

**Why ChannelTimeout is needed:**

The reason for rewinding 50 L1 blocks (ChannelTimeout) is to ensure **channel completeness**, even though some batches in those channels may be skipped:

1. **Channel completeness requirement**: A channel can only be read when all its frames are present. The `IsReady()` check requires:
   - The last frame (`isLast=true`) must be seen
   - All frames from frame 0 to `endFrameNumber` must be present
   - Missing any frame prevents the entire channel from being read

2. **Channels span multiple L1 blocks**: A single channel's frames may be distributed across multiple L1 blocks. For example:
   - Frame 0 might be in L1 block 46
   - Frame 1 might be in L1 block 47
   - ...
   - Frame 10 (last frame) might be in L1 block 96

3. **The problem without rewinding**: If we only start reading from the safe head's L1 origin (e.g., L1 block 96), we would:
   - Only read Frame 10 from L1 block 96
   - Miss Frames 0-9 from L1 blocks 46-95
   - Never be able to complete the channel (missing frames)
   - Be unable to decode any batches from that channel, even if they need to be processed

4. **Why read frames even if batches will be skipped**:
   - We cannot determine which batches need processing until the entire channel is decoded
   - A channel may contain a mix of old batches (to be skipped) and new batches (to be processed)
   - The channel must be complete before we can decode and filter batches
   - Without all frames, the channel remains incomplete and unusable

5. **ChannelTimeout = 50**: This represents the maximum number of L1 blocks a channel can span. By rewinding 50 blocks, we ensure we can read all frames of any channel that might still be valid (not timed out).

**From the logs (op-seq restart, line 5513):**
- Recalculated safe head: 8594072, L1 origin: 97
- The condition to continue walking: `pipelineL2.L1Origin.Number + 50 > 97`, i.e., `pipelineL2.L1Origin.Number > 47`
- `initialReset` walks back from L2 block 8594072 until it finds an L2 block with L1 origin <= 47
- It finds an L2 block with L1 origin 47 (47 + 50 = 97 <= 97, condition becomes false)
- Final L1 traversal origin: 47

**The Problem:**
Even though the safe head has advanced to L1 origin 97, `initialReset` rewinds the L1 traversal back to L1 block 47 (50 blocks behind the safe head's L1 origin). This causes the pipeline to re-process batches from L1 blocks 47-97 that were already applied, potentially leading to different batch selection than a continuously running node.

**Comparison with op-rpc:**
- **op-rpc** (line 35): Starts from L1 block 22 (safe head's L1 origin), continues processing without restart
- **op-seq after restart** (line 5513): Rewinds to L1 block 47 (50 blocks behind safe head's L1 origin 97)
- **Result**: `op-seq` re-processes batches from L1 blocks 47-97, while `op-rpc` continues from L1 block 22, causing divergence

### 3. Why This Causes Forks

**Scenario:**
1. **op-seq first startup** (line 30): Safe head at 8593921 with L1 origin 22, starts from L1 block 22
2. **op-rpc startup** (line 25): Safe head at 8593921 with L1 origin 22, starts from L1 block 22 (same as op-seq)
3. **op-seq restart** (line 5160): Safe head recalculated to 8594072 with L1 origin 97, but `initialReset` rewinds to L1 block 47
4. **Problem**: Starting from L1 block 47 (50 blocks behind safe head's L1 origin 97) means the pipeline will:
   - Re-process batches from L1 blocks 47-97 that were already applied
   - Potentially find different batches than what `op-rpc` (which didn't restart) would find
   - This causes a fork

**Why op-rpc doesn't fork:**
- `op-rpc` doesn't restart, so it doesn't go through `FindL2Heads` and `initialReset`
- It continues from where it left off, processing batches in order from L1 block 22
- It doesn't re-process old batches from L1 blocks 47-97
- When `op-seq` restarts and rewinds to L1 block 47, it may process different batches than `op-rpc`, causing divergence

## Code Flow

### FindL2Heads (Startup)
```
1. Load current L2 heads from Engine
2. Walk back from unsafe head
3. Verify each L2 block's L1 origin against canonical L1 chain
4. Find first block where: L1Origin.Number + SyncLookback < highestL2WithCanonicalL1Origin.L1Origin.Number
5. Set safe head to parent of this block
```

### initialReset (Pipeline Reset)
```
1. Start from resetL2Safe (new safe head from FindL2Heads)
2. Walk back L2 chain while: pipelineL2.L1Origin.Number + ChannelTimeout > safe_head.L1Origin.Number
3. With ChannelTimeout = 50 and safe_head.L1Origin = 20:
   - Condition: pipelineL2.L1Origin.Number + 50 > 20
   - Since L1 origin numbers are non-negative, this is always true for valid blocks
   - Loop continues until afterL1Genesis becomes false (reaches genesis)
   - Stops at L1 origin 20 (the safe head's L1 origin)
4. L1 traversal starts from L1 block 20
5. Pipeline advances through L1 blocks 20, 21, 22... until finding batches
6. First batch found at L1 block 156, but it's rejected as expired
```

## The Issue

The `initialReset` logic walks back to find an L1 origin old enough to buffer channel data (accounting for `ChannelTimeout = 50` since `granite_time: 0`).

**The problem**: The condition `pipelineL2.L1Origin.Number + ChannelTimeout > safe_head.L1Origin.Number` causes `initialReset` to always rewind 50 L1 blocks behind the safe head's L1 origin. This means that even when the safe head has advanced significantly (e.g., to L1 origin 97), the pipeline still starts from an old L1 block (e.g., L1 block 47), re-processing batches that were already applied.

**From the logs:**
1. **op-seq startup**: Safe head at 8593921 with L1 origin 22
2. **op-seq initialReset**: Walks back to L1 origin 22 (stops at safe head's L1 origin)
3. **op-seq L1 Traversal**: Starts from L1 block 22, advances through 23, 24, 25... until L1 block 131
4. **op-seq First Batch**: Found at L1 block 131, but rejected as "sequence window expired"
   - Batch's `start_epoch_number=26` (covers L1 blocks 26-66)
   - Batch included in L1 block 131
   - Check: `startEpochNum + SeqWindowSize < l1InclusionBlock.Number` → `26 + 100 < 131` → `126 < 131` → **true**
   - The batch is correctly rejected as expired because `26 + 100 = 126 < 131`, meaning the batch was included too late

5. **op-seq restart**: Safe head recalculated to 8594072 with L1 origin 97
6. **op-seq initialReset after restart**: Walks back to L1 origin 47 (97 - 50 = 47)
7. **op-seq L1 Traversal after restart**: Starts from L1 block 47, advances through 48, 49, 50...
8. **op-rpc**: Continues from L1 block 22, processing batches in order without restart

**Why this causes forks:**
1. **Re-processing of old batches**: `op-seq` restart rewinds to L1 block 47, re-processing batches that were already applied
2. **Different starting points**: `op-seq` starts from L1 block 47 after restart, while `op-rpc` continues from L1 block 22
3. **Inconsistent batch selection**: `op-seq` may find and process different batches from L1 blocks 47-97 than `op-rpc` would find
4. **Forks**: The sequencer and RPC nodes diverge because they're processing batches from different starting points and may select different batches, leading to different L2 block sequences
