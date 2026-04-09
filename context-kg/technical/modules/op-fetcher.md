---
name: "op-fetcher"
description: "Module design for op-fetcher: data fetching service"
---
# op-fetcher Module

## Responsibilities
- Fetches and aggregates data from L1/L2 sources
- Provides data to downstream consumers

## NOT Responsible For
- Data processing or execution logic

## Core Entities

| Entity | Key Fields | Description |
|--------|-----------|-------------|
| Fetcher CLI | cmd/ entry point | Main fetcher service |

## Dependencies
- Refer to arch/dependency.md for full dependency details

## Module-Specific Pitfalls

None identified.
