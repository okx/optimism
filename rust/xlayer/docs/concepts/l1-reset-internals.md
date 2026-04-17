# L1 Reset Internals — Why Each Step Is Needed

This document explains what breaks when L1 state is lost, why the recovery steps are ordered
the way they are, and what each script does internally.

See `docs/commands/l1-recovery.md` for the quick runbook.

---

## What Happens When the Mac Sleeps

The L1 devnet runs inside Docker. The beacon chain (Prysm) maintains an in-memory validator
registry. When the host sleeps long enough (typically overnight), Docker containers freeze and
resume in a state that Prysm cannot recover from:

```
level=warning msg="No active validator indices, skipping slot..."
```

At this point the beacon chain stops producing attestations. Without attestations, Geth's block
production stalls. L1 blocks stop. kona-node's derivation pipeline stalls. L2 blocks stop.

The only fix is to wipe all L1 data and start fresh:

```
docker/l1/execution/geth/          ← Geth chaindata
docker/l1/consensus/beacondata/    ← Prysm beacon state
docker/l1/consensus/validatordata/ ← Prysm validator keys/state
```

---

## Why Wiping L1 Breaks L2

The OP Stack genesis contract addresses are deterministic (CREATE2 based on deployer key +
nonce). After an L1 wipe:

1. **L1 resets to block 0.** All previously deployed contracts are gone.
2. **Redeploying contracts lands at the same addresses** (same deployer key, same nonce sequence).
3. **But the L1 block reference in `rollup.json` changes.** `genesis.l1` contains the L1
   block hash and number at the time of deployment. On each fresh L1, block 1 has a new hash.
4. **`genesis.l2.hash` in `rollup.json` must also change.** reth computes the L2 genesis hash
   differently from op-geth because XLayer's genesis block is at number 8593921 (not 0).
   The hash includes the block number in the header, so it differs from what op-deployer put
   in rollup.json.

If you skip the redeploy step and just restart, kona sees a `genesis.l2.hash` that doesn't
match any block in reth's database → crash.

---

## The Three Config Values That Must All Be In Sync

```
config/devnet/genesis.json     ← reth's chain config + state allocations
config/devnet/rollup.json      ← kona's chain config (references genesis.l2.hash)
config/devnet/l1-genesis.json  ← L1 genesis (stable, only changes if L1 chain params change)
```

After every L1 wipe + redeploy:

| Value | Where | Why it changes |
|---|---|---|
| `genesis.l1.hash` | rollup.json | L1 block 1 has a new hash on fresh chain |
| `genesis.l1.number` | rollup.json | Usually stays 1 |
| `genesis.l2.hash` | rollup.json | reth recomputes genesis hash including block number |
| `genesis.l2.number` | rollup.json | op-deployer always writes 0; must patch to 8593921 |
| Contract addresses in alloc | genesis.json | Redeployed from scratch |

---

## What `redeploy-op-contracts.sh` Does (Step by Step)

### Step 1 — Deploy Transactor contract
An `owner Transactor` contract (a simple multisig-like proxy used as the L1 proxy admin owner
on XLayer). Must be deployed before any OP contracts because its address is passed as
`l1ProxyAdminOwner` in `intent.toml`.

**Bug encountered**: `forge create --json 2>&1` prepends a deprecation warning line before the
JSON blob. Naive `jq` parse exits with code 5. Fix: `grep -v '^Warning' | jq ...`.

### Steps 2–3 — Bootstrap superchain + implementations
`op-deployer bootstrap` deploys the shared superchain contracts (SuperchainConfig,
ProtocolVersions) and then all OP implementation contracts (OptimismPortal, L1CrossDomainMessenger,
etc.). These are the contracts that don't change per-chain; they can be reused across
multiple OP chains on the same L1.

### Step 4 — Update intent.toml
`op-deployer apply` reads `intent.toml` for chain-specific parameters. The file is copied from
`intent.toml.bak` (the template checked into the repo) and patched with addresses from the
just-deployed superchain/implementations.

### Step 5 — Deploy OP chain + generate genesis.json + rollup.json
`op-deployer apply` deploys the per-chain contracts (L2OutputOracle, DisputeGameFactory, etc.)
and then `op-deployer inspect genesis` / `inspect rollup` generate the config files.

At this point `rollup.json` has:
- `genesis.l1` = correct new L1 block reference
- `genesis.l2.hash` = op-geth hash (wrong for reth)
- `genesis.l2.number` = 0 (wrong; XLayer genesis is at 8593921)

### Step 6 — Patch genesis.json for XLayer
op-deployer generates a standard OP Stack genesis (number=0). XLayer's genesis is at the
mainnet fork block + 1 (8593921). The patching step:

- Sets `config.legacyXLayerBlock = 8593921`
- Sets `parentHash` to the XLayer pre-genesis parent hash (constant: `0x6912fea5...`)
- Sets `number = 0x830001` (8593921 in hex)
- Creates `genesis-reth.json` (identical; reth accepts the same format)

### Step 7 — Copy + patch genesis.l2.number in rollup.json
Copies `genesis-reth.json → config/devnet/genesis.json` and `rollup.json → config/devnet/rollup.json`.

Then immediately patches `genesis.l2.number` from 0 to 8593921:

```bash
jq ".genesis.l2.number = 8593921" config/devnet/rollup.json > /tmp/patch.json
mv /tmp/patch.json config/devnet/rollup.json
```

**Why this matters**: kona's `L2ForkchoiceState::current` (in `sync/forkchoice.rs`) falls back
to `eth_getBlockByNumber(genesis.l2.number)` on a fresh DB. If that's 0, it queries for block 0
which doesn't exist → `Block not found: 0x0` → crash.

### Step 8 — Patch rollup.json genesis.l2.hash (reset-l1-reconfig.sh)
reth computes the genesis block hash including the `number` field. op-geth's genesis hash uses
number=0. Because reth uses number=8593921, the hash differs.

`reset-l1-reconfig.sh` runs `xlayer-node init --chain genesis.json --datadir <tmpdir>`, captures
the "Genesis block written" log line, extracts the hash, and patches `rollup.json`.

**Bug encountered**: reth's structured logger wraps field names in ANSI escape codes:
```
[32mINFO[0m Genesis block written [3mhash[0m[2m=[0m0x1972c5ca...
```
Grep pattern `hash=0x[0-9a-f]{64}` never matches. Fix: match the log message text first,
then extract the bare hex: `grep "Genesis block written" | grep -oE '0x[0-9a-f]{64}'`.

### Step 9 — Reset stale L2 data (reset-l2.sh)
The reth datadir holds blocks derived from the old genesis. Since the genesis hash changed,
all that data is invalid. `reset-l2.sh` wipes the datadir so reth starts from scratch.

---

## Why xlayer-toolkit Is No Longer Needed

Originally this devnet was driven by `xlayer-toolkit`, a separate repo that contained
docker-compose and deployment scripts. After decoupling:

- All deployment scripts (`redeploy-op-contracts.sh`, `reset-l1-reconfig.sh`, `reset-l2.sh`)
  live in `scripts/devnet/maintenance/` in this repo.
- `intent.toml.bak` and `state.json.bak` (op-deployer input templates) are committed to
  `docker/op-contracts/` in this repo.
- xlayer-node is built and run from this repo directly (`./scripts/devnet/start-all.sh`).
- L1 docker services (Geth + Prysm) are still Docker containers but started by
  `scripts/devnet/internal/start-l1.sh`.

The only external dependency is the Docker images: `op-contracts:latest` and `op-stack:latest`.

---

## Lessons Learned

| Lesson | Detail |
|---|---|
| forge `--json 2>&1` prepends warnings | Filter with `grep -v '^Warning'` before piping to jq |
| reth structured logs have ANSI codes | Never grep `field=value`; grep the message text + extract hex separately |
| op-deployer always writes `genesis.l2.number=0` | Patch to actual fork block after every `op-deployer inspect rollup` |
| kona uses `genesis.l2.number` to bootstrap fresh DB | Wrong value → wrong block query → crash with `Block not found` |
| Prysm validator state does not survive long container freeze | Don't try to restart; wipe and redeploy |
| reth genesis hash ≠ op-geth genesis hash for non-zero genesis | Because block header includes `number`; must use `xlayer-node init` to compute the real hash |
