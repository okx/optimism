# XLayer Node — Local Devnet

Run a complete XLayer L2 devnet locally (L1 geth + beacon + xlayer-node + op-batcher) in a single command.

---

## Quick Start

```bash
# 1. Clone (if not done)
git clone <repo-url> && cd xlayer

# 2. Install prerequisites (one-time)
#   Rust:    curl https://sh.rustup.rs -sSf | sh
#   Foundry: curl -L https://foundry.paradigm.xyz | bash && foundryup
#   Docker:  https://docs.docker.com/get-docker/
#   jq:      brew install jq

# 3. Make scripts executable (one-time after clone)
chmod +x scripts/devnet/*.sh scripts/devnet/internal/*.sh scripts/devnet/maintenance/*.sh

# 4. Start everything
./scripts/devnet/0-all.sh
```

The script handles the rest: creates `.env`, generates a JWT secret, builds `xlayer-node`, starts L1 (geth + beacon), starts `xlayer-node`, starts `op-batcher`.

**Subsequent runs** (binary already built):
```bash
./scripts/devnet/0-all.sh --no-build
```

---

## Verify It Works

```bash
# Live health dashboard (all components)
./scripts/devnet/health-check.sh

# Single health snapshot
./scripts/devnet/health-check.sh --once

# Send a test transaction and confirm unsafe → safe → finalized
./scripts/devnet/test-tx.sh --verbose
```

Healthy output looks like:
```
L1 (Ethereum)
  l1-geth:       ✅ running  block 450
  l1-beacon:     ✅ running  slot 455  dist=0
  l1-validator:  ✅ running

xlayer-node (L2)
  process:       ✅ running  pid 12345
  unsafe head:   12345  (1.0s/block)
  safe head:     12340  (lag: 5 blocks)
  finalized:     12335  (lag: 10 blocks)

op-batcher
  container:     ✅ running
  admin RPC:     ok
```

---

## Stop / Restart

```bash
# Stop everything (data preserved)
./scripts/devnet/stop-all.sh

# Restart from stopped state (no rebuild)
./scripts/devnet/start-all.sh --no-build

# Rebuild + restart xlayer-node only (L1 never touched)
./scripts/devnet/restart-node.sh

# Reset L2 chain data (wipe + restart fresh — L1 unchanged)
./scripts/devnet/maintenance/reset-l2.sh
./scripts/devnet/start-all.sh --no-build
```

---

## Logs

```bash
tail -f logs/xlayer-node.log                          # live node output
grep -i "error\|warn\|panic" logs/xlayer-node.log     # filter problems
docker compose -f docker/docker-compose.devnet.yml logs op-batcher --tail 50
```

---

## Ports

| Service       | Port  | Purpose                     |
|---------------|-------|-----------------------------|
| L1 geth       | 8545  | L1 JSON-RPC                 |
| L1 beacon     | 3500  | L1 beacon API               |
| xlayer-node   | 8123  | L2 JSON-RPC (ETH API)       |
| xlayer-node   | 9545  | L2 rollup RPC               |
| xlayer-node   | 8552  | Engine API (JWT-auth)       |
| op-batcher    | 8548  | Batcher admin RPC           |
| metrics       | 9001  | Prometheus metrics          |

---

## Common Issues

| Symptom | Fix |
|---------|-----|
| `Missing required tools: cast` | `curl -L https://foundry.paradigm.xyz \| bash && foundryup` |
| `Timeout waiting for xlayer-node L2 RPC` | `tail -50 logs/xlayer-node.log` to see why |
| Port already in use (8123, 8552) | `lsof -ti:8123 \| xargs kill` |
| TX stuck at UNSAFE (never SAFE) | `docker compose -f docker/docker-compose.devnet.yml logs op-batcher --tail 30` |
| L1 stalls after restart | `internal/start-l1.sh` auto-detects and fixes geth/beacon head desync |
| Everything broken | `./scripts/devnet/maintenance/reset-l2.sh && ./scripts/devnet/start-all.sh` |

---

For full details, see [`scripts/devnet/README.md`](scripts/devnet/README.md).
