# Proposal: Adventure-Based Stress Testing for xlayer-node

**Status**: Draft
**Date**: 2026-03-24
**Context**: xlayer-node has been decoupled from xlayer-toolkit so it runs autonomously. We want to bring the Adventure load-testing tool into xlayer-node so stress tests can be run without depending on xlayer-toolkit being present on disk.

---

## Background

### What is Adventure?

Adventure is a Go CLI tool built by the XLayer team for L2 stress testing. It drives **20,000 concurrent accounts** sending ERC20 or native token transfers against an RPC endpoint and reports real-time TPS.

It lives today at `xlayer-toolkit/tools/adventure/` and is used to benchmark xlayer-node during development.

### Current problem

xlayer-node and xlayer-toolkit have been decoupled — xlayer-node can be cloned, built, and run independently. However, running Adventure stress tests still requires xlayer-toolkit to be present on disk, which breaks the autonomy goal.

### What Adventure actually needs

Adventure has **no infrastructure dependencies beyond an RPC URL**. A complete breakdown:

| Dependency | Where it lives | Status in xlayer-node devnet |
|---|---|---|
| Adventure binary / Go source | xlayer-toolkit | ❌ Not present |
| `accounts-20k.txt` (20k private keys) | xlayer-toolkit | ❌ Not present |
| `config.json` (RPC URL, concurrency, etc.) | xlayer-toolkit | ❌ Not present |
| Funded `senderPrivateKey` | Hardhat account #1 (`0x59c6995…`) | ✅ Pre-funded — xlayer-node genesis allocates 10,000 ETH to all Hardhat accounts |
| L2 RPC at `:8123` | xlayer-node runs here | ✅ Already correct |

**Key insight**: Adventure's init phase funds the 20,000 test accounts from the one sender key — they do not need to be pre-loaded in genesis. As long as the sender key has enough ETH (100 ETH), the rest follows automatically.

---

## How Adventure works (two-phase execution)

```
Phase 1 — Init
  adventure erc20-init 100ETH -f config.json
  └─ senderPrivateKey (Hardhat #1, 10,000 ETH in genesis)
      ├─ Deploys ERC20 contract          (~10M gas)
      ├─ Deploys BatchTransfer contract  (~400K gas)
      └─ Distributes tokens to 20,000 accounts in batches of 50
         (this funds all test accounts — no genesis change needed)

Phase 2 — Bench
  adventure erc20-bench -f config.json --contract 0x<deployed>
  └─ All 20,000 accounts send concurrently
      ├─ Each picks a random recipient
      ├─ Optional EIP-2930 access lists (useAccessList: true)
      └─ Real-time TPS logged to adventure-tps.log
```

---

## Proposed Approach — Vendor Go source, build on first run (Recommended)

Copy the Adventure Go source (small — ~10 files, ~50 KB) into the xlayer-node repo. Gitignore the compiled binary. A wrapper script builds it automatically on first run.

### Resulting layout

```
xlayer-node/
├── tools/
│   └── adventure/
│       ├── main.go
│       ├── go.mod
│       ├── go.sum
│       ├── bench/
│       │   ├── erc20.go
│       │   ├── native.go
│       │   └── constants.go
│       ├── utils/
│       │   ├── config.go
│       │   ├── eth_client.go
│       │   ├── tx.go
│       │   ├── tps.go
│       │   └── account.go
│       └── testdata/
│           ├── config.json          ← RPC: :8123, senderKey: Hardhat #1
│           └── accounts-20k.txt     ← 20,000 test account private keys
│
└── scripts/devnet/
    └── stress-test.sh               ← new: build if needed, run init+bench, report TPS
```

`tools/adventure/adventure` (the compiled binary) is added to `.gitignore`.

### Implementation steps

**Step 1 — Copy source from xlayer-toolkit**

```bash
cp -r ../xlayer-toolkit/tools/adventure/main.go     tools/adventure/
cp -r ../xlayer-toolkit/tools/adventure/bench/      tools/adventure/
cp -r ../xlayer-toolkit/tools/adventure/utils/      tools/adventure/
cp    ../xlayer-toolkit/tools/adventure/go.mod      tools/adventure/
cp    ../xlayer-toolkit/tools/adventure/go.sum      tools/adventure/
cp -r ../xlayer-toolkit/tools/adventure/testdata/   tools/adventure/
```

**Step 2 — Update `.gitignore`**

Add to the root `.gitignore`:
```
tools/adventure/adventure
```

**Step 3 — Verify `config.json` already targets xlayer-node**

`testdata/config.json` already has `"rpc": ["http://127.0.0.1:8123"]` and uses Hardhat account #1 as sender — no changes needed for the default devnet.

**Step 4 — Write `scripts/devnet/stress-test.sh`**

The script should:
1. Source `lib.sh` (loads `.env`, `$L2_RPC_URL`)
2. Check Go is installed
3. Build the adventure binary if `tools/adventure/adventure` is missing
4. Patch `config.json`'s `rpc` field with `$L2_RPC_URL` at runtime (so `.env` overrides work)
5. Run `erc20-init`, capture contract address from stdout
6. Sleep 10 seconds for token distribution to settle
7. Run `erc20-bench` with the captured contract address
8. Print a TPS summary from `adventure-tps.log`

Flags to support:
| Flag | Default | Effect |
|---|---|---|
| `--mode erc20\|native` | `erc20` | Which benchmark to run |
| `--concurrency N` | `50` | Concurrent senders |
| `--init-amount XETH` | `100ETH` | ETH distributed to test accounts |
| `--duration SEC` | unlimited | Stop bench after N seconds |

**Step 5 — Add to scripts/devnet/README.md**

Document `stress-test.sh` in the same format as `perf-baseline.sh`.

### Why this approach is recommended

| Property | This approach |
|---|---|
| Requires xlayer-toolkit on disk | ❌ No |
| Commits a binary to git | ❌ No |
| Cross-platform (macOS + Linux) | ✅ Yes — builds for the current platform |
| One-time setup | ✅ `go build` runs once, binary cached locally |
| Go required on developer machine | ✅ Yes (Go 1.16+) |
| Source stays in sync with xlayer-node | ✅ Yes — vendored copy, PRs update it |

---

## Alternative Proposals

### Alternative A — Reference the pre-built binary from sibling repo

Point `stress-test.sh` at `../xlayer-toolkit/tools/adventure/adventure` directly, without copying anything.

```bash
ADVENTURE_BIN="../xlayer-toolkit/tools/adventure/adventure"
```

**Pros**: Zero files to add to xlayer-node.
**Cons**: Breaks if xlayer-toolkit is not present on disk or moves. Defeats the autonomy goal. The existing binary is arm64 macOS only — fails on Linux/amd64 CI or server environments.

**Verdict**: Acceptable for local development shortcuts only. Not suitable as the permanent solution.

---

### Alternative B — Commit the pre-built binary

Copy `xlayer-toolkit/tools/adventure/adventure` into `tools/adventure/` and commit it.

**Pros**: No Go required to use it. Works immediately after clone.
**Cons**:
- 14 MB binary in git history, forever.
- **arm64 macOS only** — the existing binary (`Mach-O 64-bit executable arm64`) will not run on any Linux server or x86 machine.
- Every Adventure update requires committing a new binary.
- Git LFS would be needed to keep repo size manageable.

**Verdict**: Not recommended. The platform lock-in alone makes this unsuitable.

---

### Alternative C — Lightweight bash-only stress test (no Adventure)

Write a pure-bash stress test using `cast send` in parallel (similar to `perf-baseline.sh`) without Adventure at all.

```bash
# send N transactions in parallel using cast
for i in $(seq 1 $COUNT); do
    cast send --rpc-url "$L2_RPC_URL" --private-key "$KEY" "$RECIPIENT" \
        --value 0.001ether --async &
done
wait
```

**Pros**: Zero new dependencies. Works with just Foundry (`cast`), which is already required.
**Cons**:
- Maximum concurrency limited to shell subprocesses — not comparable to Adventure's 20,000-account model.
- No ERC20 contract load (which is the realistic workload pattern).
- No EIP-2930 access list testing.
- Adventure's HTTP connection pooling and nonce management cannot be replicated in bash.

**Verdict**: Useful as a lightweight smoke test (already exists as `perf-baseline.sh`). Not a replacement for Adventure-level stress testing.

---

### Alternative D — Go submodule pointing to xlayer-toolkit

Add `xlayer-toolkit` as a Git submodule scoped to `tools/adventure/`.

**Pros**: Source stays in sync with upstream automatically.
**Cons**: Submodules add operational complexity (extra `git submodule update` step after clone). Pins to a specific commit of xlayer-toolkit. Adds the entire toolkit as a dependency even though only one tool is needed.

**Verdict**: Overengineered for this use case. Vendoring the relevant subdirectory is simpler and more maintainable.

---

## Comparison Summary

| | Recommended | Alt A (sibling ref) | Alt B (commit binary) | Alt C (bash only) | Alt D (submodule) |
|---|---|---|---|---|---|
| Autonomous (no xlayer-toolkit) | ✅ | ❌ | ✅ | ✅ | ❌ |
| Cross-platform | ✅ | ❌ arm64 only | ❌ arm64 only | ✅ | ✅ |
| No binary in git | ✅ | ✅ | ❌ 14MB | ✅ | ✅ |
| No Go required | ❌ | ✅ | ✅ | ✅ | ❌ |
| Full 20k-account load | ✅ | ✅ | ✅ | ❌ | ✅ |
| EIP-2930 access list test | ✅ | ✅ | ✅ | ❌ | ✅ |
| Complexity | Low | Very low | Very low | Very low | High |

---

## Open Questions

1. **Should `accounts-20k.txt` be committed?** It is 20,000 lines of private keys (~1 MB). These are devnet-only keys with no mainnet value, so committing them is safe. The file is static and never changes.

2. **Should we support `--mode native` in the initial implementation?** Native token bench is simpler (no contract deployment) and good for a quick smoke test. ERC20 is the realistic production workload. Recommended: support both from the start.

3. **Does xlayer-node CI need to run stress tests?** Full Adventure bench (20k accounts, unlimited duration) is not suitable for CI. A short-duration smoke test (`--duration 30 --concurrency 10`) could be added as an optional CI step against a locally-started devnet.
