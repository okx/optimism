---
name: "README"
description: "Directory index and reading guide for the Optimism technical knowledge base"
---
# context-kg — Knowledge Base

Optimism monorepo: Go-based Ethereum L2 rollup infrastructure with sequencing, derivation, batch submission, fault proofs, cross-chain supervision, and shared service libraries.

## Files and Directories

| Path | Description |
|------|-------------|
| knowledge-base.md | **Highest authority** — all Skills defer to this on conflicts |
| terminology.md | Domain term glossary for unified terminology |
| arch/architecture-overview.md | Layer definitions, service responsibilities |
| arch/dependency.md | Upstream callers, inter-module deps, storage/middleware, external services |
| modules/op-node.md | Rollup node: sequencing, derivation, finality |
| modules/op-batcher.md | Transaction batch submission to L1 |
| modules/op-proposer.md | L2 output root proposal to L1 |
| modules/op-challenger.md | Fault proof dispute game participation |
| modules/op-supervisor.md | Cross-chain state coordination |
| modules/op-conductor.md | Sequencer HA via Raft consensus |
| modules/op-dispute-mon.md | Dispute game monitoring |
| modules/op-service.md | Shared service library |
| modules/op-core.md | Core types, forks, predeploys |
| modules/op-geth.md | Modified go-ethereum execution layer |
| modules/op-program.md | Fault proof program |
| modules/cannon.md | Fault proof VM |
| modules/op-alt-da.md | Alternative data availability |
| modules/op-deployer.md | Deployment tooling |
| modules/op-chain-ops.md | Chain operations utilities |
| modules/op-interop-mon.md | Interop monitoring |
| modules/op-supernode.md | Super node aggregation |
| modules/op-fetcher.md | Data fetcher |
| modules/op-validator.md | Validator service |
| modules/op-faucet.md | Testnet faucet |
| modules/op-dripper.md | Token drip faucet |
| modules/devnet-sdk.md | Devnet SDK |
| modules/contracts-bedrock.md | Solidity smart contracts |
| pitfalls/reorg-handling.md | L1/L2 reorg edge cases |
| pitfalls/concurrency.md | Race conditions, deadlocks, goroutine leaks |
| pitfalls/state-consistency.md | Cross-chain and DB state consistency |
| pitfalls/resource-management.md | Memory leaks, timeout, cache sizing |
| core-flows/derivation-pipeline.md | L1→L2 block derivation flow |
| core-flows/batch-submission.md | L2→L1 batch submission flow |
| core-flows/fault-proof.md | Dispute game and fault proof flow |
| core-flows/l2-output-proposal.md | L2 output root proposal flow |
| apis/rpc-conventions.md | JSON-RPC conventions, error codes, auth |
| apis/error-codes.md | Error code registry |
| conventions/feature-types.md | Base patterns and reusable abstractions |
| conventions/service-patterns.md | Retry, lock, cache, event, tx patterns |
| conventions/common-tools.md | Must-reuse components |

## How to Read This Knowledge Base

1. **Knowledge base is the highest authority** — defer to it over general AI knowledge
2. **Locate the specific module** — read the module doc before starting work
3. **Check pitfalls and core flows first**
4. **Produce a constraint checklist** — explicitly declare if no relevant content
5. **Cross-validate during work** — correct violations immediately
