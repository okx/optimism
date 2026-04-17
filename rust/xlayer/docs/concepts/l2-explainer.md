# L2 Node Architecture — What All These URLs Are

If you've stared at the docker-compose or xlayer configs and wondered "why are there 10 different
L2 URLs?" — this explains every single one and why it exists.

---

## The Core Confusion: Every L2 Node Has TWO Ports

An OP Stack L2 node is not one thing. It is always two software components working together:

```
┌────────────────────────────────────────────────┐
│              One L2 "node"                     │
│                                                │
│  ┌──────────────────────┐                      │
│  │  Execution Layer (EL)│ ← runs the EVM,      │
│  │  (reth or op-geth)   │   stores state,      │
│  │  Port: :8545 inside  │   has the txpool     │
│  └──────────┬───────────┘                      │
│             │ Engine API (JWT, internal)        │
│  ┌──────────▼───────────┐                      │
│  │  Consensus Layer (CL)│ ← talks to L1,       │
│  │  (kona-node/op-node) │   derives L2 blocks, │
│  │  Port: :9545 inside  │   runs the sequencer │
│  └──────────────────────┘                      │
└────────────────────────────────────────────────┘
```

**EL port** (`eth_*`) = standard Ethereum JSON-RPC. Wallets, dapps, cast all talk here.
**CL port** (`optimism_*`) = rollup-specific RPC. Operators, batcher, proposer talk here.

Every L2 node in the devnet exposes both. That's why you see pairs of ports everywhere.

---

## The Four Types of L2 Node

```
                    L1 (Ethereum devnet)
                          │
                          │ derives L2 blocks from L1
                          ▼
              ┌───────────────────────┐
              │   SEQUENCER (op-seq) │  ← ONE active at a time
              │   Produces blocks    │     writes to L1 via batcher
              │   EL: :8123 (host)  │
              │   CL: :9545 (host)  │
              └───────────┬──────────┘
                          │ P2P gossip (unsafe blocks)
          ┌───────────────┼───────────────┐
          ▼               ▼               ▼
   ┌─────────────┐ ┌─────────────┐ ┌──────────────────┐
   │  RPC NODE   │ │  STANDBY    │ │  RPC NODE 2      │
   │  (op-rpc)   │ │  SEQUENCER  │ │  (op-rpc2)       │
   │  Read-only  │ │  (op-seq2,3)│ │  Read-only       │
   │  EL: :8124  │ │  Dormant,   │ │  EL: :8128       │
   │  CL: :9555  │ │  takes over │ │  CL: :9565       │
   └─────────────┘ │  if primary │ └──────────────────┘
                   │  fails      │
                   └─────────────┘
```

---

## Every L2 URL, Explained

### 1. `:8123` — The Main L2 Endpoint (end user URL)

```
Host:      http://localhost:8123
Inside:    http://op-seq:8545
xlayer-node startup: --http.port 8123
```

**What it is**: op-reth's HTTP JSON-RPC, mapped to host port 8123.
The internal port is always 8545 (reth default) but it's exposed as 8123 on the host to avoid clashing with L1 geth which also uses 8545.

**Who uses it**: Everyone. MetaMask, cast, dapps, op-batcher, op-challenger, ethers.js. If you're submitting a transaction or reading chain state, this is the URL.

**APIs exposed**: `eth_*`, `net_*`, `web3_*`, `txpool_*`, `debug_*`, `miner_*`, `admin_*`

```bash
# Examples of what goes to :8123
cast send --rpc-url http://localhost:8123 ...       # submit TX
cast balance --rpc-url http://localhost:8123 ...   # read balance
cast block --rpc-url http://localhost:8123 latest  # read block
```

---

### 2. `:9545` — The Rollup RPC (operator URL)

```
Host:      http://localhost:9545
Inside:    http://op-seq:9545
xlayer-node startup: --rpc.port 9545 (kona-node)
xlayer-test-config.toml: rpc_port = 9545
```

**What it is**: kona-node's (or op-node's) rollup JSON-RPC. This is NOT the EVM. This is the consensus layer exposing OP Stack-specific methods.

**Who uses it**: op-batcher (polls rollup RPC for safe head), op-proposer (output roots), op-challenger, monitoring scripts, operators checking sync status.

**NOT for end users.** A wallet cannot submit transactions here. It doesn't understand `eth_sendRawTransaction`.

**APIs exposed**: `optimism_*`, `opp2p_*`, `admin_*`

```bash
# Examples of what goes to :9545
curl -X POST http://localhost:9545 -d '{"method":"optimism_syncStatus",...}'  # sync heads
curl -X POST http://localhost:9545 -d '{"method":"admin_stopSequencer",...}'  # stop sequencer
curl -X POST http://localhost:9545 -d '{"method":"opp2p_peers",...}'          # P2P peers
```

---

### 3. `:8124` — RPC Node 1 EL (scale-out read endpoint)

```
Host:      http://localhost:8124
Container: op-geth-rpc or op-reth-rpc (depends on RPC_TYPE in .env)
Inside:    op-${RPC_TYPE}-rpc:8545
```

**What it is**: A separate non-sequencing reth/geth node that follows the sequencer via P2P. Same chain, same state, but it doesn't produce blocks.

**Why it exists**: To offload read traffic from the sequencer. In production, heavy `eth_call`, `debug_trace`, `eth_getLogs` queries would compete with block building on the sequencer. The RPC node absorbs that load.

**APIs**: Same as `:8123` (full `eth_*` etc.) but no block building.

```bash
# Use :8124 for heavy queries that shouldn't slow the sequencer
cast call --rpc-url http://localhost:8124 <contract> "balanceOf(address)" <addr>
curl -X POST http://localhost:8124 -d '{"method":"debug_traceTransaction",...}'
```

---

### 4. `:7546` — L2 WebSocket (real-time subscriptions)

```
Host:      ws://localhost:7546
Container: op-seq:7546 (reth --ws.port 7546)
```

**What it is**: WebSocket endpoint on the sequencer's reth. Used for real-time event streaming.

**Who uses it**: Applications that need push notifications — `eth_subscribe newHeads`, `eth_subscribe logs`, mempool watchers. HTTP polling is fine for occasional queries; WebSocket is needed for low-latency event-driven apps.

**Note**: Port 7546 (not 8546) to avoid clash with L1 geth's WebSocket on 8546.

```javascript
// Example: subscribe to new L2 blocks
const ws = new WebSocket('ws://localhost:7546');
ws.send('{"method":"eth_subscribe","params":["newHeads"]}');
```

---

### 5. `:8552` — Engine API (INTERNAL ONLY — never exposed to host in docker mode)

```
Inside container: 127.0.0.1:8552 (loopback only)
xlayer-node:     --authrpc.port 8552
xlayer-seq.sh:   --authrpc.addr=127.0.0.1 --authrpc.port=8552
```

**What it is**: The private channel between the Consensus Layer (kona-node/op-node) and the Execution Layer (reth/geth). Uses JWT authentication.

**Calls that go here**: `engine_newPayloadV3/V4`, `engine_forkchoiceUpdatedV2/V3`, `engine_getPayloadV3/V4`

**Who can call it**: Only kona-node / op-node. Nobody else. Never expose this port.

**In xlayer-node single binary**: This port still exists for reth's Engine API listener, but `ChannelEngineClient` bypasses it entirely using in-process handles. The port is only reachable via localhost and is effectively unused by kona in single-binary mode.

**In docker mode (xlayer-seq.sh)**: kona-node connects here over loopback. Still not exposed to the host network.

---

### 6. `:9555` / `:9565` — Verifier op-node RPCs

```
:9555  →  op-rpc  (verifier node for op-geth-rpc / op-reth-rpc)
:9565  →  op-rpc2 (verifier node for second RPC node)
```

**What it is**: Each RPC node (`:8124`, `:8128`) has its own op-node/kona-node pair on a different port. These are `sequencer.enabled=false` — they follow and verify, never produce.

**Why different from `:9545`**: Port 9545 is the SEQUENCER's kona-node. Ports 9555/9565 are the RPC nodes' verifier op-nodes. Different nodes, different instances.

**Who uses it**: Rarely needed directly. The RPC nodes derive their canonical chain through their own op-node, which connects to the sequencer's op-node via P2P.

---

### 7. `:8128` — RPC Node 2 EL

Same as `:8124` but a second independent RPC node for redundancy or additional load distribution.

---

### 8. `:8223`, `:8323` — Standby Sequencer EL RPCs

```
:8223  →  op-geth-seq2 or op-reth-seq2 (seq2 EL)
:8323  →  op-geth-seq3 (seq3 EL, always geth)
```

**What they are**: The execution layer of standby sequencers. These run with `--sequencer.stopped` — they are ready to take over if the primary sequencer fails. In devnet, they're dormant.

**Why they exist**: op-conductor (HA mode) manages failover. When `CONDUCTOR_ENABLED=true`, the conductor watches health and promotes seq2 or seq3 if seq1 goes down.

**In devnet (`CONDUCTOR_ENABLED=false`)**: These don't start at all in MIN_RUN mode.

---

### 9. `:9546`, `:9547` — Standby Sequencer CL RPCs

Same concept as `:8223`/`:8323` but the consensus layer (op-node) ports for seq2 and seq3.

---

## The URL Confusion in xlayer-test-config.toml

```toml
l1_rpc_url = "http://localhost:8545"   # L1 geth
l2_rpc_url = "http://localhost:8123"   # reth's OWN HTTP RPC port
```

`l2_rpc_url` is NOT the Engine API. It is reth's public HTTP JSON-RPC. kona uses it to do L2 block lookups (e.g. `eth_getBlockByNumber` to find the safe head). These are standard read queries, not block production calls.

**Concrete example of what flows over `l2_rpc_url`**:
```
kona calls:  get_block_by_number("latest")
  → hits :8123
  → reth answers with the latest L2 block
  → kona uses this to track current L2 state
```

---

## URL Reference by Use Case

| I want to... | Use this URL |
|--------------|-------------|
| Submit a transaction | `:8123` |
| Check an account balance | `:8123` |
| Read a contract (eth_call) | `:8123` (or `:8124` for heavy calls) |
| Subscribe to new blocks (WebSocket) | `ws://localhost:7546` |
| Check unsafe/safe/finalized head | `:9545` → `optimism_syncStatus` |
| Start/stop the sequencer | `:9545` → `admin_startSequencer` |
| Check P2P peers | `:9545` → `opp2p_peers` |
| Run heavy debug traces | `:8124` (don't hit the sequencer) |
| Configure batcher DA limits | `:8123` → `miner_setMaxDASize` |
| Check conductor HA status | `:8547` |
| Watch Prometheus metrics | `:9090` |

---

## Why So Many Nodes at All?

In a real production deployment there would be even more, because:

1. **Sequencer EL** `:8123` — block production, needs to be fast and uninterrupted
2. **Sequencer CL** `:9545` — L1 derivation, P2P gossip leader
3. **RPC nodes** `:8124`, `:8128` — read traffic isolation (indexers, dapps, explorers)
4. **Standby sequencers** `:8223`, `:8323` — high availability, conductor-managed failover
5. **Verifier op-nodes** `:9555`, `:9565` — each RPC node needs its own consensus follower

The devnet runs all of these on one machine for testing. In production they'd be separate machines in different data centres.

---

## In the xlayer-node Single Binary

When running `xlayer-node` locally (not Docker), only two L2 endpoints exist:

```
:8123  →  reth's ETH RPC     (--http.port 8123)
:9545  →  kona-node rollup RPC (rpc_port in xlayer-test-config.toml)
```

All the other ports (8124, 8128, 9555, etc.) are Docker-only — they're RPC and standby nodes from the xlayer-toolkit devnet. They don't exist when running the single binary directly.
