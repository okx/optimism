---
name: "op-proposer"
description: "Module design for op-proposer: L2 output root proposal to L1"
---
# op-proposer Module

## Responsibilities
- Submits finalized L2 output roots to L1 DisputeGameFactory or L2OutputOracle
- Polls for finalized (or safe, if AllowNonFinalized) L2 head
- Supports both legacy L2OO and DGF proposal paths
- Supports RollupRPC and SupervisorRPC as proposal sources
- Manages proposal interval and deduplication

## NOT Responsible For
- Batching transactions (op-batcher)
- Executing L2 blocks (op-geth)
- Managing dispute games (op-challenger)

## Core Entities

| Entity | Key Fields | Description |
|--------|-----------|-------------|
| L2OutputSubmitter | running (atomic.Bool), Cfg, Txmgr, ProposalSource | Main loop: poll, fetch output, submit proposal |
| ProposalSource | SyncStatus, ProposalAtSequenceNum | Interface for rollup or supervisor RPC |
| DisputeGameFactory | Contract binding for create(gameType, outputRoot, l2BlockNum) | Proposal target for DGF path |

## Dependencies
- Refer to arch/dependency.md for full dependency details

## Relevant Flows
- Refer to core-flows/l2-output-proposal.md for the proposal flow

## Module-Specific Pitfalls

[Pitfall] AllowNonFinalized=false blocks proposals on slow finality — always uses FinalizedL2 head; L1 reorg may make block unavailable
[Pitfall] L1 blockhash(blockNum) returns 0 if l1head == blockNum — must wait for l1head > blockNum before L2OO proposal
[Pitfall] Conflicting RPC sources — config rejects both rollup and supervisor RPC; ErrConflictingSource
[Pitfall] Genesis block proposal skipped in DGF mode — currentBlockNumber == GenesisHeight causes skip
[Pitfall] DisputeGameFactory HasProposedSince iterates backward — latency increases linearly with game count
