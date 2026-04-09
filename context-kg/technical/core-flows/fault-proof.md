---
name: "fault-proof"
description: "Core flow: fault proof and dispute game — monitoring, scheduling, game play, resolution"
---
# Fault Proof / Dispute Game Flow

## Entry Point
- eth_newHeads subscription → gameMonitor.onNewL1Head → progressGames
- Service.Start() → scheduler.Start() → worker pool initialization

## Primary Entities
- GameMonitor, Scheduler, Coordinator, Agent, Solver, Responder, GamePlayer, BondClaimer

## State Transitions

| Current State | Trigger | Target State |
|--------------|---------|-------------|
| GameStatusInProgress | Agent resolves + responder.Resolve() | GameStatusChallengerWon (terminal) |
| GameStatusInProgress | Timeout or defender counters | GameStatusDefenderWon (terminal) |
| Claim: Unresolved | Chess clock exceeded + CallResolveClaim | Claim: Resolved (terminal) |
| Agent: Idle | Scheduler dequeues job | Agent: Acting |
| Agent: Acting | ProgressGame completes | Agent: Idle |
| SchedulerJob: Pending | Coordinator marks inflight | SchedulerJob: Inflight |
| SchedulerJob: Inflight | Worker completes | SchedulerJob: Completed |

**[Rule] Terminal states must never be reversed**: GameStatusChallengerWon, GameStatusDefenderWon

## Normal Flow Steps

| Step | Action | Module |
|------|--------|--------|
| 1 | Subscribe to L1 newHeads, resubscribe on failure | gameMonitor |
| 2 | Update l1Clock, check minUpdatePeriod throttle | gameMonitor.onNewL1Head |
| 3 | Load games from factory via GetGamesAtOrAfter | gameMonitor.progressGames |
| 4 | Pass filtered games to coordinator | scheduler.Schedule |
| 5 | Receive games, call coordinator.schedule | scheduler.loop |
| 6 | Create job → player → ValidatePrestate → enqueue | coordinator.schedule |
| 7 | Check game not resolved, mark inflight | coordinator.createJob |
| 8 | Receive job, call player.ProgressGame | worker |
| 9 | Validate node synced | GamePlayer.ProgressGame |
| 10 | Call actor.Act() | GamePlayer |
| 11 | Try resolve game | Agent.tryResolve |
| 12 | Iterate claims, check chess clock, resolve | Agent.tryResolveClaims |
| 13 | Load all claims, build game tree | Agent.newGameFromContracts |
| 14 | Calculate next actions per claim | solver.CalculateNextActions |
| 15 | Check response delay, validate clock extension | Agent.performAction |
| 16 | Execute move/step/challenge | Agent.performAction switch |
| 17 | Check/upload preimage oracle data | responder.PerformAction |
| 18 | Generate tx candidate | responder (contract bindings) |
| 19 | Send and wait for confirmation | responder.txSender |
| 20 | Process result, cleanup resolved game files | coordinator.processResult |
| 21 | Replace actor with actNoop if resolved | GamePlayer.logGameStatus |
| 22 | Schedule bond claim redemption | gameMonitor → claimer.Schedule |

## Exception Branches

| Trigger | State Change | Compensation |
|---------|-------------|-------------|
| L2BlockNumberChallenged | Game skipped | Return nil, log debug |
| Trace accessor creation fails | Agent creation fails | Error wrapping |
| Game already resolved | Actor replaced with noop | Skip Act calls |
| Node out of sync | Game progression paused | Log warn, return current status |
| Prestate validation fails (allowed) | Log error, continue | Player created anyway |
| Prestate validation fails (not allowed) | Job creation fails | Game not scheduled this cycle |
| Scheduler queue full (ErrBusy) | Skip schedule | Defer to next L1 block |
| Chess clock in extension | Skip response delay | Respond immediately |
| Large preimage upload fails | ErrChallengePeriodNotOver | Queued externally, non-blocking |
| Game state empty | newGameFromContracts fails | Retry next cycle |

## Flow-Specific Pitfalls

[Pitfall] Response delay before clock extension check — must call shouldSkipDelay BEFORE sleeping
[Pitfall] Selective claim resolution — only resolve own claims or claims where we countered
[Pitfall] Stale game status in coordinator — cached status may differ from contract; ProgressGame refetches
[Pitfall] Actor lifecycle after resolution — replaced with actNoop; state does not persist
[Pitfall] Honest claim detection — do NOT counter honest claims or claims attacking honest parent
[Pitfall] Chess clock formula — claim.Clock + (now - claim.Timestamp); not just current time
[Pitfall] Disk cleanup race — must serialize with job queue; state.inflight prevents premature cleanup
[Pitfall] Preimage oracle invariant — IsLocal=true always uploads; IsLocal=false only if GlobalDataExists=false
