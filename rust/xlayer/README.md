# xlayer-node

Single binary that fuses **reth** (execution) and **kona** (consensus) into one process, replacing the HTTP Engine API with in-process async channels.

Built on the OP Stack. 1-second blocks. Derives finality from Ethereum L1.

> **To run the devnet, see [DEVNET.md](DEVNET.md).**

---

## Why

Standard OP Stack runs two processes over HTTP:

```
op-node  ──HTTP Engine API (JWT)──►  op-reth
```

xlayer-node collapses them:

```
┌─────────────────────────────────────────────┐
│               xlayer-node                   │
│                                             │
│   kona ──ChannelEngineClient──► reth        │
│   (consensus)    (in-process)   (execution) │
└─────────────────────────────────────────────┘
```

- **No network** — calls go directly to `ConsensusEngineHandle` and `PayloadBuilderHandle`
- **No serialization** — plain async Rust method calls
- **No JWT** — meaningless between two halves of the same binary

---

## Architecture

```
┌──────────────────────── xlayer-node process ────────────────────────┐
│                                                                      │
│  ┌────────────────────┐  ChannelEngineClient  ┌──────────────────┐  │
│  │ kona (consensus)   │ ───────────────────►  │ reth (execution) │  │
│  │                    │  engine_handle         │                  │  │
│  │ L1 derivation      │  payload_handle        │ EVM + state      │  │
│  │ sequencer          │  (tokio channels)      │ payload builder  │  │
│  │ P2P gossip :9223   │ ◄─────────────────── │ mempool :8123    │  │
│  │ rollup RPC :9545   │  l2_provider (HTTP)    │                  │  │
│  └────────────────────┘                        └──────────────────┘  │
│           │                                                          │
└───────────│──────────────────────────────────────────────────────────┘
            │ l1_provider (HTTP)
            ▼
  L1 geth :8545  +  L1 beacon :3500       op-batcher (Docker)
```

### Per-block cycle (4 Engine API calls, all in-process)

| # | Task | Engine API call | What happens |
|---|------|----------------|--------------|
| 1 | Build | `fork_choice_updated` + attrs | kona tells reth to start building |
| 2 | Seal | `get_payload` | kona retrieves the built block |
| 3 | Insert | `new_payload` | kona sends block back for validation |
| 4 | Sync | `fork_choice_updated` (no attrs) | kona advances head/safe/finalized |

### Components

| Component | Where | Purpose |
|-----------|-------|---------|
| reth | in-process | EVM execution, state, RPC, payload building |
| kona | in-process | L1 derivation, sequencing, P2P gossip |
| ChannelEngineClient | in-process | Type transforms + dispatch to reth handles (~250 LOC) |
| L1 geth + Prysm | Docker | Ethereum L1 devnet (PoS, 4 validators) |
| op-batcher | Docker | Compresses L2 blocks, posts batches to L1 |

### Ports

| Port | Service | Purpose |
|------|---------|---------|
| 8123 | reth | L2 JSON-RPC |
| 9545 | kona | Rollup RPC (`optimism_syncStatus`) |
| 8552 | reth | Engine API (unused — kona uses channels) |
| 8545 | L1 geth | L1 JSON-RPC |
| 3500 | L1 beacon | Beacon REST API |
| 8548 | op-batcher | Admin RPC |

---

## Code

```
bin/xlayer-node/src/main.rs          entry point: boot reth → extract handles →
                                     create ChannelEngineClient → launch kona

crates/engine-bridge/src/client.rs   ChannelEngineClient: implements EngineClient
                                     trait via reth's ConsensusEngineHandle +
                                     PayloadBuilderHandle (~250 lines)

crates/engine-bridge/src/error.rs    error types
```

### Boot sequence (main.rs)

```
1. reth launches → returns handle
2. Extract:  engine_handle  = reth_handle.beacon_engine_handle
             payload_handle = reth_handle.payload_builder_handle
3. ChannelEngineClient::new(engine_handle, payload_handle, ...)
4. rollup_node.start_with_client(engine_client)
5. tokio::select! { reth_exit, kona_exit, ctrl_c }
```

### Dependencies

The workspace depends on sibling repos within okx-optimism:

```
okx-optimism/rust/
├── kona/          ← kona fork (EngineClient trait, node service)
├── op-alloy/      ← op-alloy fork (OP types)
└── xlayer/        ← this workspace

External (sibling of okx-optimism):
└── okx-reth/      ← reth fork (ConsensusEngineHandle, PayloadBuilderHandle)
```
