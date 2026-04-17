# load-test.sh — Complete Walkthrough

A stress-test and benchmarking script for the xlayer-node devnet. It drives concurrent
transaction submissions, polls for confirmation, measures block production statistics, and
probes the Engine API latency — all in one run, producing a structured pass/fail report.

---

## Quick start

```bash
# 60-second run, 6 workers (defaults)
./scripts/devnet/load-test.sh

# Longer run with fewer workers
./scripts/devnet/load-test.sh --duration 120 --concurrency 4

# Skip waiting for safe head (faster report, won't test batcher)
./scripts/devnet/load-test.sh --no-wait-safe

# Skip Engine API latency measurement
./scripts/devnet/load-test.sh --no-engine-probe

# Save report to a specific file
./scripts/devnet/load-test.sh --out results/run1.txt
```

Prerequisites: `cast` (Foundry), `jq`, `curl`, `python3`.
The devnet must be running (`./scripts/devnet/start-all.sh`).

---

## Blockchain concepts you need to know

Before reading the code, these terms appear everywhere:

**Unsafe head** — the latest L2 block the sequencer has produced and broadcast, but not yet
submitted to L1. Think of it as "the node has built this block" but it hasn't been committed
to Ethereum yet. TXs are confirmed at unsafe head first (fast, ~0.4s on XLayer).

**Safe head** — the latest L2 block whose transactions have been posted to L1 (by op-batcher)
and are therefore derivable by any node reading L1. This is the "committed to Ethereum"
guarantee. It lags unsafe by ~10–30 seconds depending on batcher cadence.

**Finalized head** — the L2 block whose L1 batches are in L1 blocks that are themselves
finalized (Ethereum finality, ~12 minutes). Extremely hard to revert.

**Engine API** — an internal HTTP (or in XLayer's case, in-process channel) API that the
consensus layer (kona-node) uses to tell the execution layer (reth/xlayer-node) to: build a
block (`engine_forkchoiceUpdatedV3` with payloadAttributes), retrieve a built block
(`engine_getPayloadV4`), or accept a new block (`engine_newPayloadV4`). Latency here directly
impacts block production throughput.

**FCU (ForkchoiceUpdated)** — shorthand for `engine_forkchoiceUpdatedV3`. kona calls this to
set which block is the current head/safe/finalized, and optionally to kick off building the
next block.

**JWT** — the Engine API requires a JSON Web Token for authentication. Both kona and this
script sign requests with the same shared secret (`config/devnet/jwt.txt`).

---

## Script structure — the 10 phases

```
Phase 0: Parse args + validate tunables
Phase 1: Pre-flight (RPC reachable? senders funded?)
Phase 2: Baseline snapshot (record starting block numbers)
Phase 3: Engine API baseline probe (measure FCU latency before load)
Phase 4: Spawn TX workers + run load for DURATION seconds
Phase 5: Stop workers, run Engine API post-load probe
Phase 6: Poll for receipt inclusion (wait for unsafe confirmation)
Phase 7: Collect block stats (fetch block metadata via RPC)
Phase 8: Wait for safe head to advance (tests op-batcher health)
Phase 9: Compute metrics, run assertions, print report
```

---

## Tunables (top of file, lines 44–59)

```bash
DURATION=60          # How long workers keep sending TXs
CONCURRENCY=6        # Number of parallel sender accounts (max 7 — limited by SENDER_KEYS)
GAS_PRICE_GWEI=110   # Gas price per TX (must be > base fee; devnet base fee is near 0)
CAST_TIMEOUT=8       # Max seconds for a single cast send before it's killed
CONFIRM_TIMEOUT=120  # Max seconds to wait for all TXs to appear in unsafe head
SAFE_WAIT=90         # Max seconds to wait for safe head after confirmation
NONCE_RESYNC_EVERY=100  # Resync nonce from chain every N successful sends
POLL_BATCH=100       # How many receipts to fetch in parallel per polling round
ENGINE_PROBE_N=20    # FCU pings per engine probe window
ENGINE_GP_N=5        # getPayload cycles in the baseline probe
```

**When to change these**:
- `DURATION`: increase for sustained load tests (e.g. 300 for 5-minute run)
- `CONCURRENCY`: reduce if you see nonce errors; increase only if you add more SENDER_KEYS
- `GAS_PRICE_GWEI`: raise if TXs are rejected with "gas price too low"
- `CAST_TIMEOUT`: lower if you want fast failures; raise if RPC is consistently slow
- `CONFIRM_TIMEOUT`: raise if running very high load (many TXs take longer to confirm)
- `POLL_BATCH`: lower if the node gets overloaded by parallel receipt fetches

---

## Sender accounts (lines 64–76)

```bash
SENDER_KEYS=(
    "0x59c6995e..."  # maps to address 0x70997970...
    "0x5de4111a..."  # maps to address 0x3C44CdDd...
    ...  # 7 accounts total
)
RECIPIENT="0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"  # Hardhat #0
```

These are Hardhat's standard deterministic test accounts (derived from the mnemonic
`"test test test test test test test test test test test junk"`). All 7 are pre-funded
with 10,000 ETH in the xlayer-node genesis (`config/devnet/genesis.json`).

Each worker owns exactly one key. Workers never share a key — this avoids nonce race
conditions (two workers trying to submit TX with the same nonce from the same address).

**To add more workers**: append a new `(key, address)` pair to `SENDER_KEYS` AND fund the
address in genesis (`config/devnet/genesis.json` → `alloc` section). Then bump `CONCURRENCY`.

---

## Helper functions

### `ms_now()` — millisecond timestamp
```bash
ms_now() { python3 -c "import time; print(int(time.time()*1000))"; }
```
Returns current Unix timestamp in milliseconds. Used to compute TX submission time and
receipt confirmation time for latency measurement. Uses Python because macOS `date` doesn't
support `%N` (nanoseconds).

### `hex2dec()` — hex string to decimal integer
```bash
hex2dec() { python3 -c "print(to_int('$v'))" ... }
```
Ethereum RPCs return numbers as hex strings (`"0x8300a1"`). This converts them to decimal
integers for arithmetic. Handles both `"0x..."` and plain decimal strings.

### `_timeout()` — portable command timeout
```bash
_timeout() {
    if command -v timeout ...; then timeout "$secs" "$@"
    elif command -v gtimeout ...; then gtimeout "$secs" "$@"
    else "$@"; fi
}
```
GNU `timeout` is not available on macOS by default. Falls back to `gtimeout` (Homebrew
coreutils) and then to running without a timeout. Wraps `cast send` to prevent a hung RPC
from stalling a worker forever.

### `sync_status()` / `l2_heads()`
```bash
sync_status() { curl ... 'optimism_syncStatus' ... }
l2_heads()    { # returns: unsafe_num safe_num fin_num (decimal, space-separated)
```
`optimism_syncStatus` is a kona-node RPC method that returns the current unsafe/safe/finalized
L2 block numbers and their L1 origins. Used to snapshot heads at start and end of the test.

### `run_engine_probe()` — Engine API latency measurement
A 200-line embedded Python script (lines 166–288) that:
1. Generates a JWT token (HS256 signed with the shared secret)
2. Sends `N` FCU pings (no payload attributes) — measures raw Engine API roundtrip
3. Sends `M` FCU+attrs requests followed by `getPayload` — measures block build latency
4. Appends results as `label:call_type<TAB>latency_ms` lines to a TSV file

The JWT generation is done in pure Python (no external dependencies) to avoid spawning
additional processes during the hot path.

---

## Phase 4: TX worker (`_lt_worker`, lines 303–370)

This is the core of the load test. Each worker is a bash subshell owning one private key.

```
loop:
  1. Send one ETH transfer via `cast send --async` (returns immediately with TX hash)
  2. If hash received → write "hash<TAB>send_ms" to worker TSV, increment nonce
  3. On error → classify the error and recover (see error matrix below)
  4. Repeat until STOP_FILE appears
```

**Why `--async`?** `cast send` without `--async` waits for the TX to be mined before returning.
That would serialize submissions and cap us at ~1 TX per block time. With `--async`, cast
returns the TX hash immediately after the RPC call succeeds, letting us submit as fast as the
node accepts.

**Nonce management**: Each worker tracks its own nonce locally (increments on success). This
allows submitting the next TX without waiting for the previous one to be mined. Every
`NONCE_RESYNC_EVERY` sends, it re-fetches from chain to handle edge cases.

**Error matrix** (lines 351–365):

| Error pattern | Recovery |
|---|---|
| `nonce too low` | Resync nonce from chain |
| `already known` | Advance nonce (TX is already queued) |
| `replacement underpriced` | Advance nonce (another TX with same nonce exists) |
| `pool full` / `mempool full` | Resync nonce + 500ms backoff |
| `connection refused` / `EOF` | 2s backoff (transient connectivity) |
| 5 consecutive unknown errors | Resync nonce + 1s backoff |

---

## Phase 4: Load loop (lines 452–474)

While workers run in the background, the main process:
- Prints a live progress line every 3 seconds: `[elapsed/total] submitted: N errors: N unsafe: N`
- Samples the Engine API every 5 seconds (fires a background FCU probe)

```bash
while [[ ! -f "$STOP_FILE" && elapsed < DURATION ]]; do
    print live stats
    if 5s since last engine sample: run_engine_probe in background
    sleep 3
done
```

After `DURATION` seconds, it creates `STOP_FILE`. All workers check for this file at the
start of each loop iteration and exit cleanly.

---

## Phase 6: Receipt polling (lines 503–573)

After all workers stop, we have a list of submitted TX hashes. We now poll until every TX
either gets a receipt (confirmed in unsafe head) or times out.

**Algorithm**:
```
pending = [all TX hashes]
loop:
  for each batch of POLL_BATCH hashes:
    fetch receipts in parallel (background subshells)
  for each result:
    if receipt found: write to confirmed.tsv (hash, send_ms, confirm_ms, block, status)
    else: keep in pending
  if pending empty or timeout: break
  sleep 2s
```

**Why batch parallel polling?** Fetching receipts serially for 4000+ TXs would take minutes.
Batching 100 parallel `cast receipt` calls per round makes this fast.

**What is `confirmed.tsv`?** A 5-column TSV:
```
hash    send_ms    confirm_ms    block_number    status(1=ok/0=fail)
```
This is the input for latency calculations in Phase 9.

---

## Phase 7: Block stats collection (lines 576–625)

Fetches metadata for the last 120 blocks produced during the test (or all blocks if < 120).
Uses `eth_getBlockByNumber` directly via Python's `urllib` — avoids `cast block` which can
output text format or prepend warnings that break JSON parsing.

For each block, records: `block_number  timestamp  tx_count  gas_used`

This feeds into `avg_bt` (average time between consecutive block timestamps), `avg_txb`,
`peak_txb`, and `avg_gas` in the metrics.

**Why only last 120 blocks?** Fetching 500+ blocks in parallel would hammer the RPC. 120 is
enough for a statistically meaningful sample while keeping collection time under a few seconds.

---

## Phase 8: Safe head wait (lines 627–650)

Waits for kona's safe head to reach at least the highest block number seen in confirmed
receipts. This verifies that op-batcher is running and healthy — if safe head never advances,
the batcher is broken or misconfigured.

```bash
TARGET = max(block_number from confirmed.tsv)
loop:
  safe_now = l2_heads()
  if safe_now >= TARGET: pass
  if elapsed > SAFE_WAIT: timeout (non-fatal)
  sleep 2s
```

Skip with `--no-wait-safe` if you only care about sequencer throughput, not batcher health.

---

## Phase 9: Metrics + assertions

### Metrics (Python embedded script, lines 656–765)

The Python script receives:
- `confirmed.tsv` → TX latency statistics
- `blocks.tsv` → block production statistics
- `engine.tsv` → Engine API latency per call type
- Scalar values: submitted, confirmed, failed, send_errs, elapsed, head start/end values

It computes and returns a JSON blob:

| Field | What it measures |
|---|---|
| `sub_rate` | TX/s submission rate (how fast we can send TXs) |
| `eff_tps` | Confirmed TX/s (confirmed / total elapsed) |
| `incl_pct` | % of submitted TXs that got confirmed |
| `lat_min/avg/p50/p95/p99/max` | TX latency: time from `cast send` returning a hash to receipt appearing in a polling round (polled, not real-time; see note below) |
| `avg_bt` | Average seconds between consecutive block timestamps |
| `avg_txb` | Average TXs per block |
| `peak_txb` | Maximum TXs in any single block |
| `avg_gas` | Average gas used per block |
| `safe_lag` | `unsafe_final - safe_final` (how far safe head lags unsafe) |
| `fin_lag` | `unsafe_final - finalized_final` |
| `no_gaps` | True if block numbers in the sample window are consecutive (no dropped blocks) |

> **Note on TX latency**: The latency measured is "time from TX submission to when the polling
> loop first saw the receipt". Since the polling loop sweeps all pending TXs every 2 seconds,
> this includes polling delay. It is NOT a real-time push notification. The Min latency
> represents the first sweep that caught a receipt; the Max represents TXs that were caught
> late in the polling queue. For true inclusion latency (sub-second), you'd need a websocket
> subscription.

### Assertions (lines 776–818)

11 assertions, all must pass for `RESULT: PASS`:

| Assertion | What it checks |
|---|---|
| Chain ID = 195 | RPC is pointing at the right chain |
| Unsafe head advanced | Sequencer is producing blocks |
| Safe head advanced | op-batcher is running and posting batches |
| Finalized head advanced | L1 finality is progressing |
| All receipts status = 1 | No TXs reverted |
| TX inclusion rate ≥ 90% | No more than 10% of submitted TXs dropped |
| Send-error rate < 5% | Workers aren't being rejected by the mempool |
| Avg block time ≤ 1.5s (and > 0) | Sequencer isn't stalling; block time target is ~0.4s |
| No block number gaps | No dropped/skipped blocks in the sample window |
| Safe lag ≤ 30 blocks | Batcher is keeping up with the sequencer |
| Engine API responsive | At least 1 FCU call succeeded in baseline probe |

---

## Output files

| File | Contents |
|---|---|
| `load-test-YYYYMMDD_HHMMSS.txt` | Full report (same as stdout) |
| `load-test-YYYYMMDD_HHMMSS-errors.log` | All `cast send` stderr lines from workers |

The report file path can be overridden with `--out FILENAME`.

The errors log is only written if there were send errors. Useful for diagnosing nonce/pool
issues in detail.

---

## How to extend the script

### Add a new assertion
```bash
# After the existing assert block (before TOTAL_ASSERTS line):
assert "My new check  (got $(_m my_metric))" \
    "$(py_cmp "float('$(_m my_metric)') >= threshold")"
```

`_m KEY` extracts a value from the JSON metrics blob. `py_cmp "expr"` evaluates a Python
boolean expression and returns `"true"` or `"false"`.

### Add a new metric
In the Python metrics script (the `PYEOF` heredoc), add your computation and include it in
the final `print(json.dumps({...}))` call. Then reference it with `_m your_key` in the report
or assertions.

### Add more sender accounts
1. Get a private key → derive the address: `cast wallet address 0xYOUR_KEY`
2. Add the address with 10000 ETH to `config/devnet/genesis.json` under `alloc`
3. Append the key to `SENDER_KEYS` in this script
4. Redeploy genesis (if chain is fresh) or fund the address manually via `cast send`

### Change the TX type
Workers currently send `0.0001 ETH` native transfers (21,000 gas each). To send contract
calls instead, modify the `cast send` invocation in `_lt_worker` (around line 324):

```bash
# Native transfer (current):
hash=$(_timeout "$CAST_TIMEOUT" cast send \
    --rpc-url "$L2_RPC_URL" --private-key "$key" \
    --nonce "$nonce" --gas-price "${GAS_PRICE_GWEI}gwei" \
    --gas-limit 21000 --async \
    "$RECIPIENT" --value "0.0001ether" ...)

# ERC-20 transfer (example):
hash=$(_timeout "$CAST_TIMEOUT" cast send \
    --rpc-url "$L2_RPC_URL" --private-key "$key" \
    --nonce "$nonce" --gas-price "${GAS_PRICE_GWEI}gwei" \
    --gas-limit 65000 --async \
    "$TOKEN_CONTRACT" "transfer(address,uint256)" "$RECIPIENT" "1000000" ...)
```

Remember to increase `--gas-limit` for contract calls.

### Change the test duration without re-running
The script exits when `STOP_FILE` is touched. You can stop early from another terminal:
```bash
# Find the TMP dir (shown in script output) and touch the stop file — but the script
# manages TMP internally. Easier: just send SIGINT (Ctrl+C). The trap handler produces
# a partial report cleanly.
```

---

## Common failure modes

| Symptom | Cause | Fix |
|---|---|---|
| Pre-flight fails: "insufficient ETH" | Sender address not funded | Fund it: `cast send --rpc-url http://localhost:8123 --private-key $FUNDER_KEY --value 100ether $ADDRESS` |
| TX inclusion rate < 90% | Mempool full or RPC dropping TXs | Reduce `CONCURRENCY` or `GAS_PRICE_GWEI` |
| Safe head not advancing | op-batcher not running | Start batcher: see `scripts/devnet/README.md` |
| Engine API probe skipped | `config/devnet/jwt.txt` missing | Generate: `openssl rand -hex 32 > config/devnet/jwt.txt` |
| All block stats are 0 | RPC unreachable during block collection | Check node is still running; the collection happens after receipt polling (~120s after load) |
| Avg block time 0 or fails | Block stats collection failed | Run manually: `curl -s -X POST http://localhost:8123 -H 'Content-Type: application/json' -d '{"method":"eth_getBlockByNumber","params":["latest",false],"id":1,"jsonrpc":"2.0"}'` |
| `nonce too low` errors | Two workers used the same key | Check `SENDER_KEYS` for duplicate entries |
