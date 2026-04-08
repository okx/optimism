---
name: "op-supervisor"
description: "Module design for op-supervisor: cross-chain state coordination for interop"
---
# op-supervisor Module

## Responsibilities
- Coordinates cross-chain state consistency between multiple L2 chains
- Manages interop dependency sets
- Provides query RPC interface for sync state and finality
- Polls and drains event executor synchronously (100ms interval)
- Tracks safety levels: finalized, safe, cross-unsafe, local-safe, unsafe

## NOT Responsible For
- L2 block execution (op-geth)
- Batch submission (op-batcher)
- Output proposal (op-proposer)
- Dispute game participation (op-challenger)

## Core Entities

| Entity | Key Fields | Description |
|--------|-----------|-------------|
| SupervisorService | backend, rpcServer, pprofService, metricsService | Main service lifecycle |
| Backend | syncSources, chainDB, eventExecutor | Core state management and event processing |
| SafetyLevel | finalized, safe, cross-unsafe, local-safe, unsafe, invalid | Data safety classification enum |
| DerivedBlockRefPair | Source (L1), Derived (L2) | L1-to-L2 block mapping |
| BlockReplacement | Replacement ref, invalidated hash | Invalid block replacement record |

## Dependencies
- Refer to arch/dependency.md for full dependency details

## Module-Specific Pitfalls

[Pitfall] Anchor point mismatch on initialization — chain DB anchor conflicts with existing state; verify hash equality
[Pitfall] Reorg handling incomplete — multiple TODOs indicate reorg handling is not fully implemented in state tracking
[Pitfall] Block number underflow — explicit underflow protection needed for block 0; check blockNum > 0 before subtraction
[Pitfall] Goroutine leak on channel close — indefinite loop if node-event channel closed without reinitialization
[Pitfall] Inconsistent cross-chain reads — multiple reads without atomicity; implement retry with snapshot isolation
[Pitfall] Memory leak in read handles — readHandle must be released or causes leak; always defer Release()
[Warning] Configuration mismatch — if user doesn't specify executing chain, defaults to initiating chain
