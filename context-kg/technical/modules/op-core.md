---
name: "op-core"
description: "Module design for op-core: core types, fork definitions, predeploy addresses"
---
# op-core Module

## Responsibilities
- Defines hardfork names and activation logic (Canyon, Delta, Ecotone, Fjord, Granite, Holocene, Isthmus, Jovian, Interop)
- Defines predeploy contract addresses and proxy configurations
- Provides deployment configuration interfaces

## NOT Responsible For
- Runtime logic (type definitions and constants only)

## Core Entities

| Entity | Key Fields | Description |
|--------|-----------|-------------|
| forks.Name | String type | Hardfork name identifier |
| Predeploy | Address, ProxyEnabled, EnabledCondition | Smart contract predeploy definition |
| DeployConfig | Interface | Checks governance and fork timing |

## Dependencies
- Refer to arch/dependency.md for full dependency details

## Module-Specific Pitfalls

None identified — this module contains only type definitions and constants.
