# XLayer Devnet Port Reference

Source: `xlayer-toolkit/devnet/docker-compose.yml`

---

## L1 Layer

| Container | Host Port | Protocol | Purpose |
|-----------|-----------|----------|---------|
| `l1-geth` | **8545** | HTTP RPC | L1 ETH RPC (`eth,net,web3,debug,admin`) |
| `l1-geth` | **8546** | WS | L1 WebSocket |
| `l1-geth` | **8551** | Engine API | L1 Beacon↔Geth auth RPC (JWT) — internal between containers |
| `l1-beacon-chain` | **3500** | HTTP REST | Beacon REST API (used by kona / op-node) |
| `l1-beacon-chain` | **4000** | gRPC | Prysm internal (validator ↔ beacon) |
| `l1-beacon-chain` | **6060** | HTTP | pprof |
| `l1-beacon-chain` | **18080** | HTTP | Prysm metrics (container :8080 → host :18080) |
| `l1-beacon-chain` | **19090** | HTTP | Prometheus metrics (container :9090 → host :19090) |
| `l1-validator` | *(none)* | — | Talks to beacon on internal network only |

---

## L2 Sequencer (`op-seq` — xlayer-node container)

> `op-seq` runs both op-reth (execution) and kona-node (consensus) in a single container.
> The Engine API between them binds to `127.0.0.1:8552` — loopback only, never exposed outside the container.

| Host Port | Purpose |
|-----------|---------|
| **8123** | L2 public ETH RPC (op-reth listens on :8545 inside; mapped to :8123 on host) |
| **7546** | L2 WebSocket |
| **30303** TCP+UDP | op-reth P2P |
| **9001** | reth Prometheus metrics |
| **11111** | Flashblocks outbound WS (only active when `FLASHBLOCK_ENABLED=true`) |
| **9545** | kona-node rollup RPC (`optimism_syncStatus`, `admin_*`, etc.) |
| **9223** TCP+UDP | kona-node P2P gossip |
| **9002** | kona-node Prometheus metrics |
| *(8552 — not exposed)* | Engine API — `127.0.0.1:8552` inside container only |

---

## L2 RPC Nodes

| Container | Host Port | Purpose |
|-----------|-----------|---------|
| `op-geth-rpc` / `op-reth-rpc` | **8124** | RPC node 1 ETH RPC |
| `op-reth-rpc` | **7547** | RPC node 1 WebSocket |
| `op-reth-rpc` | **11113** | Flashblocks outbound WS (rpc node 1) |
| `op-geth-rpc2` / `op-reth-rpc2` | **8128** | RPC node 2 ETH RPC |
| `op-rpc` (op-node verifier 1) | **9555** | Rollup RPC |
| `op-rpc` | **9233** | P2P |
| `op-rpc2` (op-node verifier 2) | **9565** | Rollup RPC |

---

## Conductor (HA Sequencer Failover — Raft)

| Container | Host Port | Purpose |
|-----------|-----------|---------|
| `op-conductor` | **8547** | Conductor 1 RPC |
| `op-conductor` | **50050** | Conductor 1 Raft consensus |
| `op-conductor2` | **8548** | Conductor 2 RPC |
| `op-conductor2` | **50051** | Conductor 2 Raft consensus |
| `op-conductor3` | **8549** | Conductor 3 RPC |
| `op-conductor3` | **50052** | Conductor 3 Raft consensus |

---

## Secondary Sequencers (`op-seq2`, `op-seq3` — standby)

| Container | Host Port | Purpose |
|-----------|-----------|---------|
| `op-geth-seq2` / `op-reth-seq2` | **8223** | Seq2 EL ETH RPC |
| `op-seq2` (op-node) | **9546** | Seq2 rollup RPC |
| `op-seq2` | **9224** TCP+UDP | Seq2 P2P |
| `op-geth-seq3` | **8323** | Seq3 EL ETH RPC |
| `op-seq3` (op-node) | **9547** | Seq3 rollup RPC |
| `op-seq3` | **9225** TCP+UDP | Seq3 P2P |

---

## Batcher / Proposer

| Container | Port | Notes |
|-----------|------|-------|
| `op-batcher` | 8548 (internal) | Admin RPC — not mapped to host |
| `op-proposer` | 8560 (internal) | RPC — not mapped to host |
| `op-challenger` | *(none)* | No RPC port |

---

## Monitoring

| Container | Host Port | Purpose |
|-----------|-----------|---------|
| `prometheus` | **9090** | Prometheus UI + scrape endpoint |
| `grafana` | **3000** | Grafana dashboard — login: `admin` / `admin` |

---

## Quick Reference — Ports Used Day-to-Day

| Port | Use |
|------|-----|
| `8545` | Send L1 transactions; query L1 state |
| `8123` | Send L2 transactions; query L2 state (`cast`, MetaMask, etc.) |
| `9545` | Query rollup status: `optimism_syncStatus` (unsafe/safe/finalized heads) |
| `3500` | L1 beacon REST — used internally by kona and op-node |
| `9090` | Prometheus — scrape or query metrics directly |
| `3000` | Grafana — TPS, block time, pre-warming metrics |


---                                                                                                                                                                                                                                                                           

## 8123 vs 9545 — The Real Difference

It is NOT submit vs query. Both can query. They speak completely different protocols.

:8123  →  reth's Ethereum JSON-RPC                                                                                                                                                                                                                                            
Speaks: eth_*, net_*, web3_*, txpool_*, debug_*, miner_*                                                                                                                                                                                                          
Knows about: blocks, transactions, accounts, balances, EVM calls
Does NOT know: what "safe head" means, what an output root is

:9545  →  kona-node's Rollup RPC
Speaks: optimism_*, admin_*, opp2p_*
Knows about: L1 derivation, unsafe/safe/finalized heads, sequencer state
Does NOT know: eth_getBalance, eth_sendRawTransaction — these don't exist here

You absolutely CAN query from 8123. Everything in the eth_* namespace:

# ALL of these go to :8123 — queries AND writes
cast balance --rpc-url http://localhost:8123 0xAbc...        # query
cast block latest --rpc-url http://localhost:8123            # query
cast call --rpc-url http://localhost:8123 <contract> ...     # query
cast send --rpc-url http://localhost:8123 --private-key ...  # write/TX

9545 is for things that only the rollup consensus layer knows. You cannot get optimism_syncStatus from 8123 because reth has no idea what "safe head" is — that's kona's domain.

# ONLY available on :9545 — reth doesn't have these
optimism_syncStatus          # unsafe/safe/finalized heads
optimism_outputAtBlock       # output root for dispute games
admin_stopSequencer          # stop block production
opp2p_peers                  # P2P gossip peer list

One-line summary:
8123 = everything Ethereum. 9545 = everything OP Stack rollup. Use 8123 for 95% of things.

  ---
Engine API Port — This Is the Critical One

Port: 8552
Flags: --authrpc.addr 127.0.0.1 --authrpc.port 8552

In Docker (xlayer-seq.sh):

Container interior:
kona-node → http://127.0.0.1:8552 (JWT auth) → reth

Calls that cross this:
engine_newPayloadV4          (kona tells reth: here is a built block)
engine_forkchoiceUpdatedV3   (kona tells reth: this is the new head)
engine_getPayloadV3/V4       (kona asks reth: give me the built payload)

Host machine: port 8552 is NOT mapped → you cannot reach it from outside the container

In xlayer-node single binary:

Port 8552 technically still exists (reth's authrpc listener is still running)
BUT ChannelEngineClient NEVER CONNECTS TO IT.

Instead, kona holds direct Rust handles:
engine_handle  → ConsensusEngineHandle  (in-process mpsc channel)
payload_handle → PayloadBuilderHandle   (in-process mpsc channel)

The same three engine calls happen, just over memory instead of HTTP+JWT:
engine_handle.new_payload(data)                     ← no HTTP
engine_handle.fork_choice_updated(fcu, attrs, ver)  ← no HTTP
payload_handle.resolve_kind(id, WaitForPending)     ← no HTTP

Comparison:

┌────────────────────────┬──────────────────────────┬────────────────────────────────────┐
│                        │  Docker (xlayer-seq.sh)  │    Single binary (xlayer-node)     │
├────────────────────────┼──────────────────────────┼────────────────────────────────────┤
│ Engine API port        │ 127.0.0.1:8552           │ 127.0.0.1:8552 (exists but unused) │
├────────────────────────┼──────────────────────────┼────────────────────────────────────┤
│ How kona talks to reth │ HTTP + JWT over loopback │ In-process Rust channels           │
├────────────────────────┼──────────────────────────┼────────────────────────────────────┤
│ JWT needed             │ Yes                      │ No                                 │
├────────────────────────┼──────────────────────────┼────────────────────────────────────┤
│ Latency                │ ~0.1-0.5ms (loopback)    │ ~0µs (function call)               │
├────────────────────────┼──────────────────────────┼────────────────────────────────────┤
│ Exposed to host        │ Never                    │ Never                              │
└────────────────────────┴──────────────────────────┴────────────────────────────────────┘

  ---
All Three Ports, One Diagram

```
                      ┌─────────────────────────────────┐
                      │        xlayer-node process       │
                      │                                  │
    End users  :8123 ─►  reth (EVM, txpool, state)       │
    Operators  :9545 ─►  kona-node (rollup, L1 derive)   │
                      │         │                         │
                      │         │ in-process channels     │
                      │         │ (ChannelEngineClient)   │
                      │         └──► reth engine tree     │
                      │                                   │
                      │  :8552 exists but kona ignores it │
                      └─────────────────────────────────-─┘

                      ┌─────────────────────────────────┐
                      │       Docker container           │
                      │                                  │
    End users  :8123 ─►  reth :8545                      │
    Operators  :9545 ─►  kona-node :9545                 │
                      │         │                         │
                      │         │ HTTP + JWT              │
                      │         └──► :8552 (loopback)     │
                      │              reth engine API      │
                      │  :8552 NOT mapped to host        │
                      └──────────────────────────────────┘
```



### Useful one-liners

```bash
# L2 block number
cast block-number --rpc-url http://localhost:8123

# Rollup sync status (unsafe / safe / finalized heads)
curl -s -X POST http://localhost:9545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' \
  | jq '{unsafe: .unsafe_l2.number, safe: .safe_l2.number, finalized: .finalized_l2.number}'

# L1 block number
cast block-number --rpc-url http://localhost:8545

# L2 account balance
cast balance --rpc-url http://localhost:8123 <address>

# Check miner namespace is live (needed by op-batcher)
curl -s -X POST http://localhost:8123 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"miner_setMaxDASize","params":["0x0","0x0"]}' \
  | jq .result
```
