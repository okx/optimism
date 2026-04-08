---
name: "rpc-conventions"
description: "JSON-RPC conventions, response format, versioning, authentication patterns"
---
# RPC Conventions

## Response Format

| Wrapper | Fields | Success Convention | Error Convention |
|---------|--------|-------------------|-----------------|
| Native geth JSON-RPC | json.RawMessage or typed structs | error == nil (Go convention) | error != nil, wrapped with fmt.Errorf |
| OutputResponse | OutputRoot, BlockRef, WithdrawalStorageRoot, StateRoot, Status | Returned directly | Custom error codes via GetErrorCode |
| SafeHeadResponse | L1Block, SafeHead | Returned directly | ethereum.NotFound for missing |
| SyncStatus | CurrentL1, HeadL1, SafeL1, FinalizedL1, UnsafeL2, SafeL2, FinalizedL2 | Full sync state | nil fields for unavailable |

## Versioning

| Strategy | Pattern | Breaking Change Rule |
|----------|---------|---------------------|
| Method suffix | V1/V2/V3/V4 for engine methods | Multiple versions coexist; server selects via suffix |
| Engine methods | GetPayloadV1-V4, ForkchoiceUpdatedV1-V3, NewPayloadV1-V4 | Fork-dependent version selection |
| Type encoding | hexutil.Uint64 for RPC, native uint64 for client types | Conversion at RPC boundary |

## Authentication

[Rule] JWT authentication required for engine and admin RPC routes via WithJWTSecret option
[Rule] Health endpoint must be accessible without authentication for load balancer checks
[Rule] Per-route authentication control via AddRPCWithAuthentication boolean flag
[Convention] JWT secret shared between op-node and execution client

## Pagination

No formal pagination protocol. Manual approaches:
- Chunking via batching.MultiCaller for RPC batch calls
- Block number ranges for sequential data
- Stream polling via Serve() with OutOfEventsErrCode (-39001) backoff
- Stream subscription mode vs polling mode are mutually exclusive

## Idempotency

No explicit idempotency keys. Safety mechanisms:
- JWT authentication per RPC route
- Namespace isolation (admin, health, opstack, supervisor)
- TxManager handles nonce management internally for transaction idempotency
