---
name: "op-faucet"
description: "Module design for op-faucet: testnet faucet service"
---
# op-faucet Module

## Responsibilities
- Distributes testnet tokens to users
- Rate limiting and backend management

## NOT Responsible For
- Production operations (testnet only)

## Core Entities

| Entity | Key Fields | Description |
|--------|-----------|-------------|
| FaucetService | backends, rate limiter | Token distribution service |

## Dependencies
- Refer to arch/dependency.md for full dependency details

## Module-Specific Pitfalls

None identified.
