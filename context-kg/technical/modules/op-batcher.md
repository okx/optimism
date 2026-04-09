---
name: "op-batcher"
description: "Module design for op-batcher: L2 transaction batch submission to L1"
---
# op-batcher Module

## Responsibilities
- Submits L2 transaction batches to L1 as calldata or blobs
- Monitors channel state and manages channel lifecycle
- Applies DA compression (zlib, brotli)
- Throttles based on L1 gas and pending DA bytes
- Supports Alt-DA mode for off-chain data availability
- Manages txpool state and handles blocked transactions

## NOT Responsible For
- Deriving L2 blocks from L1 (op-node)
- Executing L2 blocks (op-geth)
- Managing finality (op-node)

## Core Entities

| Entity | Key Fields | Description |
|--------|-----------|-------------|
| BatchSubmitter (driver) | channelManager, txpoolState, syncActions | Main loop: load blocks, publish, handle receipts |
| ChannelManager | channelQueue, currentChannel, blockCursor, blocks queue | Manages channel lifecycle and block-to-frame conversion |
| ChannelBuilder | co (ChannelOut), frames, fullErr, timeout | Compresses blocks and generates output frames |
| SizedBlock | Block, rawSize, daSize | Block with cached size for DA cost estimation |
| ThrottleParams | MaxTxSize, MaxBlockSize, Intensity | Current throttling configuration (0.0–1.0 intensity) |

## Dependencies
- Refer to arch/dependency.md for full dependency details

## Relevant Flows
- Refer to core-flows/batch-submission.md for the batch submission flow

## Module-Specific Pitfalls

[Pitfall] Channel invalidation race condition — L2 reorg invalidates channel during submission; verify all blocks are requeued before continuing
[Pitfall] Empty channel edge case — channel times out before blocks added; always check blocks length before requeuing
[Pitfall] Block not found panic — rewind operation panics if block missing from queue; validate existence before rewind
[Pitfall] DA type switch without validation — switching calldata to blobs may leave stale state; validate channel consistency
[Pitfall] Throttle params not synced across endpoints — if miner_setMaxDASize RPC unavailable, batcher shuts down
[Pitfall] Txpool stuck if cancellation tx also fails — TxpoolCancelPending state persists forever if cancel fails
