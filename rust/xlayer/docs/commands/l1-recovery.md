# L1 Recovery After Machine Sleep

Run all commands from the project root: `cd /Users/.../working/xlayer`

---

## Decision Tree

### Short sleep (a few hours) — try normal restart first

```bash
./scripts/devnet/start-all.sh --no-build
```

If L2 blocks appear in logs → done.

---

### Long sleep (overnight / many hours) — check beacon health first

```bash
docker logs l1-beacon-chain 2>&1 | tail -5
```

**If you see "no active validator indices"** → beacon is dead. Do a full L1 wipe:

```bash
# 1. Stop everything
./scripts/devnet/stop-all.sh

# 2. Wipe L1 data
rm -rf docker/l1/execution/geth/
rm -rf docker/l1/consensus/beacondata/
rm -rf docker/l1/consensus/validatordata/

# 3. Start fresh L1
./scripts/devnet/internal/start-l1.sh

# 4. Redeploy OP contracts + regenerate genesis.json + rollup.json
./scripts/devnet/maintenance/redeploy-op-contracts.sh

# 5. Start node
./scripts/devnet/start-all.sh --no-build
```

`redeploy-op-contracts.sh` handles everything in step 4 — deploys contracts, generates
config files, patches genesis.l2.number, wipes stale L2 data. Do not run the sub-scripts
(reset-l1-reconfig.sh, reset-l2.sh) separately; they are called internally.

---

## When NOT to run `reset-l1-reconfig.sh` directly

`reset-l1-reconfig.sh` only patches `rollup.json` with the reth genesis hash. It does
not redeploy contracts or regenerate config. Running it alone after an L1 wipe will not
fix anything. It is only useful if you manually replaced `genesis.json` and need to sync
the hash into `rollup.json` without redeploying.

---

## Verify recovery succeeded

```bash
# L1 producing blocks?
cast bn --rpc-url http://localhost:8545

# L2 producing blocks?
cast bn --rpc-url http://localhost:8123

# Safe head advancing?
curl -sf -X POST http://localhost:9545 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"optimism_syncStatus","params":[]}' \
  | jq '{unsafe: .result.unsafe_l2.number, safe: .result.safe_l2.number}'
```

Safe head should advance within ~30s of the node starting (op-batcher submits backlog).

---

## See also

- `docs/concepts/l1-reset-internals.md` — why each step is needed and what breaks without it
- `scripts/devnet/maintenance/redeploy-op-contracts.sh` — the full redeployment script
