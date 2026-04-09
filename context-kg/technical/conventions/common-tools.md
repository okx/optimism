---
name: "common-tools"
description: "Must-reuse components across OP Stack services"
---
# Common Tools — Must-Reuse Components

## RPC & Networking

[Reuse] op-service/client.RPC — RPC client wrapper with rate limiting, lazy dial, polling
- [Rule] Must respect callTimeout (10s) and batchCallTimeout (20s)

[Reuse] op-service/dial — dialing convenience for L1/L2 RPC clients with retry
- Creates RollupClient, SupervisorClient, L1Client, L2Client

[Reuse] op-service/sources.EthClient — cached Ethereum RPC client
- Headers, blocks, receipts, payload caches via LRU

[Reuse] op-service/sources.L1Client — L1-specific cached client
- Receipt cache sized to sequencing window

[Reuse] op-service/sources.L2Client — L2-specific cached client
- Cache scaled to block time ratio

[Reuse] op-service/sources.EngineClient — Engine API client
- Fork-dependent method version selection (V2/V3/V4)

[Reuse] op-service/sources.RollupClient — Rollup RPC client
- optimism_* namespace methods

[Reuse] op-service/sources.SupervisorClient — Supervisor RPC client
- Interop-mode cross-chain queries

[Reuse] op-service/sources.L1BeaconClient — Beacon API client
- EIP-4844 blob sidecar fetching

## Transaction Management

[Reuse] op-service/txmgr.TxManager — reliable tx publishing
- Gas bumping, receipt handling, nonce management
- [Rule] Must use for all L1 transaction submission

## Batching & Multiplexing

[Reuse] op-service/sources/batching.IterativeBatchCall — generic RPC batch caller
- Type-safe via BatchElementCreator and CallResult

[Reuse] op-service/sources/batching.MultiCaller — high-level batch wrapper
- Call(ctx, block, ...Call) returns []*CallResult

## Metrics & Observability

[Reuse] op-service/metrics.Factory — Prometheus metrics factory with documentation
- [Rule] Must use for all metrics registration; auto-documents on creation

[Reuse] op-service/oppprof — pprof service integration
- Standard profiling endpoint setup

## CLI & Lifecycle

[Reuse] op-service/cliapp.LifecycleCmd — CLI lifecycle management
- Signal handling, graceful shutdown, two-context model

[Reuse] op-service/flags — standard CLI flags with OP_ env prefix
- [Rule] Must include OverridableForks in override flags

[Reuse] op-service/cliapp.ProtectFlags — clone Generic values before flag reuse
- Prevents accidental flag-value mutations

## Cryptography & Signing

[Reuse] op-service/crypto.SignerFactory — transaction signing factory
- Local, remote (RPC), mnemonic signing support

[Reuse] op-service/signer.SignerClient — remote signing RPC client
- TLS 1.3 minimum, certman cert rotation, health check

## Concurrency

[Reuse] op-service/locks.RWMap — thread-safe generic map
[Reuse] op-service/locks.RWValue — thread-safe single value wrapper
[Reuse] op-service/locks.Watch — watchable value with broadcast
[Reuse] op-service/safego.NoCopy — copy-detection marker for go vet
[Reuse] op-service/safemath — overflow-protected arithmetic

## Event System

[Reuse] op-service/event.System — pub-sub with priority, rate limiting, tracing
- [Rule] Must use for all event-driven communication within services

## Error Handling

[Reuse] op-service/errutil — error utility functions
[Reuse] op-service/eth.InputError — Engine API input validation error with code
[Reuse] op-service/eth.MaybeAsNotFoundErr — normalize block-not-found across RPCs
