# XLayer Devnet Commands — One-Stop Reference

All commands assume the devnet is running (`./scripts/devnet/start-all.sh`).
Tools required: `cast` (Foundry), `curl`, `jq`.

```
L1 ETH RPC      → http://localhost:8545
L1 Beacon REST  → http://localhost:3500
L2 ETH RPC      → http://localhost:8123
L2 Rollup RPC   → http://localhost:9545   (kona-node / op-node)
L2 RPC node 1   → http://localhost:8124
L2 RPC node 2   → http://localhost:8128
Conductor       → http://localhost:8547
Prometheus      → http://localhost:9090
Grafana         → http://localhost:3000
```

---

## 1. Chain Health & Sync Status

### Is everything alive?
```bash
# L1 geth alive?
cast block-number --rpc-url http://localhost:8545

# L2 reth alive?
cast block-number --rpc-url http://localhost:8123

# kona/op-node alive?
curl -sf -X POST http://localhost:9545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' | jq .

# All three in one shot
echo "L1:" $(cast block-number --rpc-url http://localhost:8545) \
  "  L2:" $(cast block-number --rpc-url http://localhost:8123) \
  "  L2-RPC:" $(cast block-number --rpc-url http://localhost:8124)
```

### OP Stack phase heads (the most useful single command)
```bash
curl -s -X POST http://localhost:9545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' \
  | jq '{
      unsafe:    .unsafe_l2    | {number, hash},
      safe:      .safe_l2      | {number, hash},
      finalized: .finalized_l2 | {number, hash},
      l1_head:   .head_l1      | {number, hash},
      l1_safe:   .safe_l1      | {number, hash}
    }'
```

### Watch heads live (refreshes every 2s)
```bash
watch -n 2 "curl -s -X POST http://localhost:9545 \
  -H 'Content-Type: application/json' \
  -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"optimism_syncStatus\",\"params\":[]}' \
  | jq '{unsafe: .unsafe_l2.number, safe: .safe_l2.number, finalized: .finalized_l2.number}'"
```

### Gap between unsafe and safe (batcher health indicator)
```bash
STATUS=$(curl -s -X POST http://localhost:9545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}')
UNSAFE=$(echo $STATUS | jq .unsafe_l2.number)
SAFE=$(echo $STATUS | jq .safe_l2.number)
echo "Gap: $((UNSAFE - SAFE)) blocks  (unsafe=$UNSAFE safe=$SAFE)"
```

### Rollup config (chain parameters)
```bash
curl -s -X POST http://localhost:9545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"optimism_rollupConfig","params":[]}' \
  | jq '{
      chain_id:         .l2_chain_id,
      block_time:       .block_time,
      seq_window:       .seq_window_size,
      max_seq_drift:    .max_sequencer_drift,
      regolith_time:    .regolith_time,
      canyon_time:      .canyon_time,
      delta_time:       .delta_time,
      ecotone_time:     .ecotone_time,
      fjord_time:       .fjord_time,
      granite_time:     .granite_time,
      holocene_time:    .holocene_time,
      isthmus_time:     .isthmus_time
    }'
```

### Output root at a block
```bash
# At latest safe block
curl -s -X POST http://localhost:9545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"optimism_outputAtBlock","params":["latest"]}' \
  | jq .

# At specific block number (hex)
curl -s -X POST http://localhost:9545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"optimism_outputAtBlock","params":["0x831234"]}' \
  | jq .
```

---

## 2. L2 Block & Transaction Queries (http://localhost:8123)

### Block info
```bash
# Latest block (summary)
cast block latest --rpc-url http://localhost:8123

# Latest block (full JSON)
cast block latest --rpc-url http://localhost:8123 --json | jq .

# Block by number
cast block 8595765 --rpc-url http://localhost:8123

# Block by number (JSON, with transactions)
curl -s -X POST http://localhost:8123 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"eth_getBlockByNumber","params":["0x82FC3D",true]}' \
  | jq '{number: .result.number, hash: .result.hash, txCount: (.result.transactions | length), gasUsed: .result.gasUsed}'

# Block timestamp (convert to human-readable)
cast block latest --rpc-url http://localhost:8123 --json \
  | jq '.timestamp | tonumber | strftime("%Y-%m-%d %H:%M:%S UTC")'
```

### Transaction queries
```bash
# TX receipt
cast receipt --rpc-url http://localhost:8123 <TX_HASH>

# TX receipt (JSON)
cast receipt --rpc-url http://localhost:8123 <TX_HASH> --json | jq .

# TX details
cast tx --rpc-url http://localhost:8123 <TX_HASH>

# TX status (1 = success, 0 = reverted)
cast receipt --rpc-url http://localhost:8123 <TX_HASH> --json | jq .status

# TX block number
cast receipt --rpc-url http://localhost:8123 <TX_HASH> --json | jq .blockNumber

# Is TX safe? (compare TX block to safe head)
TX_BLOCK=$(cast receipt --rpc-url http://localhost:8123 <TX_HASH> --json | jq -r .blockNumber | xargs printf '%d\n')
SAFE=$(curl -s -X POST http://localhost:9545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' \
  | jq .safe_l2.number)
[ "$TX_BLOCK" -le "$SAFE" ] && echo "SAFE ✓" || echo "UNSAFE (pending batch)"

# All TXs in a block
curl -s -X POST http://localhost:8123 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"eth_getBlockByNumber","params":["latest",true]}' \
  | jq '[.result.transactions[] | {hash: .hash, from: .from, to: .to, value: .value}]'
```

### Full TX lifecycle check (unsafe → safe → finalized)
```bash
TX=<TX_HASH>
TX_BLOCK=$(cast receipt --rpc-url http://localhost:8123 $TX --json | jq -r .blockNumber | xargs printf '%d\n')
STATUS=$(curl -s -X POST http://localhost:9545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}')
UNSAFE=$(echo $STATUS | jq .unsafe_l2.number)
SAFE=$(echo $STATUS | jq .safe_l2.number)
FINAL=$(echo $STATUS | jq .finalized_l2.number)

echo "TX in block: $TX_BLOCK"
[ "$TX_BLOCK" -le "$UNSAFE" ] && echo "UNSAFE  ✓" || echo "UNSAFE  ✗"
[ "$TX_BLOCK" -le "$SAFE"   ] && echo "SAFE    ✓" || echo "SAFE    ✗ (waiting for batch)"
[ "$TX_BLOCK" -le "$FINAL"  ] && echo "FINAL   ✓" || echo "FINAL   ✗ (waiting for L1 finality)"
```

---

## 3. Accounts & Balances (http://localhost:8123)

### Balance
```bash
# ETH balance
cast balance --rpc-url http://localhost:8123 <ADDRESS>

# ETH balance formatted in ether
cast balance --rpc-url http://localhost:8123 <ADDRESS> --ether

# Balance at a specific block
cast balance --rpc-url http://localhost:8123 <ADDRESS> --block 8595765
```

### Nonce
```bash
cast nonce --rpc-url http://localhost:8123 <ADDRESS>

# Pending nonce (includes mempool TXs)
cast nonce --rpc-url http://localhost:8123 <ADDRESS> --block pending
```

### Code (is this a contract?)
```bash
cast code --rpc-url http://localhost:8123 <ADDRESS>
# Returns "0x" for EOA, bytecode for contract
```

### Storage slot
```bash
cast storage --rpc-url http://localhost:8123 <CONTRACT> <SLOT_HEX>

# Example: read slot 0 of a contract
cast storage --rpc-url http://localhost:8123 0xDeadBeef...123 0x0
```

---

## 4. Sending Transactions (http://localhost:8123)

### ETH transfers
```bash
# Send ETH
cast send --rpc-url http://localhost:8123 \
  --private-key <PRIVATE_KEY> \
  <RECIPIENT> \
  --value 0.1ether

# Send ETH and wait for receipt
cast send --rpc-url http://localhost:8123 \
  --private-key <PRIVATE_KEY> \
  <RECIPIENT> \
  --value 1ether \
  --confirmations 1

# Send with explicit gas
cast send --rpc-url http://localhost:8123 \
  --private-key <PRIVATE_KEY> \
  <RECIPIENT> \
  --value 0.01ether \
  --gas-limit 21000 \
  --gas-price 1gwei
```

### Contract calls (read)
```bash
# Call a view function
cast call --rpc-url http://localhost:8123 \
  <CONTRACT> \
  "balanceOf(address)(uint256)" \
  <ADDRESS>

# Call at a specific block
cast call --rpc-url http://localhost:8123 \
  <CONTRACT> \
  "totalSupply()(uint256)" \
  --block 8595765
```

### Contract calls (write)
```bash
cast send --rpc-url http://localhost:8123 \
  --private-key <PRIVATE_KEY> \
  <CONTRACT> \
  "transfer(address,uint256)" \
  <RECIPIENT> \
  1000000000000000000
```

### Gas estimation
```bash
cast estimate --rpc-url http://localhost:8123 \
  <CONTRACT> \
  "transfer(address,uint256)" \
  <RECIPIENT> \
  1000000000000000000
```

### Current gas price
```bash
cast gas-price --rpc-url http://localhost:8123

# Base fee of latest block
cast block latest --rpc-url http://localhost:8123 --json | jq .baseFeePerGas
```

---

## 5. Mempool / txpool (http://localhost:8123)

```bash
# Mempool summary (pending + queued counts)
curl -s -X POST http://localhost:8123 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"txpool_status","params":[]}' \
  | jq '{pending: .result.pending, queued: .result.queued}'

# Pending TXs (full, can be large)
curl -s -X POST http://localhost:8123 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"txpool_inspect","params":[]}' \
  | jq .result.pending

# Is a specific TX in the mempool?
curl -s -X POST http://localhost:8123 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"eth_getTransactionByHash","params":["<TX_HASH>"]}' \
  | jq 'if .result.blockNumber == null then "PENDING (in mempool)" else "MINED in block \(.result.blockNumber)" end'

# Pending TXs for a specific address
curl -s -X POST http://localhost:8123 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"txpool_content","params":[]}' \
  | jq '.result.pending["<ADDRESS_LOWERCASE>"]'
```

---

## 6. Debug & Trace (http://localhost:8123)

```bash
# Trace a transaction (op-reth supports this)
curl -s -X POST http://localhost:8123 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"debug_traceTransaction","params":["<TX_HASH>",{}]}' \
  | jq '{gas: .result.gas, failed: .result.failed, returnValue: .result.returnValue}'

# Trace with call tracer (cleaner output)
curl -s -X POST http://localhost:8123 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"debug_traceTransaction","params":["<TX_HASH>",{"tracer":"callTracer"}]}' \
  | jq .result

# eth_getLogs — events from a contract
cast logs --rpc-url http://localhost:8123 \
  --from-block 8595000 \
  --to-block latest \
  --address <CONTRACT>

# eth_getLogs with topic filter
curl -s -X POST http://localhost:8123 \
  -H 'Content-Type: application/json' \
  -d '{
    "jsonrpc":"2.0","id":1,"method":"eth_getLogs",
    "params":[{
      "fromBlock":"0x82FC00",
      "toBlock":"latest",
      "address":"<CONTRACT>",
      "topics":["<EVENT_TOPIC_HASH>"]
    }]
  }' | jq .result

# EIP-1186 storage proof
curl -s -X POST http://localhost:8123 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"eth_getProof","params":["<ADDRESS>",["0x0"],"latest"]}' \
  | jq '{balance: .result.balance, nonce: .result.nonce, storageHash: .result.storageHash}'
```

---

## 7. L1 Queries (http://localhost:8545)

```bash
# L1 block number
cast block-number --rpc-url http://localhost:8545

# L1 latest block
cast block latest --rpc-url http://localhost:8545

# L1 account balance
cast balance --rpc-url http://localhost:8545 <ADDRESS> --ether

# L1 gas price
cast gas-price --rpc-url http://localhost:8545

# L1 base fee
cast block latest --rpc-url http://localhost:8545 --json | jq .baseFeePerGas

# L1 TX receipt (e.g. for a batch submission)
cast receipt --rpc-url http://localhost:8545 <L1_TX_HASH>

# L1 finalized block (post-merge)
curl -s -X POST http://localhost:8545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"eth_getBlockByNumber","params":["finalized",false]}' \
  | jq '{number: .result.number, hash: .result.hash}'

# L1 safe block
curl -s -X POST http://localhost:8545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"eth_getBlockByNumber","params":["safe",false]}' \
  | jq '{number: .result.number, hash: .result.hash}'

# Send ETH on L1
cast send --rpc-url http://localhost:8545 \
  --private-key <PRIVATE_KEY> \
  <RECIPIENT> \
  --value 1ether
```

---

## 8. L1 Beacon REST (http://localhost:3500)

```bash
# Beacon node version
curl -s http://localhost:3500/eth/v1/node/version | jq .

# Beacon node sync status
curl -s http://localhost:3500/eth/v1/node/syncing | jq .

# Current slot / epoch
curl -s http://localhost:3500/eth/v1/beacon/headers/head \
  | jq '{slot: .data.header.message.slot, root: .data.root}'

# Finalized checkpoint
curl -s http://localhost:3500/eth/v1/beacon/states/head/finality_checkpoints \
  | jq '{finalized: .data.finalized, justified: .data.current_justified}'

# Blob sidecars for a slot (Cancun+)
SLOT=<SLOT_NUMBER>
curl -s "http://localhost:3500/eth/v1/beacon/blob_sidecars/$SLOT" | jq .

# Genesis info
curl -s http://localhost:3500/eth/v1/beacon/genesis | jq .

# Validator count
curl -s "http://localhost:3500/eth/v1/beacon/states/head/validators" \
  | jq '[.data[] | select(.status == "active_ongoing")] | length'

# Node peer count
curl -s http://localhost:3500/eth/v1/node/peer_count | jq .
```

---

## 9. kona-node / op-node Admin RPC (http://localhost:9545)

```bash
# Full sync status
curl -s -X POST http://localhost:9545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' | jq .

# Rollup config
curl -s -X POST http://localhost:9545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"optimism_rollupConfig","params":[]}' | jq .

# Output root at block
curl -s -X POST http://localhost:9545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"optimism_outputAtBlock","params":["latest"]}' | jq .

# Peer list (P2P peers gossip)
curl -s -X POST http://localhost:9545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"opp2p_peers","params":[true]}' | jq .

# Self peer info
curl -s -X POST http://localhost:9545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"opp2p_self","params":[]}' | jq .

# Sequencer active?
curl -s -X POST http://localhost:9545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"admin_sequencerActive","params":[]}' | jq .

# Start sequencer (admin)
curl -s -X POST http://localhost:9545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"admin_startSequencer","params":["<UNSAFE_HEAD_HASH>"]}' | jq .

# Stop sequencer (admin)
curl -s -X POST http://localhost:9545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"admin_stopSequencer","params":[]}' | jq .

# Reset (dangerous — only for recovery)
curl -s -X POST http://localhost:9545 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"admin_resetDerivationPipeline","params":[]}' | jq .
```

---

## 10. Miner Namespace — op-reth (http://localhost:8123)

```bash
# Confirm miner namespace is exposed (needed by op-batcher)
curl -s -X POST http://localhost:8123 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"miner_setMaxDASize","params":["0x0","0x0"]}' \
  | jq .result
# Should return: true

# Set DA size limits (op-batcher throttle control)
curl -s -X POST http://localhost:8123 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"miner_setMaxDASize","params":["0x7A120","0x7A120"]}' \
  | jq .result
```

---

## 11. Conductor HA (http://localhost:8547)

```bash
# Conductor active status
curl -s -X POST http://localhost:8547 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"conductor_active","params":[]}' | jq .

# Conductor leader?
curl -s -X POST http://localhost:8547 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"conductor_leader","params":[]}' | jq .

# Cluster membership
curl -s -X POST http://localhost:8547 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"conductor_clusterMembership","params":[]}' | jq .

# Sequencer health via conductor proxy
curl -s -X POST http://localhost:8547 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"conductor_sequencerHealthy","params":[]}' | jq .
```

---

## 12. Prometheus Metrics (http://localhost:9090)

```bash
# Query via Prometheus HTTP API
PROM=http://localhost:9090

# reth block processing rate (TPS proxy)
curl -sg "$PROM/api/v1/query?query=rate(reth_transaction_pool_inserted_transactions_total[1m])" \
  | jq '.data.result[0].value[1]'

# reth canonical chain tip block number
curl -sg "$PROM/api/v1/query?query=reth_blockchain_tree_canonical_chain_height" \
  | jq '.data.result[0].value[1]'

# kona-node unsafe head
curl -sg "$PROM/api/v1/query?query=op_node_head_l2_block_number" \
  | jq '.data.result[0].value[1]' 2>/dev/null || echo "(metric name may vary)"

# reth p2p peer count
curl -sg "$PROM/api/v1/query?query=reth_network_peers" \
  | jq '.data.result[0].value[1]'

# List ALL metric names (useful for discovery)
curl -sg "$PROM/api/v1/label/__name__/values" | jq '.data | sort[]' | grep reth
curl -sg "$PROM/api/v1/label/__name__/values" | jq '.data | sort[]' | grep op_

# reth pre-warming metrics (if feature enabled)
curl -sg "$PROM/api/v1/query?query=reth_pre_warming_cache_hits_total" \
  | jq '.data.result'
curl -sg "$PROM/api/v1/query?query=reth_pre_warming_simulations_total" \
  | jq '.data.result'
```

---

## 13. Grafana (http://localhost:3000)

```
URL:      http://localhost:3000
Login:    admin / admin

Key dashboards (navigate via left sidebar → Dashboards):
- reth metrics    → block time, TPS, peer count, DB size
- op-node metrics → sync status, derivation pipeline
```

---

## 14. RPC Node Verification (http://localhost:8124 / 8128)

These are the non-sequencer RPC nodes for read queries. Same commands as 8123 but read-only (no sequencing).

```bash
# Confirm RPC node is syncing with sequencer
cast block-number --rpc-url http://localhost:8124
cast block-number --rpc-url http://localhost:8128

# Compare against sequencer
echo "Sequencer: $(cast block-number --rpc-url http://localhost:8123)"
echo "RPC node1: $(cast block-number --rpc-url http://localhost:8124)"
echo "RPC node2: $(cast block-number --rpc-url http://localhost:8128)"

# RPC node sync status (should match sequencer unsafe head)
curl -s -X POST http://localhost:8124 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"eth_syncing","params":[]}' | jq .
# Returns false when synced, or sync progress object when catching up
```

---

## 15. Full Devnet Health Check Script

Save as `devnet-health.sh` and run before any test session.

```bash
#!/usr/bin/env bash
set -euo pipefail

L1=http://localhost:8545
L2=http://localhost:8123
ROLLUP=http://localhost:9545
BEACON=http://localhost:3500

ok()   { printf "\033[32m✓\033[0m %s\n" "$1"; }
fail() { printf "\033[31m✗\033[0m %s\n" "$1"; }
info() { printf "  %s\n" "$1"; }

echo "=== XLayer Devnet Health Check ==="
echo ""

# L1
L1_BLOCK=$(cast block-number --rpc-url $L1 2>/dev/null) && ok "L1 geth  (block $L1_BLOCK)" || fail "L1 geth unreachable"

# L1 Beacon
BEACON_SLOT=$(curl -sf http://localhost:3500/eth/v1/beacon/headers/head 2>/dev/null | jq -r .data.header.message.slot) \
  && ok "L1 beacon (slot $BEACON_SLOT)" || fail "L1 beacon unreachable"

# L2
L2_BLOCK=$(cast block-number --rpc-url $L2 2>/dev/null) && ok "L2 reth  (block $L2_BLOCK)" || fail "L2 reth unreachable"

# kona/op-node
STATUS=$(curl -sf -X POST $ROLLUP \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' 2>/dev/null)
if [ -n "$STATUS" ]; then
  UNSAFE=$(echo $STATUS | jq .unsafe_l2.number)
  SAFE=$(echo $STATUS | jq .safe_l2.number)
  FINAL=$(echo $STATUS | jq .finalized_l2.number)
  ok "kona-node rollup RPC"
  info "unsafe=$UNSAFE  safe=$SAFE  finalized=$FINAL  gap=$((UNSAFE-SAFE))"
else
  fail "kona-node rollup RPC unreachable"
fi

# Miner namespace
MINER=$(curl -sf -X POST $L2 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"miner_setMaxDASize","params":["0x0","0x0"]}' 2>/dev/null | jq -r .result)
[ "$MINER" = "true" ] && ok "miner namespace (op-batcher ready)" || fail "miner namespace not exposed — restart with --http.api miner"

echo ""
echo "=== Done ==="
```

---

## 16. Quick Test: Send TX and Track All Phases

```bash
#!/usr/bin/env bash
# Usage: ./track-tx.sh <private-key> <recipient> <value-in-ether>
PRIVKEY=$1
RECIPIENT=$2
VALUE=${3:-0.01}

echo "Sending $VALUE ETH to $RECIPIENT..."
TX=$(cast send --rpc-url http://localhost:8123 \
  --private-key $PRIVKEY \
  $RECIPIENT \
  --value ${VALUE}ether \
  --json | jq -r .transactionHash)

echo "TX: $TX"
echo "Waiting for inclusion..."

# Wait for inclusion
while true; do
  RECEIPT=$(cast receipt --rpc-url http://localhost:8123 $TX --json 2>/dev/null)
  TX_BLOCK=$(echo $RECEIPT | jq -r .blockNumber 2>/dev/null)
  [ -n "$TX_BLOCK" ] && [ "$TX_BLOCK" != "null" ] && break
  sleep 1
done
TX_BLOCK_DEC=$(printf '%d' $TX_BLOCK)
echo "  UNSAFE ✓  (block $TX_BLOCK_DEC)"

# Wait for safe
echo "Waiting for SAFE..."
while true; do
  SAFE=$(curl -s -X POST http://localhost:9545 \
    -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' \
    | jq .safe_l2.number)
  [ "$TX_BLOCK_DEC" -le "$SAFE" ] && break
  sleep 3
done
echo "  SAFE    ✓  (safe_head=$SAFE)"

# Wait for finalized
echo "Waiting for FINALIZED..."
while true; do
  FINAL=$(curl -s -X POST http://localhost:9545 \
    -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' \
    | jq .finalized_l2.number)
  [ "$TX_BLOCK_DEC" -le "$FINAL" ] && break
  sleep 5
done
echo "  FINAL   ✓  (finalized_head=$FINAL)"
echo "Done."
```

---

## 17. net / web3 / eth Basics

```bash
# Chain ID
cast chain-id --rpc-url http://localhost:8123

# Network version
cast rpc --rpc-url http://localhost:8123 net_version

# Client version (reth build info)
cast rpc --rpc-url http://localhost:8123 web3_clientVersion

# Peer count
cast rpc --rpc-url http://localhost:8123 net_peerCount | xargs printf '%d\n'

# Is node listening?
cast rpc --rpc-url http://localhost:8123 net_listening
```

---

## Summary — What to Use for What

| Task | URL | Command / Section |
|------|-----|-------------------|
| Is L1 alive? | `:8545` | `cast block-number --rpc-url http://localhost:8545` |
| Is L2 alive? | `:8123` | `cast block-number --rpc-url http://localhost:8123` |
| Is kona/op-node alive? | `:9545` | `optimism_syncStatus` — §1 |
| Unsafe / safe / finalized heads | `:9545` | `optimism_syncStatus` — §1 |
| Watch heads live | `:9545` | `watch` one-liner — §1 |
| Rollup chain config | `:9545` | `optimism_rollupConfig` — §1 |
| Output root | `:9545` | `optimism_outputAtBlock` — §1 |
| L2 block info | `:8123` | `cast block` — §2 |
| L2 TX receipt | `:8123` | `cast receipt` — §2 |
| TX lifecycle (unsafe→safe→final) | `:9545` + `:8123` | Full script — §2 |
| Account balance (L2) | `:8123` | `cast balance --ether` — §3 |
| Account nonce | `:8123` | `cast nonce` — §3 |
| Is address a contract? | `:8123` | `cast code` — §3 |
| Send ETH on L2 | `:8123` | `cast send` — §4 |
| Call contract (read) | `:8123` | `cast call` — §4 |
| Call contract (write) | `:8123` | `cast send` — §4 |
| Estimate gas | `:8123` | `cast estimate` — §4 |
| Mempool size | `:8123` | `txpool_status` — §5 |
| Is TX pending? | `:8123` | `eth_getTransactionByHash` — §5 |
| Trace a TX | `:8123` | `debug_traceTransaction` — §6 |
| Event logs | `:8123` | `cast logs` / `eth_getLogs` — §6 |
| Storage proof (EIP-1186) | `:8123` | `eth_getProof` — §6 |
| L1 finalized block | `:8545` | `eth_getBlockByNumber finalized` — §7 |
| L1 safe block | `:8545` | `eth_getBlockByNumber safe` — §7 |
| L1 Beacon slot / finality | `:3500` | REST endpoints — §8 |
| Blob sidecars | `:3500` | `/eth/v1/beacon/blob_sidecars/:slot` — §8 |
| P2P peer list | `:9545` | `opp2p_peers` — §9 |
| Start / stop sequencer | `:9545` | `admin_startSequencer` / `admin_stopSequencer` — §9 |
| op-batcher miner check | `:8123` | `miner_setMaxDASize` — §10 |
| Conductor leader / HA status | `:8547` | `conductor_leader` — §11 |
| Prometheus metrics | `:9090` | `api/v1/query` — §12 |
| Pre-warming cache metrics | `:9090` | `reth_pre_warming_*` — §12 |
| Grafana dashboards | `:3000` | Browser — §13 |
| Verify RPC nodes in sync | `:8124` `:8128` | `cast block-number` compare — §14 |
| Full devnet health check | all | Bash script — §15 |
| Send TX + track all phases | `:8123` `:9545` | Bash script — §16 |
| Chain ID / client version | `:8123` | `cast chain-id` / `web3_clientVersion` — §17 |
