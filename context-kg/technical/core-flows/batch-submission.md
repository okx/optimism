---
name: "batch-submission"
description: "Core flow: L2→L1 batch submission — block loading, channel management, tx publishing"
---
# Batch Submission Flow

## Entry Point
- POST /admin/start_batcher → adminAPI.StartBatcher — starts batch submission loop
- blockLoadingLoop — polls L2 sequencer for sync status at PollInterval
- /admin/flush_batcher → adminAPI.FlushBatcher — forces immediate publish

## Primary Entities
- BatchSubmitter (driver), ChannelManager, ChannelBuilder, TxManager, ThrottleParams

## State Transitions

| Current State | Trigger | Target State |
|--------------|---------|-------------|
| TxpoolGood | ErrAlreadyReserved | TxpoolBlocked |
| TxpoolBlocked | detectTxpoolBlocked | TxpoolCancelPending |
| TxpoolCancelPending | Cancel tx completes | TxpoolGood |
| ChannelBuilder: initialized | AddBlock exceeds capacity or timeout | ChannelBuilder: IsFull |
| Channel: noneSubmitted | TxConfirmed | Channel: hasConfirmedTxs → isFullySubmitted |
| Channel: hasConfirmedTxs | channelTimeout exceeded | Channel: isTimedOut |

**[Rule] Terminal states must never be reversed**: isFullySubmitted, isTimedOut (channel lifecycle ends)

## Normal Flow Steps

| Step | Action | Module |
|------|--------|--------|
| 1 | Poll L2 sequencer for sync status | blockLoadingLoop |
| 2 | Compute sync actions (prune/clear/load) | syncAndPrune() |
| 3 | Load unsafe blocks into channel manager | loadBlocksIntoState() |
| 4 | Signal throttling loop with DA bytes | sendToThrottlingLoop() |
| 5 | Signal publishing loop | tryPublishSignal() |
| 6 | Enqueue L2 blocks, detect reorg | channelManager.blocks.Enqueue() |
| 7 | Create new channel if needed | ensureChannelWithSpace() |
| 8 | Add blocks to channel until full | processBlocks() |
| 9 | Compress block data | ChannelBuilder.co.AddBlock() |
| 10 | Generate output frames | OutputFrames() |
| 11 | Prepare tx data from frames | nextTxData() |
| 12 | Dequeue tx data | channelMgr.TxData() |
| 13 | Handle Alt-DA path if enabled | publishToAltDAAndL1() |
| 14 | Create blob or calldata tx candidate | blobTxCandidate() / calldataTxCandidate() |
| 15 | Submit via TxManager | sendTx() |
| 16 | Process receipts | receiptsLoop |
| 17 | Record failures for retry | TxFailed() — rewind frame cursor |
| 18 | Record confirmations, check timeout | TxConfirmed() — update inclusion blocks |
| 19 | Handle timed-out channels | handleChannelInvalidated() — rewind, requeue |
| 20 | Compute throttle params | throttlingLoop |
| 21 | Distribute throttle to L2 endpoints | miner_setMaxDASize RPC |

## Exception Branches

| Trigger | State Change | Compensation |
|---------|-------------|-------------|
| L2 reorg (parent mismatch) | AddL2Block returns ErrReorg | waitNodeSyncAndClearState(), retry |
| Sync status out-of-order | outOfSync=true | Skip pruning/loading, retry next tick |
| Safe head advanced past local | Gap detected | Clear state, reload from safeL2.Number+1 |
| Txpool blocked | ErrAlreadyReserved | Send cancellation tx to unblock |
| Channel timeout on chain | isTimedOut=true | Rewind blockCursor, retry in new channel |
| DA server request fails | Error | recordFailedDARequest(), retry |
| Transaction send fails | Error | recordFailedTx(), retry |
| Max pending transactions | Queue blocks | Wait for prior txs to confirm |
| Context cancelled | Done() | All goroutines exit gracefully |

## Flow-Specific Pitfalls

[Pitfall] Reorg undetected before channel creation — AddL2Block only checks tip hash; gaps can slip through
[Pitfall] Channel duration timeout too aggressive — large backlog causes premature channel close; use ignoreMaxChannelDuration
[Pitfall] Throttle params not synced — if miner_setMaxDASize unavailable, batcher shuts down
[Pitfall] Txpool stuck if cancellation fails — TxpoolCancelPending persists forever
[Pitfall] Channel cursor rewind loses pending frames — only rewinds one channel on failure
[Pitfall] Nonce drift if TxManager resets — monitor pending tx age and reset nonce if stale
