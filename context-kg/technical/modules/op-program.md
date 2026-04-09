---
name: "op-program"
description: "Module design for op-program: fault proof program for offline L2 state verification"
---
# op-program Module

## Responsibilities
- Derives L2 state from L1 data inside fault proof VM
- Provides preimage oracle for trace generation
- Supports both single-chain and interop configurations
- Runs offline within cannon/asterisc VMs

## NOT Responsible For
- Online block building or sequencing
- Runtime service operation

## Core Entities

| Entity | Key Fields | Description |
|--------|-----------|-------------|
| Host config | L1RPC, L2RPC, Network, RollupConfig | Offline verification configuration |
| Client | Interop config, claim verification | In-VM claim verification logic |

## Dependencies
- Refer to arch/dependency.md for full dependency details

## Module-Specific Pitfalls

[Pitfall] Unsafe preimage access — pre-image oracle panics on I/O failure; use error returns in critical paths
