---
name: "op-node"
description: "Module design for op-node: rollup sequencing, derivation, finality, P2P"
---
# op-node Module

## Responsibilities
- Coordinates full rollup sequencing, derivation, finality, and block execution
- Manages L1/L2 state synchronization
- Optionally sequences blocks (controlled by conductor)
- Serves RPC API for downstream services (optimism_syncStatus, optimism_outputAtBlock)
- Manages P2P gossip network for block propagation
- Handles fork choice updates and engine API interactions

## NOT Responsible For
- Posting batches to L1 (op-batcher)
- Proposing L2 output roots (op-proposer)
- Managing dispute games (op-challenger)
- Running as HA service independently (requires op-conductor)

## Core Entities

| Entity | Key Fields | Description |
|--------|-----------|-------------|
| rollup.Config | Genesis, BatchInboxAddress, DepositContractAddress, L1ChainID, L2ChainID | Main rollup configuration |
| driver.Driver | StatusTracker, Finalizer, EngineController, Sequencer, DerivationPipeline | Main event loop coordinator |
| DerivationPipeline | ResettableStages[], engineIsReset, resetting counter | Ordered stage pipeline for L1→L2 derivation |
| EngineController | unsafeHead, pendingSafe, localSafe, safeHead, finalizedHead | Manages execution engine state |

## Dependencies
- Refer to arch/dependency.md for full dependency details

## Relevant Flows
- Refer to core-flows/derivation-pipeline.md for the derivation pipeline flow

## Module-Specific Pitfalls

[Pitfall] Engine reset not confirmed before pipeline step continues — ErrEngineResetReq returned; pipeline.Step() must check engineIsReset flag first
[Pitfall] Context cancellation during initialization — node init() cancels context mid-init; all components must properly cleanup
[Pitfall] Nil Rollup Config ChainOpConfig — cfg.Rollup.ChainOpConfig can be nil, causing panic downstream; add explicit nil checks
[Pitfall] Event system drain blocking — event drain may block if consumers don't read fast enough
[Pitfall] Timestamp-based fork detection collision — fork detection relies on activation timestamps which can collide; verify fork precedence rules
[Warning] Multiple TODOs marking incomplete event system refactoring across node.go, engine_controller.go, finalizer.go
