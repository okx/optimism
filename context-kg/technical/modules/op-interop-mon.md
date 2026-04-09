---
name: "op-interop-mon"
description: "Module design for op-interop-mon: cross-chain interop monitoring"
---
# op-interop-mon Module

## Responsibilities
- Monitors cross-chain message delivery and finality consistency
- Tracks chain health across interop chains
- Monitoring only — no active participation

## NOT Responsible For
- Execution, derivation, or active chain operations

## Core Entities

| Entity | Key Fields | Description |
|--------|-----------|-------------|
| MonitorService | config, metrics | Monitoring service lifecycle |

## Dependencies
- Refer to arch/dependency.md for full dependency details

## Module-Specific Pitfalls

None identified.
