# Devnet Guide

Run xlayer-node locally: L1 (Docker) + xlayer-node (native binary) + op-batcher (Docker).

---

## Prerequisites

Install once:

| Tool | Install |
|------|---------|
| Rust 1.91+ | `curl https://sh.rustup.rs -sSf \| sh && rustup install 1.91` |
| Foundry (cast) | `curl -L https://foundry.paradigm.xyz \| bash && foundryup` |
| Docker Desktop | https://docs.docker.com/get-docker/ |
| jq | `brew install jq` (macOS) / `apt install jq` (Linux) |

---

## First Run (fresh clone)

```bash
cd rust/xlayer

# Make scripts executable
chmod +x scripts/devnet/*.sh scripts/devnet/internal/*.sh scripts/devnet/maintenance/*.sh

# Build + start everything (~5 min build, ~1 min L1 bootstrap)
./scripts/devnet/0-all.sh
```

`0-all.sh` does everything automatically:
1. Checks prerequisites (cargo, cast, docker, jq, curl, openssl)
2. Creates `config/devnet/.env` from `.env.example`
3. Generates `config/devnet/jwt.txt`
4. Builds `xlayer-node` binary (`cargo build --release`)
5. Starts L1 (Docker: geth + Prysm beacon + validator)
6. Starts xlayer-node (native binary, background)
7. Starts op-batcher (Docker)

### First run will fail with `ResetForkchoiceError` — this is expected

The committed `genesis.json` references L1 block hashes from a previous deployment.
Your fresh L1 has different block hashes. Fix:

```bash
# Deploy OP contracts on your fresh L1 and regenerate genesis.json + rollup.json
./scripts/devnet/maintenance/redeploy-op-contracts.sh

# Start the node (L1 is already running)
./scripts/devnet/start-all.sh --no-build
```

This takes ~3 minutes. After it completes, the node starts producing blocks.

**You only need to do this once.** L1 data is preserved across stop/restart.

---

## Verify

```bash
# Blocks advancing? (run twice, number must increase)
cast bn --rpc-url http://localhost:8123

# Full smoke test: send TX, track through unsafe → safe → finalized
./scripts/devnet/test-tx.sh

# Live health dashboard
./scripts/devnet/health-check.sh --once
```

---

## Stop / Resume

```bash
# Stop everything (all data preserved)
./scripts/devnet/stop-all.sh

# Resume (no rebuild, no re-init — picks up where it left off)
./scripts/devnet/start-all.sh --no-build
```

---

## Reset L2 (fresh chain, same L1)

```bash
./scripts/devnet/stop-all.sh
./scripts/devnet/maintenance/reset-l2.sh
./scripts/devnet/start-all.sh --no-build
```

This wipes `/tmp/xlayer-data` and logs. L1 is untouched.

---

## Logs

```bash
tail -f logs/xlayer-node.log                        # node output
grep "engine_bridge" logs/xlayer-node.log | tail -5  # engine bridge calls
docker logs op-batcher --tail 30                     # batcher
```

---

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `ResetForkchoiceError("Block not found: 0x...")` | genesis.json references L1 blocks that don't exist on your L1 | `./scripts/devnet/maintenance/redeploy-op-contracts.sh` then `start-all.sh --no-build` |
| `Timeout waiting for xlayer-node L2 RPC` | Node crashed on startup | `tail -50 logs/xlayer-node.log` — look for the error |
| Port already in use (8123 / 8545) | Previous run didn't stop cleanly | `./scripts/devnet/stop-all.sh` or `lsof -ti:8123 \| xargs kill` |
| TX stuck at UNSAFE (never reaches SAFE) | op-batcher not running or crashed | `docker logs op-batcher --tail 30` |
| L1 stalls after restart | geth/beacon head desync | `start-l1.sh` auto-detects and fixes this — just run `start-all.sh` |
| Docker network conflict | Stale network from previous run | `docker network rm xlayer-devnet` then retry |
| Stale container conflict | Leftover containers | `docker rm -f $(docker ps -aq --filter name=l1-)` then retry |
| Everything broken | Unknown state | `stop-all.sh` → `maintenance/reset-l2.sh` → `start-all.sh --no-build` |

---

## Scripts Reference

| Script | Purpose |
|--------|---------|
| `0-all.sh` | First-run entry point — checks deps, creates config, builds, starts everything |
| `start-all.sh [--no-build]` | Start from stopped state (assumes config exists) |
| `stop-all.sh` | Stop all components (data preserved) |
| `test-tx.sh` | Smoke test — send TX, track unsafe → safe → finalized |
| `health-check.sh [--once]` | Live dashboard or single snapshot |
| `maintenance/reset-l2.sh` | Wipe L2 chain data + logs (L1 untouched) |
| `maintenance/redeploy-op-contracts.sh` | Deploy OP contracts on fresh L1, regenerate genesis |
