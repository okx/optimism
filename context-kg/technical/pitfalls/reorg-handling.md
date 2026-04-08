---
name: "reorg-handling"
description: "Pitfalls related to L1/L2 reorg handling across modules"
---
# Reorg Handling Pitfalls

[Pitfall] Channel invalidation on L2 reorg — L2 reorg invalidates channel during submission; verify all blocks are requeued before continuing channel processing
- Trigger: L2 reorg while batcher has in-flight channels
- Correct approach: handleChannelInvalidated() rewinds blockCursor, requeues blocks, drops newer channels
- Module: op-batcher (channel_manager.go)

[Pitfall] L1 reorg detection in derivation — parent hash mismatch triggers full pipeline reset
- Trigger: L1 reorganization during L1 traversal
- Correct approach: pipeline.Step() → Reset all stages sequentially via L1Traversal → L1Src → FrameQueue → etc.
- Module: op-node (pipeline.go)

[Pitfall] Reorg handling incomplete in supervisor DB — multiple TODOs indicate reorg handling not fully implemented
- Trigger: Chain reorganization affecting supervisor state
- Correct approach: Implement bisection reset for pre-interop blocks
- Module: op-supervisor (db.go)

[Pitfall] Interop activation block reorg — Rewinder may fail on reorg affecting activation block
- Trigger: Reorg at exact activation block boundary
- Correct approach: Add sentinel error checks for activation block reorgs
- Module: op-supervisor (reset_tracker.go)

[Pitfall] Block references cached by number — EthClient caches by hash only; number-based caching invalidated by reorg
- Trigger: L1 reorg causes number→hash mapping change
- Correct approach: Only cache by block hash, never by block number
- Module: op-service (eth_client.go)

[Warning] Node inconsistency during reorg — DB may indicate conflict when node state inconsistent with supervisor
- Module: op-supervisor (syncnode/reset_tracker.go)
