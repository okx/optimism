---
name: "op-chain-ops"
description: "Module design for op-chain-ops: chain operations and genesis utilities"
---
# op-chain-ops Module

## Responsibilities
- Genesis state generation and configuration
- Cross-domain withdrawal message handling
- Migration utilities (Canyon, Ecotone, Fjord, Jovian checks)
- Deployment config validation
- Solidity compiler integration
- Source map utilities

## NOT Responsible For
- Runtime execution (tooling only)
- Service management

## Core Entities

| Entity | Key Fields | Description |
|--------|-----------|-------------|
| DeployConfig | genesis/config.go | Chain deployment configuration |
| WithdrawalMessage | crossdomain/types.go | Cross-domain withdrawal interface |

## Dependencies
- Refer to arch/dependency.md for full dependency details

## Module-Specific Pitfalls

None identified.
