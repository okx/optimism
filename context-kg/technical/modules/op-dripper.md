---
name: "op-dripper"
description: "Module design for op-dripper: token drip faucet service"
---
# op-dripper Module

## Responsibilities
- Periodically drips tokens to configured addresses
- Service lifecycle management

## NOT Responsible For
- Production operations (testnet/operational only)

## Core Entities

| Entity | Key Fields | Description |
|--------|-----------|-------------|
| DripperService | config, service lifecycle | Token drip service |

## Dependencies
- Refer to arch/dependency.md for full dependency details

## Module-Specific Pitfalls

None identified.
