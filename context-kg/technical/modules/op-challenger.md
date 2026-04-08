---
name: "op-challenger"
description: "Module design for op-challenger: fault proof dispute game participation"
---
# op-challenger Module

## Responsibilities
- Monitors DisputeGameFactory for new games
- Participates in fault dispute games (attack, defend, step)
- Manages keccak preimage uploads
- Claims bonds on resolved games
- Supports multiple game types (Cannon, Asterisc, SuperCannon)
- Coordinates parallel game processing via scheduler

## NOT Responsible For
- L2 block execution (op-geth)
- Batch submission (op-batcher)
- Block sequencing (op-node)

## Core Entities

| Entity | Key Fields | Description |
|--------|-----------|-------------|
| GameMonitor | source, scheduler, claimer, l1Clock | Subscribes to L1 newHeads and schedules game processing |
| Scheduler | coordinator, jobQueue, resultQueue, workers | Parallel game job scheduling and execution |
| Agent | solver, responder, loader, selective mode | Calculates moves and performs on-chain actions |
| Solver | claimSolver, honestClaims | Determines attack/defend/step actions per claim |
| Responder | txSender, contract, uploader | Executes on-chain transactions for game actions |
| GamePlayer | actor, syncValidator, status | Manages game lifecycle, validates sync before acting |

## Dependencies
- Refer to arch/dependency.md for full dependency details

## Relevant Flows
- Refer to core-flows/fault-proof.md for the fault proof flow

## Module-Specific Pitfalls

[Pitfall] Response delay logic vs clock extension — must check shouldSkipDelay BEFORE sleeping; extension period takes precedence
[Pitfall] Selective claim resolution — only resolve claims we made or countered; do NOT resolve opposing claims
[Pitfall] Stale game status — coordinator enqueues based on cached status; ProgressGame refetches from contract
[Pitfall] Actor lifecycle after game resolves — player replaces actor with actNoop; subsequent calls are no-ops
[Pitfall] Honest claim detection — shouldCounter checks honestClaims.IsHonest; do NOT counter honest claims or claims attacking honest parent
[Pitfall] Preimage oracle state invariant — IsLocal=true always uploads; IsLocal=false only if GlobalDataExists=false
[Pitfall] Disk cleanup race on resolved games — must serialize with job queue; state.inflight gate prevents premature cleanup
