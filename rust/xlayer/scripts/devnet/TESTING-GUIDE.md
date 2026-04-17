# XLayer Load Testing — Complete Guide

> Single source of truth. Replaces `HOW-TO-RUN-LOAD-TEST.md` in both repos.

<!-- ─── SET THESE TO YOUR LOCAL CHECKOUT PATHS ─── -->
<!-- XLAYER_NODE_ROOT: path to the xlayer-node repo (this repo's rust/xlayer/ directory) -->
<!-- XLAYER_TOOLKIT_ROOT: path to the xlayer-toolkit repo checkout -->
<!--
   All commands below use these two placeholders:
     <XLAYER_NODE_ROOT>     — e.g. /home/you/repos/okx-optimism/rust/xlayer
     <XLAYER_TOOLKIT_ROOT>  — e.g. /home/you/repos/xlayer-toolkit
   Find-and-replace them with your actual paths before running any commands.
-->

---

## Before You Start — Branch Checklist

| Repo | Branch | Why |
|------|--------|-----|
| **xlayer-node** (this repo) | `master` | Load test script and node binary live here |
| **xlayer-toolkit** | `feature/latency-metrics` | Has the `devnet/load-test.sh` that generated the reference benchmark numbers |

### Verify your branches before anything else

```bash
# xlayer-node repo — must be on master
cd <XLAYER_NODE_ROOT>
git branch --show-current        # should print: master
git status                       # should be clean

# xlayer-toolkit repo — must be on feature/latency-metrics for toolkit comparison
cd <XLAYER_TOOLKIT_ROOT>
git branch --show-current        # should print: feature/latency-metrics
git checkout feature/latency-metrics   # if not already on it
git status                       # should be clean
```

**Why `feature/latency-metrics`?**
This is the branch that contains the version of `devnet/load-test.sh` used to produce the
reference toolkit numbers in the benchmark table below. Other branches (e.g.
`feature/kona-cl-hypothesis`) have a different version of that script which targets a
different setup and will not produce comparable results.

**If you only run the xlayer-node benchmark** (no toolkit comparison), the xlayer-toolkit
branch does not matter — you just need to stop its containers:
```bash
cd <XLAYER_TOOLKIT_ROOT>/devnet
docker compose down --remove-orphans
```

### First-time setup — build the binary

If `target/release/xlayer-node` does not exist yet, build it first (takes ~5 min):
```bash
cd <XLAYER_NODE_ROOT>
cargo build --release -p xlayer-node
```
After the binary exists, always use `--no-build` with `start-all.sh` to skip rebuild.

### Pre-run checklist

```bash
# 1. No process holding ports 8123 or 8552
lsof -i :8123 -i :8552    # must return nothing

# 2. Binary exists
ls -lh target/release/xlayer-node

# 3. Docker is running
docker ps
```

---

## Mental Model First

```
┌──────────────────────────────────────────────────────────────┐
│  SHARED L1  (same Docker containers, same volumes, forever)  │
│  l1-geth  l1-beacon-chain  l1-validator                      │
│  ports: 8545, 8546, 8551, 3500, 4000                        │
└──────────────┬───────────────────────┬───────────────────────┘
               │                       │
   xlayer-toolkit                xlayer-node
   op-reth + kona-node           single binary (kona + reth)
   Docker containers             host process
   ports 8123, 8552              ports 8123, 8552
```

Both stacks were deployed against the **same L1** with the **same OP contract addresses**.
Both bind `8123` (L2 RPC) and `8552` (authrpc) on the host — only one can run at a time.

**The L1 containers (`l1-geth`, `l1-beacon-chain`, `l1-validator`) have identical names and
ports in both docker-compose files. They are shared infrastructure — not separate instances.**

### What to wipe and when

| Data | Location | Wipe when? | Why |
|------|----------|------------|-----|
| xlayer-node L2 data | `/tmp/xlayer-data` | Before each xlayer-node test run | Fresh baseline |
| xlayer-toolkit L2 data | Docker volumes inside containers | `docker compose down` handles it | Fresh baseline |
| Node logs | `logs/` | Optional, handled by reset-l2.sh | Diagnostic only |
| **L1 execution data** | `docker/l1/execution/geth` | **NEVER** | genesis.json anchors to these block hashes |
| **L1 consensus data** | `docker/l1/consensus/beacondata` | **NEVER** | Same reason |

**L1 redeployment** (`redeploy-op-contracts.sh`) is **not** part of normal switching.
It is a one-time recovery procedure for when L1 volumes are accidentally deleted.

---

## L1 Architecture: Two Separate Chains

Both stacks define containers named `l1-geth`, `l1-beacon-chain`, `l1-validator` on host port 8545.
Only one set can be running at a time. Each stack has its own L1 volume path and its own genesis:

```
xlayer-toolkit L1 volumes:  xlayer-toolkit/devnet/l1-geth/        (toolkit's own L1 chain)
xlayer-node    L1 volumes:  xlayer-node/docker/l1/                (xlayer-node's own L1 chain)
```

`genesis.json` in each stack was generated against that stack's own L1 deployment.
They are not cross-compatible. Never mix them.

**Stopping L1 containers is safe** — volumes are preserved, restart picks up from the same block.
**Deleting L1 volume directories is catastrophic** — `genesis.json` becomes invalid.

---

## Running Tests

### xlayer-node

Expected total time: ~4 min (60s load + ~90s safe-head wait + startup).
Report saved to: `load-test-YYYYMMDD_HHMMSS.txt` in the xlayer-node repo root.

```bash
# 1. Stop xlayer-toolkit's stack (including its L1)
cd <XLAYER_TOOLKIT_ROOT>/devnet
docker compose down --remove-orphans    # stops toolkit L1 + app, volumes preserved

# 2. Reset xlayer-node L2 data (L1 data untouched — different directory)
cd <XLAYER_NODE_ROOT>
./scripts/devnet/maintenance/reset-l2.sh

# 3. Start xlayer-node's L1 + node + batcher (skip rebuild — binary already built)
./scripts/devnet/start-all.sh --no-build

# 4. Verify blocks are advancing (run twice — number must increment between calls)
cast bn --rpc-url http://localhost:8123
sleep 3
cast bn --rpc-url http://localhost:8123

# 5. Verify ENGINE BRIDGE logging is active (must return lines — if empty, see Recovery)
grep "engine_bridge" logs/xlayer-node.log | head -3

# 6. Run the benchmark
./scripts/devnet/load-test.sh
```

### xlayer-toolkit

> **Reminder:** ensure xlayer-toolkit is on `feature/latency-metrics` before running.
> See [Branch Checklist](#before-you-start--branch-checklist) above.

Expected total time: ~4 min (60s load + ~90s safe-head wait + startup).
Report saved to: `devnet/load-test-YYYYMMDD_HHMMSS.txt` in the xlayer-toolkit repo.

```bash
# 1. Stop xlayer-node (stop-all.sh leaves L1 running — stop it explicitly too)
cd <XLAYER_NODE_ROOT>
./scripts/devnet/stop-all.sh           # stops node + batcher
docker compose -f docker/docker-compose.devnet.yml stop l1-geth l1-beacon-chain l1-validator

# 2. Ensure xlayer-toolkit is on the right branch
cd <XLAYER_TOOLKIT_ROOT>
git checkout feature/latency-metrics

# 3. Start xlayer-toolkit's stack (its own L1 starts automatically)
cd devnet
docker compose down --remove-orphans   # clean up any orphan containers
./4-op-start-service.sh               # starts toolkit L1 + op-reth + kona-node

# 4. Verify blocks are advancing (takes 30–60s to start)
cast bn --rpc-url http://localhost:8123
sleep 3
cast bn --rpc-url http://localhost:8123

# 5. Run the benchmark
./load-test.sh
```

### Optional flags (both scripts)
```bash
./load-test.sh --duration 120      # longer run for more stable averages
./load-test.sh --concurrency 4     # fewer workers (default: 6)
./load-test.sh --no-engine-probe   # skip authrpc FCU probe (xlayer-node only)
./load-test.sh --out my-run.txt    # custom report file path
```

---

## Full Comparison Workflow — Both Stacks in One Session

Run this sequence to produce a directly comparable pair of reports.

```bash
# ── Step 1: xlayer-node run ────────────────────────────────────────────────────
cd <XLAYER_TOOLKIT_ROOT>/devnet
docker compose down --remove-orphans

cd <XLAYER_NODE_ROOT>
./scripts/devnet/maintenance/reset-l2.sh
./scripts/devnet/start-all.sh --no-build
# wait for blocks + ENGINE BRIDGE lines in log, then:
./scripts/devnet/load-test.sh
# note the report filename printed at the end, e.g. load-test-20260325_231451.txt

# ── Step 2: xlayer-toolkit run ─────────────────────────────────────────────────
./scripts/devnet/stop-all.sh
docker compose -f docker/docker-compose.devnet.yml stop l1-geth l1-beacon-chain l1-validator

cd <XLAYER_TOOLKIT_ROOT>
git checkout feature/latency-metrics
cd devnet
docker compose down --remove-orphans
./4-op-start-service.sh
# wait 30–60s for blocks, then:
./load-test.sh
# note the report filename, e.g. load-test-20260325_210631.txt

# ── Step 3: Compare ────────────────────────────────────────────────────────────
# xlayer-node report:  xlayer-node/load-test-YYYYMMDD_HHMMSS.txt
# toolkit report:      xlayer-toolkit/devnet/load-test-YYYYMMDD_HHMMSS.txt
#
# Key sections to compare:
#   THROUGHPUT          → Effective TPS
#   TX INCLUSION LATENCY → p50 / p99
#   L2 HEADS            → Safe lag increase (batcher health)
#   ENGINE BRIDGE       → FCU+attrs / new_payload (xlayer-node only)
#   HTTP ENGINE API     → FCU baseline avg (toolkit only)
```

---

## Benchmark Results (reference run, 2026-03-25)

| Metric | xlayer-toolkit | xlayer-node | Note |
|--------|---------------|-------------|------|
| Effective TPS | 72.6 TX/s | 73.7 TX/s | |
| TX inclusion p50 | 1511 ms | 1447 ms | |
| TX inclusion p99 | 2013 ms | 2006 ms | |
| Avg block time | 1.0 s | 1.0 s | |
| Peak TXs / block | 281 | 149 | |
| Safe lag increase | −301 blks | +0 blks | xlayer-node batcher is healthier |
| FCU latency avg (HTTP probe) | 1.66 ms | 0.55 ms | loopback vs container HTTP |
| **FCU+attrs avg (kona→reth)** | N/A | **0.874 ms** (p50: 0.191 ms) | ENGINE BRIDGE — in-process channel |
| **new_payload avg (kona→reth)** | N/A | **2.202 ms** (p50: 0.330 ms) | ENGINE BRIDGE — in-process channel |

ENGINE BRIDGE is the key differentiator. It measures the actual cost of kona delivering a
block to reth over a tokio channel — no HTTP, no serialization. xlayer-toolkit has no equivalent.

---

## Recovery Runbook

### `ResetForkchoiceError("Block not found: 0x...")`

**What happened**: L1 volumes were wiped. The fresh L1 has different block hashes than
what `genesis.json` was built against. kona looks up the L2 genesis anchor on L1 and
finds nothing.

**Fix** (takes ~5 min):
```bash
cd <XLAYER_NODE_ROOT>

# 1. Make sure L1 is running (it starts fresh from block 1 after a wipe)
cast bn --rpc-url http://localhost:8545    # should return a number

# If L1 is NOT running:
docker compose -f docker/docker-compose.devnet.yml up -d l1-geth l1-beacon-chain l1-validator
sleep 30
cast bn --rpc-url http://localhost:8545

# 2. Redeploy OP contracts on fresh L1, regenerates genesis.json + rollup.json
DOCKER_NETWORK=xlayer-devnet ./scripts/devnet/maintenance/redeploy-op-contracts.sh

# 3. Start the node
./scripts/devnet/start-all.sh --no-build
```

### Safe lag FAIL (Δ > 50 blocks)

Batcher fell behind during the test. Reset L2 and rerun:
```bash
./scripts/devnet/stop-all.sh
./scripts/devnet/maintenance/reset-l2.sh
./scripts/devnet/start-all.sh --no-build
# Wait for cast bn to increment, then:
./scripts/devnet/load-test.sh
```

### Port already in use (8123 or 8552)

```bash
lsof -i :8123 -i :8552    # see what's holding it

# If xlayer-toolkit containers:
cd <XLAYER_TOOLKIT_ROOT>/devnet
docker compose down --remove-orphans

# If stray xlayer-node binary:
pkill -f xlayer-node

# If stray op-batcher:
cd <XLAYER_NODE_ROOT>
docker compose -f docker/docker-compose.devnet.yml stop op-batcher
```

### Block rate stuck at 0 blocks/5s

The load test blocks here intentionally — it will not run while the chain isn't producing.

Check what's happening:
```bash
# Is the node in fast-sync? (shows 200+ blocks/5s, wait it out)
tail -f logs/xlayer-node.log | grep -E "block|sync|error"

# Is the node dead?
ps aux | grep xlayer-node | grep -v grep

# Recovery if node is not running:
./scripts/devnet/stop-all.sh
./scripts/devnet/start-all.sh --no-build
```

### ENGINE BRIDGE section missing from report

`engine_bridge=debug` not active. Check:
```bash
grep "engine_bridge" logs/xlayer-node.log | head -5
# If empty, the node was started without the log level set.
# start-all.sh now sets XLAYER_LOG_LEVEL=info,engine_bridge=debug by default.
# Restart the node:
./scripts/devnet/stop-all.sh
./scripts/devnet/start-all.sh --no-build
```

---

## Incident Postmortem — How We Broke L1 (2026-03-25)

### What happened

1. xlayer-toolkit load test ran successfully (11/11 PASS).
2. To switch to xlayer-node, ran `docker compose down --remove-orphans` on xlayer-toolkit. ✅
   - This correctly stopped toolkit's L1 containers (volumes preserved).
3. xlayer-node's `start-all.sh` was not run to bring up its own L1 — instead, recovery
   attempts escalated and someone ran:
   ```bash
   rm -rf docker/l1/execution/geth
   rm -rf docker/l1/consensus/beacondata
   ```
   **This was the fatal step.** ❌ This deleted xlayer-node's L1 volume directories.
4. L1 restarted at block 1 with entirely new block hashes.
5. `genesis.json` contains `l1.hash` — the hash of the specific L1 block where OP contracts
   were deployed. That block no longer exists on the fresh L1.
6. xlayer-node crashed on startup: `ResetForkchoiceError("Block not found: 0x45f...")`

### Why recovery was slow

`redeploy-op-contracts.sh` failed silently at step 1 with exit code 125. The cause:
the script defaulted to `DOCKER_NETWORK=dev-op` but the actual network is `xlayer-devnet`.
Docker couldn't find the network, the container failed to start, but `set -e` exited before
printing a useful message. The correct invocation:
```bash
DOCKER_NETWORK=xlayer-devnet ./scripts/devnet/maintenance/redeploy-op-contracts.sh
```
was not documented anywhere.

### Permanent fixes applied

| Fix | File |
|-----|------|
| Default `DOCKER_NETWORK` corrected to `xlayer-devnet` | `maintenance/redeploy-op-contracts.sh` |
| `--log.file.directory logs/reth` added to node startup | `start-all.sh`, `internal/start-node.sh` |
| `XLAYER_LOG_LEVEL` defaults to `info,engine_bridge=debug` | `start-all.sh`, `internal/start-node.sh` |
| This guide written | `scripts/devnet/TESTING-GUIDE.md` |

### What to never do again

```
# ❌ NEVER — wipes L1, breaks genesis.json
rm -rf docker/l1/execution/geth
rm -rf docker/l1/consensus/beacondata
docker compose -f docker/docker-compose.devnet.yml down -v   # the -v flag wipes volumes

# ✅ CORRECT — wipes only L2, L1 survives
./scripts/devnet/maintenance/reset-l2.sh
```

---

## File Map

```
project root: <XLAYER_NODE_ROOT>/

scripts/devnet/
  start-all.sh                  start L1 (if needed) + node + batcher
  stop-all.sh                   stop node + batcher (leaves L1 running)
  load-test.sh                  load test (60s, 6 workers, ENGINE BRIDGE parsed)
  health-check.sh               live dashboard: heads, lag, block rate, peers
  TESTING-GUIDE.md              ← YOU ARE HERE
  maintenance/
    reset-l2.sh                 wipe /tmp/xlayer-data + logs — SAFE
    redeploy-op-contracts.sh    ONLY after L1 wipe: redeploy + regen genesis/rollup
    reset-l1-reconfig.sh        patch rollup.json genesis.l2.hash (called by above)

config/devnet/
  genesis.json                  L2 genesis — contains L1 block hash anchor
  rollup.json                   rollup config — contains genesis.l2.hash
  xlayer-node.toml              node config (ports, rollup path, etc.)

logs/
  xlayer-node.log               stdout (info level)
  reth/195/reth.log             file log (engine_bridge=debug lines — parsed by load-test)

docker/l1/
  execution/geth/               L1 MDBX state — NEVER WIPE
  consensus/beacondata/         L1 beacon state — NEVER WIPE
```


---

## Reset Levels — Safest to Most Destructive

| Operation | What it wipes | When to use |
|-----------|--------------|-------------|
| `reset-l2.sh` | `/tmp/xlayer-data` + logs | Before every xlayer-node run — takes seconds |
| `docker compose down` (no `-v`) | Stops containers, volumes intact | Switching stacks — takes seconds |
| `rm -rf docker/l1/...` + redeploy | Destroys L1 chain entirely | Almost never — takes 5–10 min, requires `redeploy-op-contracts.sh` |

**Never do a fresh L1 redeploy as part of normal stack switching.** Each stack has its own
L1 volume directory. Stopping and restarting containers is always safe — volumes survive.
Redeployment is only needed if you physically deleted the L1 volume directories.

