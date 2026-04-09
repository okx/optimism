---
name: "op-dispute-mon"
description: "Module design for op-dispute-mon: dispute game monitoring"
---
# op-dispute-mon Module

## Responsibilities
- Observes dispute games and tracks progression
- Monitors bond state and distribution
- Validates honest actor behavior
- Provides monitoring metrics and alerts
- Read-only: does not participate in games

## NOT Responsible For
- Game participation or bond claiming (op-challenger)
- Block execution or sequencing (op-node)

## Core Entities

| Entity | Key Fields | Description |
|--------|-----------|-------------|
| EnrichedGameData | GameMetadata, claims, endpoints, bond info | Complete game analysis data |
| EnrichedClaim | faultTypes.Claim + resolved flag | Enhanced claim with resolution state |
| BidirectionalTree | Claims with parent-child links | Flat claim tree for analysis |

## Dependencies
- Refer to arch/dependency.md for full dependency details

## Module-Specific Pitfalls

None identified beyond general monitoring patterns.
