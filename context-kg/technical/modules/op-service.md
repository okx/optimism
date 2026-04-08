---
name: "op-service"
description: "Module design for op-service: shared service library for all OP Stack services"
---
# op-service Module

## Responsibilities
- Provides shared RPC server/client infrastructure
- TxManager for reliable L1 transaction submission with gas bumping
- Retry framework with configurable strategies (exponential, fixed)
- Event system for pub-sub with priority scheduling
- Metrics factory with Prometheus integration
- Signing infrastructure (local, remote, mnemonic)
- Source clients: L1Client, L2Client, EngineClient, RollupClient, SupervisorClient, BeaconClient
- Caching, batching, rate limiting for RPC calls
- Thread-safe data structures (RWMap, RWValue, Watch)
- Safe math operations with overflow protection
- CLI framework and lifecycle management

## NOT Responsible For
- Service-specific business logic
- Rollup protocol rules
- Contract interactions beyond generic tx sending

## Core Entities

| Entity | Key Fields | Description |
|--------|-----------|-------------|
| TxManager | Send, SendAsync, nonce management | Reliable tx publication with gas bumping |
| Event.System | Register, Emit, Tracers | Pub-sub event distribution |
| EthClient | Headers, Blocks, Receipts caches | Cached Ethereum RPC client |
| IterativeBatchCall | Keys, Values, BatchSize | Generic RPC request batching |
| SignerFactory | ChainID → SignerFn | Transaction signing factory |

## Dependencies
- Refer to arch/dependency.md for full dependency details

## Module-Specific Pitfalls

[Pitfall] TxManager re-entrance — Send must never be re-entered; nonces managed internally
[Pitfall] Blob price bump 100% vs regular 10% — geth mempool replacement policy requires higher blob bumps
[Pitfall] Event system abort — CriticalErrorEvent sets abort flag; subsequent events skipped
[Pitfall] Watch subscriber blocking — Set() blocks until all subscribers accept; use buffered channels
[Pitfall] IterativeBatchCall size vs concurrency — batch size must not exceed concurrent request limit
