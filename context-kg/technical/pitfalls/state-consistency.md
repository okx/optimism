---
name: "state-consistency"
description: "Pitfalls related to cross-chain and DB state consistency"
---
# State Consistency Pitfalls

[Pitfall] Anchor point mismatch on initialization — chain DB anchor point conflicts with existing state
- Trigger: Supervisor initialization with conflicting anchor
- Correct approach: Verify hash equality of anchor vs existing first entry on init
- Module: op-supervisor (anchor.go)

[Pitfall] Inconsistent cross-chain reads — multiple reads without atomicity guarantee
- Trigger: Concurrent reads from different chains during state computation
- Correct approach: Implement atomic multi-read or retry with snapshot isolation
- Module: op-supervisor (cross/safe_update.go)

[Pitfall] Safe head db inconsistency on EL sync start — stale data returned during EL sync transition
- Trigger: EL sync start while safe head DB has stale entries
- Correct approach: SyncDeriver.OnELSyncStarted() calls SafeHeadReset(eth.L2BlockRef{}) before proceeding
- Module: op-node (sync_deriver.go)

[Pitfall] Batch reader edge case with io.EOF — batch may be non-nil while err==io.EOF
- Trigger: RLP stream returns data and EOF simultaneously
- Correct approach: Check both batch and error; handle dual return from rlp.Stream
- Module: op-node (channel_in_reader.go)

[Pitfall] Stale game status in coordinator cache — coordinator caches game status from previous block
- Trigger: Game resolved between blocks but coordinator uses stale status
- Correct approach: ProgressGame refetches status from contract; do not rely on cached status
- Module: op-challenger (coordinator.go)

[Pitfall] Configuration mismatch on upgrade — defaults to initiating chain if executing chain not specified
- Trigger: Supervisor config without explicit executing chain
- Correct approach: Validate executing vs initiating chain IDs explicitly
- Module: op-supervisor (backend.go)

[Warning] L1 BlockRef heuristic fragility — MaybeAsNotFoundErr uses string matching for compatibility with different RPC implementations
- Module: op-service (eth/errors.go)
