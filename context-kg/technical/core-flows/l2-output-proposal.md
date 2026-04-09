---
name: "l2-output-proposal"
description: "Core flow: L2 output root proposal — polling, fetching, submitting to L1"
---
# L2 Output Proposal Flow

## Entry Point
- CLI main → cliapp.LifecycleCmd → proposer.Main() → ProposerServiceFromCLIConfig
- RPC admin: admin_start-proposer, admin_stop-proposer

## Primary Entities
- L2OutputSubmitter, ProposalSource (RollupProposalSource or SupervisorProposalSource), DisputeGameFactory, L2OutputOracle

## State Transitions

| Current State | Trigger | Target State |
|--------------|---------|-------------|
| ProposerService: stopped | StartL2OutputSubmitting() | ProposerService: started |
| L2OutputSubmitter: not running | CAS succeeds | L2OutputSubmitter: running |
| L2OutputSubmitter: running | StopL2OutputSubmitting() CAS | L2OutputSubmitter: stopped |
| Proposal: not needed | FetchL2OOOutput/FetchDGFOutput returns shouldPropose=true | Proposal: needs submission |
| Proposal: needs submission | sendTransaction succeeds | Proposal: submitted (terminal for round) |

**[Rule] Terminal states must never be reversed**: submitted proposal (one per round)

## Normal Flow Steps

| Step | Action | Module |
|------|--------|--------|
| 1 | Parse CLI flags, validate config | main.go, config.go:Check() |
| 2 | Initialize service: metrics, TxManager, L1Client, ProposalSource | service.go |
| 3 | Create ProposalSource (Rollup or Supervisor) | service.go:initRPCClients |
| 4 | Create L2OutputSubmitter (L2OO or DGF) | service.go:initDriver |
| 5 | Start polling loop at PollInterval (12s default) | driver.go:loop |
| 6 | Query L2OO NextBlockNumber or DGF HasProposedSince | FetchL2OOOutput / FetchDGFOutput |
| 7 | Get sync status (FinalizedL2 or SafeL2) | FetchCurrentBlockNumber |
| 8 | Fetch L2 output root and L1 block info | ProposalSource.ProposalAtSequenceNum |
| 9 | Wait for L1 head > blockNum | waitForL1Head |
| 10 | Build and send transaction | sendTransaction (L2OO or DGF path) |
| 11 | DGF: query initBonds, build create() tx | DisputeGameFactory.ProposalTx |

## Exception Branches

| Trigger | State Change | Compensation |
|---------|-------------|-------------|
| NextBlockNumber < current | No change | Skip proposal, debug log |
| Output not finalized (or not safe) | No change | Skip, wait for finality |
| Recently proposed (DGF) | No change | Skip, interval not elapsed |
| Output root unchanged | No change | Skip, no new data |
| Fetch errors | Continue loop | Warn log, retry next interval |
| L1Head not advanced | Wait polling | Ticker-based retry |
| Receipt failed | Submitted but reverted | Error log, continue |
| Start when already running | Error | CAS fails |

## Flow-Specific Pitfalls

[Pitfall] AllowNonFinalized=false blocks proposals on slow finality — default uses FinalizedL2 only
[Pitfall] L1 blockhash returns 0 if l1head == blockNum — must wait for l1head > blockNum before L2OO submit
[Pitfall] Conflicting RPC sources — cannot have both rollup and supervisor RPC
[Pitfall] Genesis block proposal skipped in DGF mode — genesis height causes skip
[Pitfall] DGF HasProposedSince linear scan — latency increases with game count
[Pitfall] SupervisorProposalSource uses MinSyncedL1 — if clients diverge, uses lowest L1
[Pitfall] Context timeout 10 minutes in proposeOutput — abandoned if stuck, no retry
