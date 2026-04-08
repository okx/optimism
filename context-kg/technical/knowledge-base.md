---
name: "knowledge-base"
description: "Highest-authority rules — all Skills defer to this on conflicts"
---
# Knowledge Base

> This is the highest-weight file in the knowledge base. All Skills that support
> context-kg defer to this file when conflicts arise with general AI knowledge.

## Data Type Constraints

[Rule] op-service/eth/types.go: Bytes32, Bytes48, Bytes65, Bytes96, Bytes256 must use fixed-size arrays, never slices — Reason: deterministic hashing and RLP encoding depend on exact byte length
[Rule] op-service/eth/types.go: Uint64Quantity and Uint256Quantity must use hex encoding for JSON-RPC marshaling — Reason: Engine API specification requires hex-encoded quantities
[Rule] op-service/eth/types.go: PayloadID must be exactly 8 bytes (Bytes8) — Reason: Engine API payload identification protocol
[Rule] op-service/eth/types.go: ErrorCode must check IsEngineError() for range -38000 to -38100 and IsGenericRPCError() for -32600 to -32700 — Reason: Engine API and JSON-RPC standard error code ranges
[Rule] op-node/rollup/types.go: Epoch must be uint64, never use int64 or signed types for block numbers — Reason: block numbers are unsigned; negative values indicate protocol errors
[Rule] op-challenger/game/fault/types/types.go: Position encodes depth and index in game tree; must never be negative — Reason: bisection game tree invariant
[Rule] op-supervisor/supervisor/types/types.go: SafetyLevel values must follow precedence: finalized > safe > cross-unsafe > local-safe > unsafe > invalid — Reason: chain safety model

## Naming Constraints

[Rule] op-service/event/events.go: Event.String() must return simple name string, never content — Reason: used for Prometheus metric labels; content causes cardinality explosion
[Rule] op-service/flags/flags.go: Environment variables must use PrefixEnvVar pattern with OP_ prefix — Reason: namespace isolation across services
[Rule] op-service/metrics/factory.go: Metrics must use fullName(namespace, subsystem, name) pattern — Reason: Prometheus naming convention compliance
[Rule] op-service/rpc/server.go: RPC namespaces must be registered with lowercase names (admin, health, opstack, supervisor) — Reason: JSON-RPC method routing convention
[Rule] op-service/apis/doc.go: API interface names must follow {Namespace}API pattern (AdminAPI, HealthAPI, OpstackAPI) — Reason: consistent API discovery and documentation

## Dependency Constraints

[Rule] op-service/sources/eth_client.go: EthClient must never cache block references by block number — Reason: L1 reorg risk invalidates number-to-hash mapping
[Rule] op-service/sources/eth_client.go: EthClient must never use cached headers/txs when querying by label (latest, safe, finalized) — Reason: chain head volatility
[Rule] op-service/sources/eth_client.go: If TrustRPC is false, EthClient must verify proofs and block hashes against state root — Reason: detect RPC lies from untrusted providers
[Rule] op-service/sources/l1_client.go: L1ClientConfig must size ReceiptsCacheSize to 1.5x sequencing window — Reason: avoid cache misses during normal derivation
[Rule] op-service/sources/l2_client.go: L2ClientConfig cache sizing must scale with (seqWindowSize * 3/2) * (12 / blockTime) — Reason: L2 block density proportional to block time ratio
[Rule] op-service/sources/engine_client.go: EngineClient.NewPayload method version (V2/V3/V4) must match timestamp via EngineVersionProvider — Reason: fork-dependent API versioning
[Rule] op-service/sources/engine_client.go: EngineClient.ForkchoiceUpdate must include attributes for block building; payload ID only returned if building — Reason: Engine API specification
[Rule] op-service/sources/batching/batching.go: IterativeBatchCall batch size must not exceed concurrency limits — Reason: will block if batch > concurrent request limit
[Rule] op-service/sources/limit.go: LimitRPC semaphore must always be released even on error — Reason: prevents deadlock from unreleased semaphore
[Rule] op-service/txmgr/txmgr.go: TxManager.Send must never be re-entered; nonces managed internally — Reason: sequential nonce assignment prevents nonce gaps
[Rule] op-service/txmgr/txmgr.go: Blob transaction price bumps require 100% increase minimum; regular tx 10% — Reason: geth mempool replacement policy
[Rule] op-proposer/proposer/config.go: Proposer must have exactly one of L2OutputOracle or DisputeGameFactory address — Reason: mutually exclusive proposal paths
[Rule] op-batcher/batcher/config.go: Batcher L1EthRpc, L2EthRpc, and RollupRpc must all be specified; L2 and rollup RPC array lengths must match — Reason: one-to-one L2-to-rollup endpoint mapping
[Rule] op-challenger/config/config.go: Challenger requires L1EthRpc, L1Beacon, GameFactoryAddress, Datadir, at least one L2Rpc, and MaxConcurrency >= 1 — Reason: minimum viable configuration for dispute game participation
[Rule] op-node/rollup/derive/pipeline.go: All ResettableStages must reset in linear order (L1Traversal → L1Src → AltDA → FrameQueue → ChannelMux → ChannelInReader → BatchMux → AttributesQueue) — Reason: stage dependencies require ordered reset for consistency
[Rule] op-service/client/rpc.go: RPC clients must respect callTimeout (10s default) and batchCallTimeout (20s default) — Reason: prevent hanging on unresponsive endpoints

## Security Constraints

[Rule] op-service/signer/client.go: TLS minimum version must be TLS 1.3 for remote signer connections — Reason: older TLS versions have known vulnerabilities
[Rule] op-service/rpc/jwt.go: JWT authentication required for engine and admin RPC routes — Reason: prevent unauthorized block building and admin operations
[Rule] op-service/rpc/server.go: Health endpoint must be accessible without authentication — Reason: load balancer health checks must not require secrets
[Rule] op-service/crypto/signature.go: Private key and mnemonic must never both be provided — Reason: ambiguous signing identity
[Rule] op-service/crypto/signature.go: privKey.PublicKey.Curve must be set to crypto.S256() — Reason: match geth nocgo equality check; prevents key mismatch
[Rule] op-service/eth/errors.go: MaybeAsNotFoundErr must normalize block-not-found errors from RPC — Reason: harden against different RPC implementations returning inconsistent error formats
[Rule] op-supervisor/config/config.go: Supervisor requires FullConfigSetSource and SyncSources with Check() validation — Reason: prevent unchecked chain configurations
[Rule] op-node/rollup/interop/config.go: Interop RPC server requires both RPCAddr and RPCJwtSecretPath together — Reason: prevent unauthenticated interop RPC exposure
