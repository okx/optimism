---
name: "op-geth"
description: "Module design for op-geth: modified go-ethereum execution layer for L2"
---
# op-geth Module

## Responsibilities
- L2 block execution via modified go-ethereum
- Engine API implementation (engine_newPayload, engine_forkchoiceUpdated, engine_getPayload)
- Transaction pool management
- State storage and trie management
- P2P networking (separate from op-node P2P)
- X Layer custom configurations and Apollo config integration

## NOT Responsible For
- Rollup derivation logic (op-node)
- Batch submission (op-batcher)
- Consensus beyond engine API (op-conductor)

## Core Entities

| Entity | Key Fields | Description |
|--------|-----------|-------------|
| EthConfig | NetworkId, SyncMode, TxPool | Ethereum node configuration |
| Blockchain | State trie, block storage | Core chain state management |

## Dependencies
- Refer to arch/dependency.md for full dependency details

## Module-Specific Pitfalls

[Pitfall] X Layer chains have hardcoded fork times that override rollup config — network detection by ChainID
[Warning] okpay/config.go and xlayer/ directories contain custom extensions — review for compatibility
