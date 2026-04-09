---
name: "derivation-pipeline"
description: "Core flow: L1→L2 block derivation pipeline — entry point, state transitions, steps, exceptions"
---
# Derivation Pipeline Flow

## Entry Point
- Driver.eventLoop() — main event loop triggered by L1 updates, finalized blocks, sequencer timer
- PipelineDeriver.OnEvent(PipelineStepEvent) — derivation step handler initiated by StepSchedulingDeriver
- SyncDeriver.SyncStep() — synchronization step called by event loop

## Primary Entities
- DerivationPipeline, EngineController, L1Traversal, L1Retrieval, FrameQueue, ChannelMux, ChannelInReader, BatchQueue, AttributesQueue

## State Transitions

| Current State | Trigger | Target State |
|--------------|---------|-------------|
| L1Traversal: idle | AdvanceL1Block() | L1Traversal: fetching → ready |
| L1Retrieval: closed | NextData() | L1Retrieval: open with data |
| FrameQueue: accumulating | NextFrame() | FrameQueue: popping frames |
| ChannelMux: pre-Holocene (ChannelBank) | Transform(forks.Holocene) or Reset() | ChannelMux: post-Holocene (ChannelAssembler) |
| ChannelInReader: empty | WriteChannel() or NextChannel() | ChannelInReader: decompressed batches |
| BatchQueue: buffering | NextBatch(parent) | BatchQueue: validating → emitting SingularBatch |
| AttributesQueue: batch input | NextAttributes(parent) | AttributesQueue: PayloadAttributes output |
| Engine: unsafeHead | ForkchoiceUpdate, NewPayload | Engine: pendingSafe → localSafe → safeHead → finalizedHead |
| Pipeline: resetting | ConfirmEngineReset() | Pipeline: ready |

**[Rule] Terminal states must never be reversed**: finalizedHead (only advances forward)

## Normal Flow Steps

| Step | Action | Module |
|------|--------|--------|
| 1 | Plans sequencer action, monitors L1, checks unsafe queue gaps, schedules derivation | Driver.eventLoop() |
| 2 | Calls Engine.TryUpdateEngine(), checks EL sync status | SyncDeriver.SyncStep() |
| 3 | Requests derivation pipeline step | StepSchedulingDeriver.AttemptStep() |
| 4 | Drives all stages, handles reset, advances L1 origin on EOF | DerivationPipeline.Step() |
| 5 | Fetches next L1 block, checks for reorg, updates system config | L1Traversal.AdvanceL1Block() |
| 6 | Opens data source for current L1 block, iterates frame data | L1Retrieval.NextData() |
| 7 | Parses frames, prunes invalid (Holocene), buffers | FrameQueue.NextFrame() |
| 8 | Reads raw channels, decompresses, deserializes batches | ChannelMux + ChannelInReader |
| 9 | Validates batches against L1 origin, generates empty on timeout | BatchQueue/Stage |
| 10 | Creates PayloadAttributes from singular batch | AttributesQueue.NextAttributes() |
| 11 | Buffers pending attributes, waits for PendingSafeUpdate | AttributesHandler |
| 12 | Submits ForkchoiceUpdate to EL with new pendingSafeHead | Engine.TryUpdatePendingSafe() |
| 13 | Executes payload, advances localSafeHead | Engine.NewPayload() |
| 14 | Marks block as locally safe after span batch | Engine.TryUpdateLocalSafe() |
| 15 | Marks blocks as finalized based on L1 finality | Finalizer |

## Exception Branches

| Trigger | State Change | Compensation |
|---------|-------------|-------------|
| L1 reorg (parent hash mismatch) | ResetEvent emitted | pipeline.Step() → Reset all stages sequentially |
| Frame queue prune fails (Holocene) | Invalid frames discarded | Continuable, no state rollback |
| Batch validation failure | Batch rejected, reset triggered | AttributesQueue → NewResetError |
| Engine EL sync in progress | Pipeline backs off | SyncDeriver checks IsEngineInitialELSyncing() |
| Engine execution fails | Temporary error backoff | Attributes held in handler, retry next step |
| Safe head notification fails | ResetEvent emitted | Pipeline reset triggered |
| Channel decompression error | NotEnoughData | ChannelInReader calls NextChannel() |
| Insufficient L1 data | Derivation idles | Wait for L1 or timer trigger |
| Unsafe payload queue gap | Request sync | Driver.checkForGapInUnsafeQueue() |

## Flow-Specific Pitfalls

[Pitfall] Engine reset not confirmed before step continues — ErrEngineResetReq; pipeline.Step() must check engineIsReset flag first
[Pitfall] Pending attributes not confirmed (in-flight) — PipelineDeriver tracks needAttributesConfirmation, drops step if true
[Pitfall] L1 origin transition without verification — DerivationPipeline.Step() must call VerifyNewL1Origin()
[Pitfall] Fork activation missed during reset — transformStages() must check IsActivationBlock() and call Transform()
[Pitfall] Channel timeout not applied — BatchQueue must check ChannelTimeout(pipelineOrigin.Time)
[Pitfall] Pipeline reset loop not terminated — resetting counter only increments on io.EOF from Reset()
[Pitfall] Safe head db inconsistency on EL sync start — SyncDeriver.OnELSyncStarted() calls SafeHeadReset(eth.L2BlockRef{})
