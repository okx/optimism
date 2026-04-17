# Parallel Transaction Submission: Race Conditions & Solutions

## The Problem: Nonce Race Conditions

When sending transactions in parallel from a **single EOA (Externally Owned Account)**, you will encounter nonce race conditions that cause transaction failures.

### Why Serial Submission Is Slow

```bash
# Serial submission (original perf-baseline.sh)
for i in 1..20; do
  cast send --private-key $KEY ...   # ~1-2s per call
done
# Total: 20-40 seconds for 20 TXs
```

Each `cast send` call:
1. Queries the node for the current nonce via `eth_getTransactionCount`
2. Signs the transaction with that nonce
3. Submits via `eth_sendRawTransaction`
4. Waits for RPC response

**Throughput bottleneck:** If each call takes 1.5s, maximum submission rate = 0.66 TX/s, regardless of how fast the sequencer can process transactions.

---

## Race Condition Scenario

### What Happens with Naive Parallel Submission (Single Account)

```bash
# BAD: All processes race on nonce
for i in 1..20; do
  (cast send --private-key $SAME_KEY ...) &
done
wait
```

**Timeline:**

```
t=0ms:  Process 1 queries nonce → receives "5"
t=5ms:  Process 2 queries nonce → receives "5"  (same!)
t=10ms: Process 3 queries nonce → receives "5"  (same!)
...
t=50ms: All 20 processes sign TX with nonce=5
t=100ms: All 20 TXs submitted to mempool

Result:
- TX with nonce=5 from Process 1: ✅ accepted
- TXs with nonce=5 from Process 2-20: ❌ rejected ("nonce too low" or "replacement underpriced")
```

**Failure rate:** 95% of transactions fail (19 out of 20 in this example).

### Error Messages You'll See

```
Error: (code: -32000, message: nonce too low: address 0x..., tx: 5 state: 6, data: None)
Error: (code: -32000, message: replacement transaction underpriced, data: None)
```

---

## Solution 1: Multiple Sender Accounts (Round-Robin) ✅ **IMPLEMENTED**

**How it works:**
- Use multiple funded accounts (e.g., 3-5 devnet accounts)
- Round-robin: TX 1 → Account A, TX 2 → Account B, TX 3 → Account C, TX 4 → Account A, ...
- Each account manages its own nonce independently
- No coordination needed between processes

**Advantages:**
- ✅ Simple implementation
- ✅ No race conditions
- ✅ Each account's nonce is managed by the node automatically
- ✅ Works with any Ethereum node (no special RPC support needed)
- ✅ Safe for devnet (uses existing funded test accounts)

**Implementation in perf-baseline.sh:**

```bash
# Pool of devnet accounts (from config/devnet/.env)
SENDER_KEYS=(
  $TEST_SENDER_KEY
  $OP_PROPOSER_PRIVATE_KEY
  $OP_CHALLENGER_PRIVATE_KEY
)

# Round-robin parallel sends
for i in 1..$TX_COUNT; do
  SENDER_IDX=$(( (i - 1) % ${#SENDER_KEYS[@]} ))
  KEY="${SENDER_KEYS[$SENDER_IDX]}"
  
  (cast send --private-key "$KEY" ...) &
done
wait
```

**Result:**
- TX 1: Account A, nonce=10
- TX 2: Account B, nonce=5
- TX 3: Account C, nonce=3
- TX 4: Account A, nonce=11  (A's next nonce)
- TX 5: Account B, nonce=6   (B's next nonce)
- ...

**No collisions!** Each account has its own nonce sequence.

---

## Solution 2: Pre-signed Transactions with Manual Nonce Management (Advanced)

**How it works:**
1. Query current nonce for the account: `N`
2. Pre-sign 20 transactions with nonces: N, N+1, N+2, ..., N+19
3. Submit all 20 signed raw transactions in parallel via `eth_sendRawTransaction`

**Advantages:**
- ✅ Single account (if that's a requirement)
- ✅ Guaranteed no nonce conflicts (you control nonces)
- ✅ Maximum throughput

**Disadvantages:**
- ❌ Complex: requires manual nonce tracking
- ❌ Fragile: if any TX fails (e.g., insufficient gas), all subsequent TXs are blocked
- ❌ Race risk: another process/script could send a TX from the same account, breaking your nonce sequence

**Example (pseudo-code):**

```bash
# Query starting nonce
NONCE=$(cast nonce --rpc-url $RPC $SENDER_ADDR)

# Pre-sign all TXs
for i in 0..19; do
  TX_NONCE=$((NONCE + i))
  SIGNED=$(cast mktx --nonce $TX_NONCE --private-key $KEY ...)
  
  # Submit raw signed TX in parallel
  (cast publish --rpc-url $RPC $SIGNED) &
done
wait
```

**Not recommended for general use** — Solution 1 is simpler and more robust.

---

## Solution 3: Node-level Nonce Management (Requires Node Support)

Some Ethereum clients support **pending nonce** queries that account for transactions in the mempool:

```bash
# Query nonce including pending TXs
cast nonce --rpc-url $RPC $ADDR --pending
```

**How it helps:**
- Process 1 submits TX with nonce=5
- Process 2 queries pending nonce → receives "6" (accounts for Process 1's pending TX)
- Process 3 queries pending nonce → receives "7"

**Limitations:**
- ❌ Still has race conditions if queries happen simultaneously before mempool updates
- ❌ Not all nodes support accurate pending nonce tracking
- ❌ Doesn't help if you're using `cast send` (which doesn't use `--pending` flag by default)

**Verdict:** Not reliable for high-throughput parallel submission.

---

## Devnet Setup Requirements for Parallel Mode

### Prerequisite: Multiple Funded Accounts on L2

The parallel mode in `perf-baseline.sh` requires at least 2 funded accounts. By default, it uses:

1. `TEST_SENDER_KEY` (already used in serial mode)
2. `OP_PROPOSER_PRIVATE_KEY`
3. `OP_CHALLENGER_PRIVATE_KEY`

**Check funding status:**

```bash
# Source env vars
source config/devnet/.env

# Check each account balance on L2
for KEY in "$TEST_SENDER_KEY" "$OP_PROPOSER_PRIVATE_KEY" "$OP_CHALLENGER_PRIVATE_KEY"; do
  ADDR=$(cast wallet address "$KEY")
  BAL=$(cast balance --rpc-url http://localhost:8123 "$ADDR" --ether)
  echo "$ADDR: $BAL ETH"
done
```

**Fund accounts if needed:**

```bash
# From a funded L2 account (e.g., RICH_L1_PRIVATE_KEY after bridging)
cast send --rpc-url http://localhost:8123 \
  --private-key $RICH_ACCOUNT_KEY \
  --value 10ether \
  $TARGET_ADDR
```

### Automatic Balance Checking

The script automatically:
- Checks each candidate account's L2 balance
- Skips accounts with < 0.1 ETH
- Fails with a clear error if fewer than 2 funded accounts are available

**Error message if insufficient accounts:**

```
❌ Parallel mode requires at least 2 funded accounts. Found: 1
  Ensure OP_PROPOSER_PRIVATE_KEY and OP_CHALLENGER_PRIVATE_KEY accounts are funded on L2.
```

---

## Performance Comparison: Serial vs Parallel

### Serial Mode (Original)

```bash
./scripts/devnet/perf-baseline.sh --count 20
```

**Typical results:**
- Submission time: 30-40s
- Submission rate: 0.5-0.7 TX/s
- Time-to-unsafe: 15-25s (average includes queueing delay)
- Effective TPS: 0.4-0.6

**Bottleneck:** RPC round-trip time for each `cast send` call.

---

### Parallel Mode (Multi-Sender)

```bash
./scripts/devnet/perf-baseline.sh --parallel --count 20
```

**Expected results:**
- Submission time: 2-5s (all TXs sent concurrently)
- Submission rate: 4-10 TX/s (limited by node processing, not client)
- Time-to-unsafe: 1-3s (minimal queueing)
- Effective TPS: 3-8 (approaches sequencer block production rate)

**Bottleneck:** Sequencer block building and node RPC processing capacity.

---

## Safety Notes

### Devnet Only ⚠️

The accounts used in parallel mode are **Foundry's default test accounts** with well-known private keys. These are:

- ✅ Safe for local devnet testing
- ❌ **NEVER use on mainnet or public testnets** (funds will be stolen immediately)

### Private Key Security

The script reads private keys from `config/devnet/.env`, which is `.gitignore`d. Always:

- ✅ Keep `.env` out of version control
- ✅ Use separate keys for production
- ✅ Never commit private keys to git

---

## Troubleshooting

### "Parallel mode requires at least 2 funded accounts"

**Common causes:**

#### 1. Duplicate Keys in .env (Most Common)

Your `.env` might have the same private key for multiple accounts:

```bash
# BAD: Same key used for multiple accounts
TEST_SENDER_KEY=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d
OP_PROPOSER_PRIVATE_KEY=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d  # SAME!
OP_CHALLENGER_PRIVATE_KEY=0x8b3a350cf5c34c9194ca9aa3f146b2b9afed22cd83d3c5f6a3f2f243ce220c01
```

**Check if you have duplicates:**

```bash
source config/devnet/.env
echo "TEST_SENDER:   $(cast wallet address "$TEST_SENDER_KEY")"
echo "OP_PROPOSER:   $(cast wallet address "$OP_PROPOSER_PRIVATE_KEY")"
echo "OP_CHALLENGER: $(cast wallet address "$OP_CHALLENGER_PRIVATE_KEY")"
```

If two addresses are the same, the script will skip the duplicate and you'll only have 1 unique account.

**Fix: Use Foundry's default test accounts (recommended for devnet)**

Foundry provides 10 test accounts with known private keys. Use different ones from the set:

```bash
# In config/devnet/.env, use distinct Foundry accounts:
TEST_SENDER_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80        # Account 0
OP_PROPOSER_PRIVATE_KEY=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d   # Account 1
OP_CHALLENGER_PRIVATE_KEY=0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a # Account 2
```

Foundry default accounts reference: https://book.getfoundry.sh/reference/forge/forge-test#test-accounts

#### 2. Unfunded Accounts

**Check balances:**

```bash
source config/devnet/.env

for KEY in "$TEST_SENDER_KEY" "$OP_PROPOSER_PRIVATE_KEY" "$OP_CHALLENGER_PRIVATE_KEY"; do
  ADDR=$(cast wallet address "$KEY")
  BAL=$(cast balance --rpc-url http://localhost:8123 "$ADDR" --ether)
  echo "$ADDR: $BAL ETH"
done
```

**Fund the accounts:**

```bash
# Fund proposer (if needed)
cast send --rpc-url http://localhost:8123 \
  --private-key $TEST_SENDER_KEY \
  --value 10ether \
  $(cast wallet address "$OP_PROPOSER_PRIVATE_KEY")

# Fund challenger (if needed)
cast send --rpc-url http://localhost:8123 \
  --private-key $TEST_SENDER_KEY \
  --value 10ether \
  $(cast wallet address "$OP_CHALLENGER_PRIVATE_KEY")
```

**Wait for confirmation:**

```bash
# Check the balance updated
cast balance --rpc-url http://localhost:8123 \
  $(cast wallet address "$OP_CHALLENGER_PRIVATE_KEY") \
  --ether
```

### Some TXs Show "FAILED" in Parallel Mode

**Possible causes:**
1. Node RPC rate limiting (too many concurrent requests)
2. Insufficient funds in one of the sender accounts
3. Node processing backlog

**Debug:**
- Check node logs: `tail -f logs/xlayer-node.log`
- Reduce `--count` to test with fewer TXs
- Verify all sender account balances

### Parallel Mode Still Shows Low TPS

**Possible causes:**
1. Node itself is slow (CPU/disk bottleneck)
2. Block production cadence is slow (check L2 block time)
3. RPC processing capacity limit

**Check:**
- Node resource usage: `docker stats` or `top`
- Block production rate: `cast block latest --rpc-url http://localhost:8123 --json | jq .timestamp` (repeat)
- RPC latency: `time cast bn --rpc-url http://localhost:8123`

---

## Best Practices

### For Load Testing

1. **Use parallel mode** with multiple sender accounts
2. **Monitor node health** during the test (use `4-health-check.sh`)
3. **Start small** (20-50 TXs) and scale up gradually
4. **Check for failures** in the output and node logs
5. **Measure over multiple runs** to account for variability

### For Production Benchmarking

1. Use dedicated load-testing accounts (not operator keys)
2. Monitor system resources (CPU, memory, disk I/O)
3. Compare against baseline metrics from `docs/performance/`
4. Test different TX types (simple transfers, contract calls, large calldata)

---

## Related Documentation

- [Block Transitions](../concepts/block-transitions.md) — Understanding unsafe/safe/finalized progression
- [Devnet Commands](../commands/devnet-commands.md) — Quick reference for devnet operations
- [Health Check Script](../../scripts/devnet/4-health-check.sh) — Monitor node status during load tests

---

## Summary

| Approach | Nonce Race Risk | TX Failure Rate | Complexity | Recommended |
|----------|----------------|-----------------|------------|-------------|
| **Serial (single account)** | None | 0% | Low | ✅ Default |
| **Parallel (single account, naive)** | High | ~95% | Low | ❌ Never use |
| **Parallel (multi-account round-robin)** | None | 0% | Low | ✅ **Best for load testing** |
| **Parallel (pre-signed, manual nonce)** | Low | <5% | High | ⚠️ Advanced use only |

**Use `--parallel` mode safely:** The script implements multi-account round-robin, so there are **no nonce races** and **no failed transactions** due to nonce conflicts.

