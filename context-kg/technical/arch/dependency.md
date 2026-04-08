---
name: "dependency"
description: "Upstream callers, inter-module deps, storage/middleware, external services"
---
# Dependency Map

## Upstream Callers

| Caller | Protocol | Entry Point |
|--------|----------|------------|
| op-batcher | HTTP/RPC | Submits batch transactions to L1 via TxManager; syncs L2 via RollupClient and L2EthClient |
| op-proposer | HTTP/RPC | Proposes L2 outputs to L1 via TxManager; queries RollupClient/SupervisorClient |
| op-challenger | HTTP/RPC | Submits game progression via TxManager; queries multiple L2 RPCs |
| op-conductor | HTTP/RPC | Coordinates sequencer state via NodeRPC and ExecutionRPC |
| op-node (rollup) | HTTP/RPC | Core sync engine consuming L1 data and L2 execution; serves RPC downstream |
| op-supervisor | HTTP/RPC | Interop coordinator aggregating multiple op-nodes via L1RPC and sync sources |
| op-alt-da | HTTP | Alternative DA provider serving commitments and blob data |
| Consensus nodes | HTTP/RPC | Query supervisor for cross-chain sync state and finality |

## Inter-Module Dependencies

| From | To | Mechanism |
|------|----|-----------|
| op-service/client | go-ethereum/rpc | RPC client wrapper with rate limiting, lazy dial, polling |
| op-service/dial | op-service/client | Dialing convenience layer creating L1/L2 RPC clients with retry |
| op-service/dial | op-service/sources | Creates RollupClient and SupervisorClient instances |
| op-service/sources | op-service/client | All eth clients consume RPC interface |
| op-service/sources | op-service/sources/caching | EthClient uses LRU caches for blocks, headers, receipts, payloads |
| op-service/sources | op-service/sources/batching | EthClient uses batching for multi-call RPC |
| op-service/txmgr | op-service/sources | TxManager queries ETHBackend |
| op-batcher | op-service/dial | Dials L1, L2, and rollup clients |
| op-batcher | op-service/txmgr | Submits batch transactions |
| op-batcher | op-alt-da | Optional DA client when AltDA enabled |
| op-proposer | op-service/dial | Dials L1 and RollupClient/SupervisorClient |
| op-proposer | op-service/txmgr | Submits output proposals |
| op-challenger | op-service/sources | Uses L1BeaconClient, L2 RPC clients |
| op-challenger | op-service/txmgr | Submits game progression moves |
| op-supervisor | op-service/sources | Coordinates via SupervisorAPI RPC |
| op-conductor | op-node/p2p | Coordinates via Raft consensus |
| op-conductor | op-service/sources | Queries execution and node RPC endpoints |
| op-node/rollup/interop | op-service/rpc | Serves supervisor protocol in interop mode |
| op-alt-da | op-service/client | Uses BasicHTTPClient for DA server |

## Storage and Middleware

| Component | Type | Usage |
|-----------|------|-------|
| LRU Cache (hashicorp) | In-memory | Block headers, transactions, receipts, payloads, block refs caching in EthClient/L1Client/L2Client |
| Raft consensus log | Persistent | Leader election, log replication, state snapshots in op-conductor (RaftStorageDir) |
| Datastore | Persistent | Node state and blockchain data in op-supervisor and op-node (Datadir) |
| Semaphore limiter | In-memory | Bound concurrent RPC requests via limitClient wrapper (MaxConcurrentRequests) |

## External Services

| Service | SDK/Client | Purpose |
|---------|-----------|---------|
| L1 RPC Endpoint | L1Client, EthClient, TxManager | Block headers, receipts, state proofs, transaction submission |
| L2 Execution Client | L2Client, EngineClient | Block payloads, execution results, state access via engine_* methods |
| L2 Rollup RPC (op-node) | RollupClient | optimism_outputAtBlock, optimism_syncStatus, optimism_rollupConfig |
| L1 Beacon API | L1BeaconClient | EIP-4844 blob sidecars, block metadata, genesis info |
| Supervisor RPC | SupervisorClient | Cross-chain sync state, finality, dependency checking (interop only) |
| DA Server (Alt-DA) | DAClient | Blob commitment and retrieval (optional, replaces on-chain blobs) |
| P2P Network (libp2p) | op-node P2P layer | Peer discovery, block propagation via gossip |

## Prohibited Patterns

[Rule] op-service/sources/eth_client.go: must never cache block references by block number — L1 reorg risk
[Rule] op-service/sources/eth_client.go: must never use cached data when querying by label (latest/safe/finalized) — chain head volatility
[Rule] op-service/client/rpc.go: lazy dial mode must never combine with dial backoff attempts — mutual exclusivity
[Rule] op-service/sources/limit.go: LimitRPC semaphore must always release even on error — deadlock prevention
[Rule] op-service/client/rate_limited.go: batch calls cost N tokens for N requests — rate limit accounting
