---
name: "op-alt-da"
description: "Module design for op-alt-da: alternative data availability layer"
---
# op-alt-da Module

## Responsibilities
- Provides alternative DA server for off-chain blob storage
- Serves DA commitments and blob data via HTTP
- Integrates with op-batcher for DA mode selection

## NOT Responsible For
- On-chain blob submission (op-batcher handles mode selection)
- Rollup derivation (op-node)

## Core Entities

| Entity | Key Fields | Description |
|--------|-----------|-------------|
| DAClient | DAServerURL, BasicHTTPClient | HTTP client for DA server communication |
| CLIConfig | DAServerURL, Enabled | DA server configuration |

## Dependencies
- Refer to arch/dependency.md for full dependency details

## Module-Specific Pitfalls

[Pitfall] DAServerURL must be valid and accessible when enabled — URL parsed via net/url; inaccessible server blocks batching
