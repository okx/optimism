---
name: "op-validator"
description: "Module design for op-validator: validator service"
---
# op-validator Module

## Responsibilities
- Validates L2 chain state and outputs
- Provides validation services for chain operators

## NOT Responsible For
- Block execution or sequencing

## Core Entities

| Entity | Key Fields | Description |
|--------|-----------|-------------|
| Validator CLI | cmd/ entry point | Main validator service |

## Dependencies
- Refer to arch/dependency.md for full dependency details

## Module-Specific Pitfalls

None identified.
