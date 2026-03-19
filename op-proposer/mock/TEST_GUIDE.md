# TEE Game Type (1960) — Hands-On Testing Guide

This guide walks you through testing the op-proposer TEE game type (1960) locally using the mock TeeRollup server. All parameters are provided manually—no `.env` file dependency.

## Overview

Testing the TEE game type requires three components working together:

1. **Mock TeeRollup RPC server** (`mockteerpc`) — Simulates the TeeRollup HTTP endpoint, returning block info with incrementing heights
2. **op-proposer** — The proposer binary that reads from TeeRollup and submits TEE game disputes
3. **Verification script** (`list_games.sh`) — Lists created games to verify proposal success

The flow: proposer fetches block info from mock TeeRollup → computes rootClaim → submits proposal → list_games.sh confirms the game was created.

---

## Step 1: Install Binaries
0x1D8D70AD07C8E7E442AD78E4AC0A16f958Eba7F0
On macOS, binaries must be installed via `go install` (not run directly from build output) due to security restrictions.

From the repository root, install both the mock TeeRollup server and the proposer:

```bash
cd op-proposer
go install ./mock/cmd/mockteerpc
go install ./cmd/main.go
```

Both binaries are now in `$GOPATH/bin` and available system-wide.

---

## Step 2: Start the Mock TeeRollup RPC Server

In a new terminal, start the mock server with the flags below:

```bash
mockteerpc \
  --addr=:8090 \
  --init-height=1000000 \
  --error-rate=0 \
  --delay=0
```

### Flag Reference

| Flag | Description |
|------|-------------|
| `--addr` | Listen address (default `:8090`). Use any available port. |
| `--init-height` | Starting block height (default `1000`). **Use a value higher than your anchor sequence number.** The mock increments this by 1–50 every second. |
| `--error-rate` | Fraction of requests that return errors, 0.0–1.0 (default `0`). Use `0` for testing. |
| `--delay` | Maximum random response delay in milliseconds (default `0`). Use `0` for fastest testing. |

### Verify the Server is Running

In another terminal, check that the endpoint returns valid data:

```bash
curl http://localhost:8090/v1/chain/confirmed_block_info
```

Expected output:

```json
{
  "code": 0,
  "message": "OK",
  "data": {
    "height": 1000000,
    "appHash": "0x3a7bd3e2360a3d29eea436fcfb7e44c735d117c42d1c1835420b6b9942dd4f1b",
    "blockHash": "0x1234abcd5678ef901234567890abcdef1234567890abcdef1234567890abcdef"
  }
}
```

If you get `"data": null`, the server initialization failed. Restart it.

---

## Step 3: Start the Proposer

In a third terminal, start the proposer with realistic placeholder addresses and keys:

```bash
main \
  --l1-eth-rpc="http://localhost:8545" \
  --tee-rollup-rpc="http://localhost:8090" \
  --game-type=1960 \
  --game-factory-address="0xCe4cD48CD7802a2dD6b026043bc2eE831c555d77" \
  --private-key="0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d" \
  --poll-interval=12s \
  --proposal-interval=12s \
  --rpc.port=7302 \
  --log.level=info
```

### Flag Reference

| Flag | Description |
|------|-------------|
| `--l1-eth-rpc` | L1 Ethereum RPC endpoint. Must be running and synced. |
| `--tee-rollup-rpc` | TEE Rollup RPC endpoint (mock or real). Required for game type 1960. |
| `--game-type` | Dispute game type. Must be `1960` for TEE games. |
| `--game-factory-address` | DisputeGameFactory contract address on L1. Deploy or get from devnet. |
| `--private-key` | Proposer wallet private key (without `0x` prefix if only hex). **Must have sufficient ETH for gas.** |
| `--poll-interval` | How often to check L1 state (default `12s`). Shorter = faster proposals. |
| `--proposal-interval` | Minimum time between proposals (default `12s`). Space out proposals to avoid nonce conflicts. |
| `--rpc.port` | Port for proposer's own RPC server (default `8544`). Use `7302` to avoid conflicts. |
| `--log.level` | Log verbosity: `debug`, `info`, `warn`, `error`. Use `info` for normal testing. |

**Note:** `--tee-rollup-rpc` is mutually exclusive with `--rollup-rpc`, `--supervisor-rpcs`, and other super-node RPCs.

### Expected Startup Logs

Within the first few seconds:

```
msg="Connected to DisputeGameFactory" address=0xCe4cD48CD7802a2dD6b026043bc2eE831c555d77
msg="Started RPC server" endpoint=http://[::]:7302
msg="Proposer started"
```

Within ~12 seconds (one proposal interval):

```
msg="No proposals found for at least proposal interval, submitting proposal now"
msg="Proposing output root" sequenceNum=1000012 extraData="0x..."
msg="Transaction confirmed" tx=0xabcd...
```

If the proposer waits longer, check that:
- Mock TeeRollup is running and reachable at `--tee-rollup-rpc`
- L1 RPC is running at `--l1-eth-rpc`
- The private key's address has ETH for gas

---

## Step 4: Verify Games with list_games.sh

In a fourth terminal, use the list_games.sh script to confirm that games were created:

```bash
cd /Users/jimmyshi/code/optimism/op-proposer/mock
./list_games.sh \
  --rpc http://localhost:8545 \
  --factory 0xCe4cD48CD7802a2dD6b026043bc2eE831c555d77 \
  --count 5
```

### Flag Reference

| Flag | Description |
|------|-------------|
| `--rpc` | L1 RPC endpoint (same as proposer's `--l1-eth-rpc`). |
| `--factory` | DisputeGameFactory address (same as proposer's `--game-factory-address`). |
| `--count` | Number of latest games to list (default `10`). |

### Expected Output

```
Game 0:
  Type: 1960
  Timestamp: 1710892800
  Proxy: 0xa5875EdD032eFbe7773084ae23C588daC243bc01

Game 1:
  Type: 1960
  Timestamp: 1710892812
  Proxy: 0xb7c96ee3f2c1d4f8a9e0b2c3d4e5f6a7b8c9d0e
```

Key observations:
- **Type must be `1960`** (if type is not 1960, the game was not created as a TEE game)
- **Timestamps increase** (each new game is ~12 seconds apart, matching proposal-interval)
- **Proxy addresses differ** (each game gets a unique proxy contract)

---

## Step 5: Verify rootClaim (Optional)

To verify that the rootClaim was computed correctly, fetch the game's claim data using cast:

```bash
# Use the proxy address from list_games.sh output
GAME_ADDR=0xa5875EdD032eFbe7773084ae23C588daC243bc01
L1_RPC="http://localhost:8545"

# Read the claim fields
cast call $GAME_ADDR "rootClaim()(bytes32)" --rpc-url $L1_RPC
cast call $GAME_ADDR "blockHash()(bytes32)" --rpc-url $L1_RPC
cast call $GAME_ADDR "stateHash()(bytes32)" --rpc-url $L1_RPC
```

Example output:

```
blockHash:   0x1234abcd5678ef901234567890abcdef1234567890abcdef1234567890abcdef
stateHash:   0x9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba
rootClaim:   0xabcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789
```

### Verify the Formula

The rootClaim must equal `keccak256(blockHash || stateHash)`:

```bash
BLOCK_HASH="0x1234abcd5678ef901234567890abcdef1234567890abcdef1234567890abcdef"
STATE_HASH="0x9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba"

# Compute expected rootClaim
EXPECTED=$(cast keccak $(cast concat-hex $BLOCK_HASH $STATE_HASH))
echo "Expected rootClaim: $EXPECTED"

# Compare with actual
ACTUAL=$(cast call $GAME_ADDR "rootClaim()(bytes32)" --rpc-url $L1_RPC)
echo "Actual rootClaim:   $ACTUAL"
```

Both values must match.

---

## Troubleshooting

| Error | Cause | Fix |
|-------|-------|-----|
| `tee-rollup: no confirmed block available (data is null)` | Mock server not running or endpoint unreachable | Check `mockteerpc` is running; verify `--tee-rollup-rpc` URL is correct |
| `tee-rollup-rpc is required for TeeRollup game type (1960)` | Missing `--tee-rollup-rpc` flag | Add `--tee-rollup-rpc=http://localhost:8090` |
| `l2SequenceNumber() <= anchorSeqNum` | Mock init-height is below anchor state sequence number | Restart `mockteerpc` with higher `--init-height` (e.g., `1000000`) |
| `nonce too low` | Multiple proposer instances running concurrently | Stop all proposer processes; restart only one instance |
| `failed to bind to address "0.0.0.0:7302"` | RPC port already in use | Use a different `--rpc.port` (e.g., `7303`, `7304`) |
| `connection refused` (L1 RPC) | L1 node not running or wrong address | Ensure L1 node is running at `--l1-eth-rpc` |
| `insufficient funds` | Proposer wallet has no ETH | Fund the wallet address derived from `--private-key` |
| `game type 1960 not registered` | DisputeGameFactory not properly configured | Verify game type 1960 is registered in the factory; deploy TestGame contract |

---

## Tips for Local Testing

- **Speed up iteration:** Use lower values for `--poll-interval` and `--proposal-interval` (e.g., `5s`) to submit games faster.
- **Simulate network delay:** Set `--delay=500` on the mock server to test timeout handling.
- **Simulate errors:** Set `--error-rate=0.1` to randomly fail 10% of mock requests and test retry logic.
- **Watch continuously:** Use `watch` to monitor games in real-time:
  ```bash
  watch -n 2 './list_games.sh --rpc http://localhost:8545 --factory 0xCe4cD48CD7802a2dD6b026043bc2eE831c555d77 --count 3'
  ```
- **Inspect logs:** Set `--log.level=debug` on the proposer to see detailed execution flow (verbose output).

---

## Next Steps

Once basic testing works:

1. Run `op-proposer` unit tests: `go test ./proposer/... -v`
2. Test with a real TeeRollup (not mock) by pointing `--tee-rollup-rpc` to the real endpoint
3. Run end-to-end tests with `op-challenger` and dispute resolution
