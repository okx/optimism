---
name: "op-supernode"
description: "Module design for op-supernode: super node aggregation service"
---
# op-supernode Module

## Responsibilities
- Aggregates multiple chain nodes into a single super node
- RPC routing across chains via rpc_router
- Metrics routing via metrics_router
- Shared resource management

## NOT Responsible For
- Individual chain derivation or execution

## Core Entities

| Entity | Key Fields | Description |
|--------|-----------|-------------|
| RPCRouter | Chain routing | Routes RPC calls to appropriate chain node |
| MetricsRouter | Chain routing | Aggregates metrics across chains |

## Dependencies
- Refer to arch/dependency.md for full dependency details

## Module-Specific Pitfalls

None identified.
