---
name: "architecture-overview"
description: "Layer definitions, allowed/prohibited call directions, service responsibilities"
---
# Architecture Overview

## Layer Definitions

| Layer | Responsibilities | Allowed Calls | Prohibited Calls |
|-------|-----------------|---------------|-----------------|
| Core Sequencing | op-node/rollup/{driver,derive,sequencing}: L2 block derivation, sequencing, engine control | L1Chain, L2Chain, Derivation, Engine | Network/P2P/AltDA (conditional, not direct) |
| L1 State Management | op-node, op-batcher, op-proposer, op-challenger: reading L1 state | L1Client, EthClient, RPCClient | Write operations (must go through TxManager) |
| L2 State & Consensus | op-node/rollup/engine, op-conductor: execution and consensus | EngineController, ExecutionPayload | Direct block validation, finality override |
| Batch & Output Submission | op-batcher, op-proposer: posting data to L1 | TxManager, L1Client | Direct contract calls (must use TxManager) |
| Dispute & Validation | op-challenger: fault proof game participation | GameFactory, BondClaimer, DisputeGame | Direct keccak verification (uses preimage oracle) |
| Operational Infrastructure | op-supervisor, op-dispute-mon, op-interop-mon: monitoring | Metrics, Logging, RPC, PProf | Backend-specific implementation details |
| Testing & Utilities | op-sync-tester, op-test-sequencer, op-dripper, op-faucet | Test backends, session management | Production execution (testing only) |

## Service Responsibilities

| Module | Responsibility | NOT Responsible For |
|--------|---------------|---------------------|
| op-node | Coordinates rollup sequencing, derivation, finality, block execution; manages L1/L2 state; optionally sequences blocks via conductor | Batch posting, output proposal, dispute games, HA independently |
| op-batcher | Submits L2 transaction batches to L1 as calldata or blobs; monitors channel state; applies DA compression; throttles based on L1 gas | Block derivation, L2 execution, finality management |
| op-proposer | Submits finalized L2 output roots to L1 DisputeGameFactory or L2OutputOracle; polls for finalized safe head | Batching, L2 execution, dispute management |
| op-challenger | Monitors DisputeGameFactory, participates in fault games, manages keccak preimages, claims bonds | L2 execution, batching, block sequencing |
| op-conductor | Provides HA sequencer consensus via Raft; coordinates leader election and sequencer state | Block derivation, batching, output proposal |
| op-supervisor | Coordinates cross-chain state consistency between multiple L2 chains; manages interop dependency sets | L2 execution, batching, output proposal, dispute participation |
| op-dispute-mon | Observes dispute games, tracks bonds, monitors claim progression, validates honest actor behavior | Game participation, bond claiming (read-only) |
| op-interop-mon | Monitors cross-chain message delivery and finality consistency across interop chains | Execution, derivation (monitoring only) |
| op-service | Shared libraries: RPC, TxManager, retry, metrics, event system, signing, caching, batching | Service-specific business logic |
| op-core | Core types: fork definitions, predeploy addresses, chain parameters | Runtime logic (type definitions only) |
| op-geth | Modified go-ethereum execution layer for L2 block execution | Rollup logic (execution only) |
| cannon | Fault proof VM: MIPS/RISCV instruction execution for dispute game traces | Online execution (offline trace generation) |
| op-program | Fault proof program: derives L2 state from L1 data inside fault proof VM | Online block building (offline verification) |
| op-deployer | Deployment tooling for L2 chain setup and contract deployment | Runtime operations |
| op-chain-ops | Chain operations: genesis, migrations, cross-domain utilities | Runtime execution (tooling only) |
