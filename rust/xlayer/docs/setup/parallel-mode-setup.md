# Parallel Mode Setup Guide

## Quick Setup (3 Steps)

### Step 1: Update `.env` with Distinct Keys

Edit `config/devnet/.env` and ensure you have **3 distinct** Foundry test accounts:

```bash
# Use these distinct private keys (from Foundry's default test accounts)
TEST_SENDER_KEY=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d        # Account 1
OP_PROPOSER_PRIVATE_KEY=0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a   # Account 2
OP_CHALLENGER_PRIVATE_KEY=0x8b3a350cf5c34c9194ca9aa3f146b2b9afed22cd83d3c5f6a3f2f243ce220c01 # Account 5
```

**These map to addresses:**
- Account 1: `0x70997970C51812dc3A010C7d01b50e0d17dc79C8`
- Account 2: `0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC`
- Account 5: `0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc`

### Step 2: Start Your L2 Node (if not already running)

```bash
./scripts/devnet/0-all.sh
```

### Step 3: Fund the Additional Accounts

Only Account 1 (TEST_SENDER_KEY) is funded by default. Fund the other two:

```bash
source config/devnet/.env

# Fund Account 2 (OP_PROPOSER)
cast send --rpc-url http://localhost:8123 \
  --private-key $TEST_SENDER_KEY \
  --value 50ether \
  0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC

# Fund Account 5 (OP_CHALLENGER)
cast send --rpc-url http://localhost:8123 \
  --private-key $TEST_SENDER_KEY \
  --value 50ether \
  0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc
```

Wait 1-2 seconds for blocks to be produced, then verify:

```bash
# Check all balances
cast balance --rpc-url http://localhost:8123 0x70997970C51812dc3A010C7d01b50e0d17dc79C8 --ether
cast balance --rpc-url http://localhost:8123 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC --ether
cast balance --rpc-url http://localhost:8123 0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc --ether
```

You should see ~50 ETH in each of the last two accounts.

### Step 4: Run Parallel Mode

```bash
./scripts/devnet/perf-baseline.sh --parallel --count 20
```

**Expected output:**

```
ℹ  Parallel mode: using multiple sender accounts to avoid nonce races
  ✓ 0x70997970C51812dc3A010C7d01b50e0d17dc79C8 (999999999999.9 ETH)
  ✓ 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC (50.0 ETH)
  ✓ 0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc (50.0 ETH)
ℹ  Using 3 sender accounts for parallel submission
ℹ  Sending 20 transactions...

  sent 1/20  0x...
  sent 2/20  0x...
  ...
✅ Sent 20 TXs in 3s (6.7 TX/s submission rate)
```

---

## FAQ

### Q: How many keys do I really need?

**Minimum:** 2 distinct accounts  
**Recommended:** 3 distinct accounts  
**For high load (100+ TXs):** Consider 4-5 accounts

**Why 3?** With 20 TXs and 3 accounts:
- Each account sends ~7 TXs
- Lower nonce contention
- Better distribution across accounts

### Q: Why are the private keys visible?

These are **Foundry's well-known test accounts**. They are:
- ✅ Safe for local devnet
- ✅ Safe for testing/CI
- ❌ **NEVER use on mainnet or public testnets** (funds will be stolen)

See: https://book.getfoundry.sh/reference/forge/forge-test#test-accounts

### Q: Can I use my own keys?

Yes! Generate new keys:

```bash
# Generate 3 new accounts
cast wallet new
cast wallet new
cast wallet new
```

Then add the private keys to `.env` and fund them on L2.

### Q: What if I only have 2 accounts funded?

That's fine! The script requires a minimum of 2. You'll see:

```
ℹ  Using 2 sender accounts for parallel submission
```

With 20 TXs, each account will send 10 TXs.

### Q: What was the bash error about?

The script originally used bash 4+ associative arrays (`declare -A`), but macOS ships with bash 3.2 by default.

**Fixed:** Now uses a simple string-based deduplication approach that works with bash 3.2.

---

## Troubleshooting

### Error: "Parallel mode requires at least 2 funded accounts. Found: 1"

**Cause:** Only one unique account is configured, or the others have 0 balance.

**Fix:**
1. Check your `.env` — ensure TEST_SENDER_KEY, OP_PROPOSER_PRIVATE_KEY, and OP_CHALLENGER_PRIVATE_KEY use **different private keys**
2. Fund the accounts (see Step 3 above)

### Error: "declare: -A: invalid option"

**Cause:** Your bash version is < 4.0 (macOS default is 3.2).

**Status:** ✅ **FIXED** — Script now works with bash 3.2

### Warning: "Skipping 0x... (insufficient balance)"

**Cause:** Account has < 0.1 ETH.

**Fix:** Fund the account:

```bash
cast send --rpc-url http://localhost:8123 \
  --private-key $TEST_SENDER_KEY \
  --value 50ether \
  <address_to_fund>
```

---

## Performance Expectations

### Serial Mode (--no-parallel)
- Submission rate: 0.5-0.7 TX/s
- Time-to-unsafe: 15-25s average
- Effective TPS: 0.4-0.6

### Parallel Mode (--parallel, 3 accounts)
- Submission rate: 4-10 TX/s
- Time-to-unsafe: 1-3s average
- Effective TPS: 3-8

**Expected speedup:** 8-10x faster

---

## Summary Commands

```bash
# 1. Update .env with distinct keys (use the values from Step 1)
vim config/devnet/.env

# 2. Start L2 (if not running)
./scripts/devnet/0-all.sh

# 3. Fund accounts (copy-paste the commands from Step 3)
source config/devnet/.env
cast send --rpc-url http://localhost:8123 --private-key $TEST_SENDER_KEY --value 50ether 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC
cast send --rpc-url http://localhost:8123 --private-key $TEST_SENDER_KEY --value 50ether 0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc

# 4. Verify balances
cast balance --rpc-url http://localhost:8123 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC --ether
cast balance --rpc-url http://localhost:8123 0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc --ether

# 5. Run parallel test
./scripts/devnet/perf-baseline.sh --parallel --count 20
```

Done! 🚀

