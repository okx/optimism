---
name: "contracts-bedrock"
description: "Module design for contracts-bedrock: Solidity smart contracts for OP Stack"
---
# contracts-bedrock Module

## Responsibilities
- Solidity smart contracts for OP Stack protocol
- DisputeGameFactory, L2OutputOracle, OptimismPortal
- Bridge contracts for L1↔L2 messaging
- Predeploy contracts deployed at genesis
- Foundry-based build and test system

## NOT Responsible For
- Go service logic
- Runtime operations

## Core Entities

| Entity | Key Fields | Description |
|--------|-----------|-------------|
| DisputeGameFactory | Game creation, bond management | Factory for creating dispute games |
| L2OutputOracle | Output root storage | Legacy L2 output proposal target |
| OptimismPortal | Deposits, withdrawals | Cross-domain messaging portal |

## Dependencies
- Refer to arch/dependency.md for full dependency details

## Module-Specific Pitfalls

None identified — Solidity contracts have separate testing patterns.
