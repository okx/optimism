# Enhanced Load Test & Benchmark Suite

## Overview

The enhanced load test suite (`5-perf-baseline.sh`) measures **all critical latency metrics** for comparing xlayer-node (single-binary) against reth+op-node (2-process) setups.

### 🛡️ Comprehensive Pre-Flight Health Checks

**The script validates ALL infrastructure before starting:**

1. ✅ **L1 geth** (execution layer) — RPC responding, producing blocks
2. ✅ **L1 beacon-chain** (consensus layer) — syncing status checked
3. ✅ **L1 validator** — container running, proposing blocks
4. ✅ **xlayer-node** (L2 execution) — RPC responding, producing blocks
5. ✅ **kona-node** (L2 rollup/consensus) — rollup RPC operational
6. ✅ **op-batcher** — container running, admin RPC responding

**If ANY critical component is down, the script FAILS FAST with clear instructions.**

This prevents the "all zeros" problem where transactions are sent but never confirmed.

## Key Metrics Captured

### 1. Transaction Throughput
- **Submission rate**: TX/s during send phase
- **Effective TPS (unsafe)**: TXs confirmed per second (sequencer throughput)
- **Effective TPS (safe)**: TXs becoming safe per second (L1-derived)
- **Effective TPS (finalized)**: TXs becoming finalized per second

### 2. Transaction Latencies (End-to-End)
- **Time to unsafe**: TX send → unsafe inclusion (sequencer latency)
- **Time to safe**: TX send → safe confirmation (includes L1 batch + derivation)
- **Time to finalized**: TX send → finalized (includes L1 beacon finality)

### 3. Engine API Call Latencies ⭐ **NEW**
Extracted from logs with min/max/avg/p95 statistics:

- **`fork_choice_updated`**: Consensus → execution head update + optional build trigger
- **`new_payload`**: Consensus → execution block validation/execution
- **`get_payload`**: Consensus retrieves built block from execution

These are the **critical inter-process communication metrics** that differ between:
- **xlayer-node**: Direct in-process calls via Rust channels (~microseconds)
- **reth+op-node**: HTTP+JWT over loopback socket (~milliseconds)

---

## Usage

### Quick Start

```bash
# Standard 50 TX test (serial)
./scripts/devnet/5-perf-baseline.sh

# Fast parallel test with 200 TXs
./scripts/devnet/5-perf-baseline.sh --parallel --count 200

# Continuous load for 60 seconds
./scripts/devnet/5-perf-baseline.sh --duration 60 --parallel

# Custom results directory
./scripts/devnet/5-perf-baseline.sh --results-dir ./benchmarks
```

### Flags

| Flag | Description | Default |
|------|-------------|---------|
| `--count N` | Send exactly N transactions | 50 |
| `--duration N` | Send TXs continuously for N seconds (overrides --count) | 0 (disabled) |
| `--parallel` | Use multiple sender accounts to maximize throughput | false (serial) |
| `--results-dir DIR` | Where to save CSV/JSON results | `./results` |

---

## Prerequisites

### 1. Ensure Multiple Funded Accounts (for `--parallel`)

```bash
# Check your .env has distinct keys
source config/devnet/.env
echo "TEST_SENDER:   $(cast wallet address "$TEST_SENDER_KEY")"
echo "OP_PROPOSER:   $(cast wallet address "$OP_PROPOSER_PRIVATE_KEY")"
echo "OP_CHALLENGER: $(cast wallet address "$OP_CHALLENGER_PRIVATE_KEY")"
```

If any addresses are duplicates, update `.env` with distinct Foundry test accounts:

```bash
TEST_SENDER_KEY=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d
OP_PROPOSER_PRIVATE_KEY=0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a
OP_CHALLENGER_PRIVATE_KEY=0x8b3a350cf5c34c9194ca9aa3f146b2b9afed22cd83d3c5f6a3f2f243ce220c01
```

Fund accounts:

```bash
cast send --rpc-url http://localhost:8123 --private-key $TEST_SENDER_KEY \
  --value 50ether 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC

cast send --rpc-url http://localhost:8123 --private-key $TEST_SENDER_KEY \
  --value 50ether 0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc
```

See: [Parallel Mode Setup Guide](../setup/parallel-mode-setup.md)

### 2. Ensure Logs Are Available

The script reads `logs/xlayer-node.log` to extract Engine API latencies.

**If running xlayer-node as a background process:**

```bash
# Start with log capture
./scripts/devnet/2-start-node.sh > logs/xlayer-node.log 2>&1 &
```

**If running via systemd or Docker**, ensure logs are written to `logs/xlayer-node.log` or set `LOG_FILE` environment variable:

```bash
export LOG_FILE=/path/to/your/xlayer-node.log
./scripts/devnet/5-perf-baseline.sh --parallel --count 100
```

---

## Output

### Console Summary

**Health Check Phase:**

```
── Pre-flight health checks (all nodes must be running)...

  [1/6] L1 geth (execution)...           ✅ OK (block: 12456)
  [2/6] L1 beacon-chain (consensus)...   ✅ OK (synced)
  [3/6] L1 validator...                  ✅ OK
  [4/6] xlayer-node (L2 execution)...    ✅ OK (block: 8615289)
  [5/6] kona-node (L2 rollup/consensus)... ✅ OK (unsafe: 8615289, safe: 8615250)
  [6/6] op-batcher...                    ✅ OK (admin RPC responding)

✅ All nodes healthy — ready to start load test
```

**Load Test Results:**

```
✅ Sent 200 TXs in 12s (16.7 TX/s submission rate)
✅ Unsafe: 200/200 confirmed in 8s
✅ Safe head reached block 8615289 in +18s

── Results

Transaction Throughput
  TXs sent:                      200
  TXs confirmed (unsafe):        200
  TXs confirmed (safe):          200
  TXs confirmed (finalized):     198
  Submission rate:               16.7 TX/s
  Effective TPS (unsafe):        9.52
  Effective TPS (safe):          8.12
  Effective TPS (finalized):     3.21

Transaction Latencies (avg)
  Time to unsafe:                1234ms
  Time to safe:                  18567ms
  Time to finalized:             42891ms

Engine API Call Latencies
  fork_choice_updated:
    Count: 201  |  Avg: 145 µs  |  P95: 289 µs  |  Min: 82 µs  |  Max: 512 µs
  new_payload:
    Count: 198  |  Avg: 2341 µs  |  P95: 4567 µs  |  Min: 1234 µs  |  Max: 8901 µs
  get_payload:
    Count: 200  |  Avg: 456 µs  |  P95: 789 µs  |  Min: 234 µs  |  Max: 1234 µs

  Total elapsed:                 45s

✅ Results saved:
  CSV:  results/perf-baseline-20260325-143052.csv
  JSON: results/perf-baseline-20260325-143052.json
```

### CSV Export

Machine-readable format for charting:

```csv
metric,value,unit
txs_sent,200,count
txs_unsafe,200,count
txs_safe,200,count
txs_finalized,198,count
submission_rate,16.67,tx_per_sec
tps_unsafe,9.52,tx_per_sec
tps_safe,8.12,tx_per_sec
tps_finalized,3.21,tx_per_sec
latency_unsafe_avg,1234,ms
latency_safe_avg,18567,ms
latency_finalized_avg,42891,ms
fcu_count,201,count
fcu_avg,145,us
fcu_p95,289,us
...
```

### JSON Export

Structured format for automation:

```json
{
  "timestamp": "20260325-143052",
  "config": {
    "tx_count": 200,
    "duration": 0,
    "parallel": true,
    "sender_accounts": 3
  },
  "throughput": {
    "txs_sent": 200,
    "txs_unsafe": 200,
    "txs_safe": 200,
    "txs_finalized": 198,
    "submission_rate_tx_per_sec": 16.67,
    "tps_unsafe": 9.52,
    "tps_safe": 8.12,
    "tps_finalized": 3.21
  },
  "latency_ms": {
    "unsafe_avg": 1234,
    "safe_avg": 18567,
    "finalized_avg": 42891
  },
  "engine_api_us": {
    "fork_choice_updated": {
      "count": 201,
      "avg": 145,
      "p95": 289,
      "min": 82,
      "max": 512
    },
    "new_payload": { ... },
    "get_payload": { ... }
  },
  "timing": {
    "wall_clock_total_s": 45,
    "send_elapsed_s": 12
  }
}
```

---

## Comparing xlayer-node vs reth+op-node

### Step 1: Run Benchmark on xlayer-node

```bash
# Ensure xlayer-node is running
./scripts/devnet/0-all.sh

# Run enhanced load test
./scripts/devnet/5-perf-baseline.sh --parallel --count 200

# Save results
cp results/perf-baseline-*.json benchmarks/xlayer-node-run1.json
```

### Step 2: Run Benchmark on reth+op-node (2-process setup)

**Setup reth+op-node** (in a separate directory):

```bash
# Start reth (execution layer)
op-reth node --chain=your-rollup.json --http --authrpc.jwtsecret=/path/to/jwt.txt

# Start op-node (consensus layer)
op-node --rollup.config=rollup.json --l1=http://localhost:8545 \
  --l2=http://localhost:8551 --l2.jwt-secret=/path/to/jwt.txt
```

**Copy the script** to that setup:

```bash
# Copy the enhanced script
cp xlayer/scripts/devnet/5-perf-baseline.sh reth-opnode-setup/

# Update variables to match reth+op-node ports
export L2_RPC_URL=http://localhost:8545  # reth HTTP
export L2_ROLLUP_RPC_URL=http://localhost:9545  # op-node rollup RPC
export LOG_FILE=/path/to/op-node.log  # op-node logs (for Engine API metrics)

# Run the same test
./5-perf-baseline.sh --parallel --count 200
```

### Step 3: Compare Results

```bash
# Side-by-side comparison
jq '{
  setup: "xlayer-node",
  tps_unsafe: .throughput.tps_unsafe,
  latency_unsafe_avg_ms: .latency_ms.unsafe_avg,
  fcu_avg_us: .engine_api_us.fork_choice_updated.avg,
  new_payload_avg_us: .engine_api_us.new_payload.avg,
  get_payload_avg_us: .engine_api_us.get_payload.avg
}' benchmarks/xlayer-node-run1.json

jq '{
  setup: "reth+op-node",
  tps_unsafe: .throughput.tps_unsafe,
  latency_unsafe_avg_ms: .latency_ms.unsafe_avg,
  fcu_avg_us: .engine_api_us.fork_choice_updated.avg,
  new_payload_avg_us: .engine_api_us.new_payload.avg,
  get_payload_avg_us: .engine_api_us.get_payload.avg
}' benchmarks/reth-opnode-run1.json
```

**Expected differences:**

| Metric | xlayer-node (1-process) | reth+op-node (2-process) | Difference |
|--------|------------------------|--------------------------|------------|
| `fcu_avg_us` | ~100-300 µs | ~500-2000 µs | **5-10x faster** (no HTTP) |
| `new_payload_avg_us` | ~2000-5000 µs | ~3000-8000 µs | **1.5-2x faster** (no serialization) |
| `get_payload_avg_us` | ~200-500 µs | ~800-2000 µs | **3-5x faster** (direct handle) |
| `tps_unsafe` | Similar | Similar | Bottleneck is EVM, not IPC |
| `latency_unsafe_avg_ms` | Slightly lower | Slightly higher | Reduced by cumulative Engine API overhead |

---

## Troubleshooting

### All Zeros in Results (Most Common Issue!)

**This is now PREVENTED by pre-flight health checks!**

The script will fail before sending any transactions if critical infrastructure is down:

```
── Pre-flight health checks (all nodes must be running)...

  [1/6] L1 geth (execution)...           ✅ OK (block: 12456)
  [2/6] L1 beacon-chain (consensus)...   ✅ OK (synced)
  [3/6] L1 validator...                  ✅ OK
  [4/6] xlayer-node (L2 execution)...    ❌ DOWN
        RPC: http://localhost:8123
        Fix: Check if xlayer-node process is running:
             ps aux | grep xlayer-node
             ./scripts/devnet/2-start-node.sh
  [5/6] kona-node (L2 rollup/consensus)... ❌ DOWN
        RPC: http://localhost:9545
        Note: Rollup RPC is served by kona inside xlayer-node
        Fix: Ensure xlayer-node is running with rollup RPC enabled
  [6/6] op-batcher...                    ⚠️  DOWN
        Container: op-batcher
        Impact: Safe/finalized heads will NOT advance
        Fix: docker compose -f docker/docker-compose.devnet.yml up -d op-batcher

        ⚠️  WARNING: Continuing without batcher — only unsafe metrics will be measured

❌ Pre-flight checks FAILED — cannot proceed with load test

  Quick fix (start everything):
    ./scripts/devnet/0-all.sh

  Or start individual components as shown above.
```

**Old behavior (before fix):**
- Script would send TXs to non-existent endpoints
- Wait 80+ seconds for confirmations that never come
- Report all zeros with no clear explanation

**New behavior:**
- Validates ALL nodes upfront (takes ~2 seconds)
- Fails immediately with specific node that's down
- Provides exact commands to fix the issue

---

### "Parallel mode requires at least 2 funded accounts"

See [Parallel Mode Setup Guide](../setup/parallel-mode-setup.md) to fund additional accounts.

### "Cannot extract Engine API metrics — log file not available"

Ensure xlayer-node is writing logs:

```bash
# Check log file exists
ls -lh logs/xlayer-node.log

# Or set custom log path
export LOG_FILE=/path/to/xlayer-node.log
./scripts/devnet/5-perf-baseline.sh --parallel --count 100
```

### Engine API metrics show zero

**Cause**: Log format changed or logs don't contain `engine_bridge` target.

**Fix**: Check log level in `config/devnet/xlayer-node.toml`:

```toml
[node]
log_level = "info"  # or "debug" for more detail
```

Or set via environment:

```bash
export XLAYER_LOG_LEVEL=debug
./scripts/devnet/2-start-node.sh
```

### Low TPS compared to expectations

1. **Check parallel mode is enabled**: `--parallel` flag
2. **Verify multiple accounts funded**: Script should show "Using 3 sender accounts"
3. **Increase TX count**: Use `--count 500` or `--duration 60` for sustained load
4. **Check node resources**: CPU/disk may be bottleneck (use `docker stats` or `top`)

---

## Benchmark Best Practices

### 1. Warm-up Run

Always discard the first run (cold cache, JIT warmup):

```bash
# Warm-up
./scripts/devnet/5-perf-baseline.sh --count 50

# Wait 30s for finalization to complete
sleep 30

# Real benchmark (3 runs)
for i in 1 2 3; do
  ./scripts/devnet/5-perf-baseline.sh --parallel --count 200
  sleep 30
done
```

### 2. Compare Like-for-Like

- **Same hardware**: Run both setups on identical machines
- **Same config**: Block time, gas limit, L1 block time
- **Same load**: Use identical `--count` and `--parallel` settings
- **Same stage**: Compare unsafe-to-unsafe, not unsafe-to-finalized across setups

### 3. Multiple Runs

Take median of 3-5 runs to account for variance:

```bash
# Automated multi-run
for i in {1..5}; do
  ./scripts/devnet/5-perf-baseline.sh --parallel --count 200
  sleep 60  # cooldown between runs
done

# Analyze
jq -s 'map(.throughput.tps_unsafe) | add / length' results/*.json  # avg TPS
```

---

## Advanced: Profiling Engine API Calls

For deeper analysis, increase log verbosity:

```toml
# config/devnet/xlayer-node.toml
[node]
log_level = "trace,engine_bridge=trace"
```

Then grep for detailed timing:

```bash
tail -f logs/xlayer-node.log | grep engine_bridge
```

Look for:
- `FCU` — fork_choice_updated entry
- `FCU ok` — completion with elapsed time
- `new_payload ok` — block execution time
- `get_payload ok` — payload retrieval time

---

## Related Documentation

- [Block Transitions](../concepts/block-transitions.md) — Unsafe/safe/finalized progression
- [Parallel Mode Setup](../setup/parallel-mode-setup.md) — Configure multi-account testing
- [Channel Architecture](../design/06-channel-architecture.md) — How Engine API works in-process

---

## Summary

The enhanced load test (`5-perf-baseline.sh`) provides:

✅ **Complete metrics** for comparing xlayer-node vs reth+op-node  
✅ **Engine API latencies** extracted from logs (FCU, new_payload, get_payload)  
✅ **Parallel mode** for realistic throughput testing  
✅ **CSV/JSON export** for automation and charting  
✅ **Same script works on both setups** for apples-to-apples comparison  

Run it, compare, and quantify the benefits of the single-binary architecture! 🚀

