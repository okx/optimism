# scripts/devnet/ — Script Reference

Developer reference for every script in this directory.
For the full operational guide (quick-start, stop/restart, logs, errors) see [`DEVNET.md`](../../DEVNET.md).

All scripts **must be run from the project root** (`xlayer/`). They resolve config and binary paths relative to `$XLAYER_ROOT` (set by `lib.sh`).

---

## Shared Foundation — `lib.sh`

Sourced by every script. Not run directly.

**What it provides:**

| Symbol | Description |
|--------|-------------|
| `$XLAYER_ROOT` | Absolute path to the project root |
| `$XLAYER_BINARY` | Path to the built binary (`target/release/xlayer-node`) |
| `$XLAYER_DATA_DIR` | Chain data directory (default: `/tmp/xlayer-node-data`) |
| `$L2_RPC_URL` | L2 ETH JSON-RPC (`:8123`) |
| `$L2_ROLLUP_RPC_URL` | Rollup RPC for `optimism_syncStatus` (`:9545`) |
| `$L1_RPC_URL` | L1 geth RPC (`:8545`) |
| `info / ok / warn / fail / step` | Coloured status helpers — use these, not plain `echo` |
| `wait_for_rpc <url> <name>` | Polls until the RPC answers; exits 1 on timeout |
| `check_deps <tool...>` | Asserts tools are on PATH; prints install hint if missing |

All values come from `config/devnet/.env`. Copy `config/devnet/.env.example` to `config/devnet/.env` before first run.

---

## Orchestrators

### `0-all.sh`

**Entry point. First-run safe.**

Handles everything a fresh checkout needs before starting:
- Checks prerequisites (`cargo`, `cast`, `docker`, `jq`, `curl`)
- Creates `config/devnet/.env` from `.env.example` if absent
- Generates `config/devnet/jwt.txt` if absent
- Delegates to `internal/start-l1.sh` then `internal/start-node.sh`

| Flag | Effect |
|------|--------|
| `--no-build` | Skip `cargo build`; use the existing binary |

Exit `0` on clean start, `1` on any failure.

---

### `start-all.sh`

**Start from a known-stopped state.**

Assumes prerequisites are met and `.env` / `jwt.txt` already exist (i.e. `0-all.sh` has been run at least once). Use this after `stop-all.sh`.

| Flag | Effect |
|------|--------|
| `--no-build` | Skip `cargo build` |

---

### `stop-all.sh`

**Stop devnet components. Chain data is never deleted.**

| Flag | Effect |
|------|--------|
| *(none)* | Stop everything: xlayer-node + op-batcher + L1 docker services |
| `--node` | Stop xlayer-node + op-batcher only; L1 keeps running |
| `--l1` | Stop L1 docker services only; node keeps running |

---

### `restart-node.sh`

**Rebuild and restart xlayer-node + op-batcher. L1 is never touched.**

Equivalent to `stop-all.sh --node` followed by a build and `start-all.sh`.

| Flag | Effect |
|------|--------|
| `--no-build` | Skip `cargo build` |

---

## Testing

### `load-test.sh`

**Stress test + assertion suite. The primary tool for validating xlayer-node under load.**

Drives concurrent native-ETH transfers from 6 genesis-funded Hardhat senders, then asserts every transaction lands on-chain. Produces a full metrics report on exit.

| Flag | Default | Effect |
|------|---------|--------|
| `--duration SEC` | `60` | Seconds of active load |
| `--concurrency N` | `6` | Concurrent sender workers (max 7) |
| `--no-wait-safe` | off | Skip waiting for safe head after load |
| `--out FILE` | auto-named | Path to write the report |

Exit `0` if all 10 assertions pass, `1` otherwise — suitable for CI.

**Report sections:**
- **Throughput** — TXs submitted/confirmed/dropped, submission rate, effective TPS
- **Latency** — time-to-unsafe: min / avg / p50 / p95 / p99 / max (ms)
- **Block production** — blocks produced, avg block time, avg/peak TXs per block, avg gas per block
- **L2 heads** — unsafe/safe/finalized at start and end with delta and lag
- **Assertions** — 10 xlayer-specific checks: chain ID, head advancement, receipt status, block gaps, safe lag, inclusion rate, batcher health

No external dependencies beyond `cast`, `jq`, `curl`, `python3`. Senders are pre-funded in genesis — no setup needed.

---

### `test-tx.sh`

**Send transactions and verify the full lifecycle: unsafe → safe → finalized.**

Use this as a functional smoke-test after any change. Returns exit `0` only if every TX reaches FINALIZED within the timeout — suitable for CI.

| Flag | Default | Effect |
|------|---------|--------|
| `--count N` | `1` | Number of TXs to send |
| `--value VAL` | `0.001ether` | Value per TX (e.g. `1ether`) |
| `--timeout SEC` | `300` | Seconds to wait for FINALIZED |
| `--verbose` | off | Print head-poll output on each iteration |

Requires `TEST_SENDER_KEY` and `TEST_RECIPIENT` in `.env`.

---

### `perf-baseline.sh`

**Measure L2 TPS and time-to-unsafe latency. Not a sustained load test.**

Sends a burst of TXs and reports:
- Submission rate (TX/s sent to RPC)
- Per-TX time-to-unsafe (ms from send to block inclusion)
- Effective TPS (confirmed TX/s over the measurement window)

| Flag | Default | Effect |
|------|---------|--------|
| `--count N` | `20` | Number of TXs to send |

Requires `bc` (`brew install bc`), `TEST_SENDER_KEY`, and `TEST_RECIPIENT` in `.env`.

---

## Monitoring

### `health-check.sh`

**Live dashboard showing all component heads and status.**

Displays: L1 geth block, L1 beacon slot, L2 unsafe/safe/finalized heads, safe lag, block rate, op-batcher container status.

| Flag | Default | Effect |
|------|---------|--------|
| `--once` | off | Print a single snapshot and exit |
| `--watch N` | `5` | Refresh interval in seconds |

---

## Internal Scripts

> Called by the orchestrators above. Also usable standalone for step-by-step startup or interactive debugging — run each in its own terminal.

### `internal/start-l1.sh`

Starts L1 geth + Prysm beacon via `docker compose`. On restart, auto-detects and repairs geth/beacon head desync (common after an unclean shutdown). Blocks until the L1 RPC is reachable before returning.

### `internal/start-node.sh`

Builds `xlayer-node` (respects `--no-build`), then launches it in the foreground with all output written to both stdout and `logs/xlayer-node.log`. Waits until the L2 RPC (`:8123`) answers, then starts the op-batcher Docker container. Run this in a dedicated terminal when you want to see live node output.

---

## Maintenance Scripts

> **Read the in-file header before running. These scripts are destructive or for exceptional infrastructure operations.**

### `maintenance/reset-l2.sh`

**Wipe L2 chain data and logs. L1 and all config files are untouched.**

| Flag | Effect |
|------|--------|
| *(none)* | Delete `$XLAYER_DATA_DIR` and `logs/` |
| `--secrets` | Also delete `config/devnet/jwt.txt` (auto-regenerated on next start) |
| `--nuke` | Also delete `config/devnet/.env` — requires typing `yes-delete-my-config` twice |

Use after: genesis change, rollup.json update, or a corrupted database.

### `maintenance/reset-l1-reconfig.sh`

**⚠️ Infrastructure maintenance only. Regular developers never need this.**

Run only after wiping `docker/l1/` and redeploying OP contracts from scratch (a full L1 reset). Computes the reth genesis hash from the new `config/devnet/genesis.json` and patches `config/devnet/rollup.json` with the correct value. Warns if stale L2 data needs clearing.

Full procedure: see the comment block at the top of the script and the "After a Full L1 Reset" section in [`DEVNET.md`](../../DEVNET.md).

---

## Adding a New Script

1. Source `lib.sh` first:
   ```bash
   source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"   # from scripts/devnet/
   source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh" # from scripts/devnet/internal/ or maintenance/
   ```
2. Declare tool dependencies early: `check_deps cast jq curl`
3. Use `info`, `ok`, `warn`, `fail`, `step` for all status output — not `echo`
4. Use `$XLAYER_ROOT` for absolute paths — never hardcode `/tmp/` or `~/`
5. Exit `0` on success, `1` on any failure
