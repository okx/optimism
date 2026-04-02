---
name: run-acceptance-tests
description: Build dependencies and run acceptance tests locally in the Optimism monorepo. Handles contracts, cannon prestates, Rust binaries, and optional kona prestate.
---

# Run Acceptance Tests

Build all dependencies and run acceptance tests locally against the in-process devnet.

See `docs/ai/acceptance-tests.md` for full reference.

## When to Use

- Before merging changes that affect devnet behavior
- When CI acceptance tests fail and you need to reproduce locally
- When asked to verify a change works end-to-end

## Prerequisites

- The optimism monorepo checked out with submodules initialized
- mise activated in the shell (`eval "$(mise activate bash)"` if not already active) and `mise install` completed (provides `just`, `gotestsum`, `forge`, etc.)
- A working C toolchain (`clang` or `gcc`) for Rust builds
- For kona reproducible prestates: Docker must be available

## Steps

### 1. Run Tests

All `just` targets automatically build dependencies (contracts, cannon prestates, Rust binaries) before running. Always set `RUST_JIT_BUILD=1` so the test framework automatically builds any additional Rust binaries it needs (e.g. op-reth). Run from `op-acceptance-tests/`:

**Specific test:**
```bash
RUST_JIT_BUILD=1 cd op-acceptance-tests && just test -run TestMyTest ./op-acceptance-tests/tests/base/
```

**Specific package:**
```bash
RUST_JIT_BUILD=1 cd op-acceptance-tests && just test ./op-acceptance-tests/tests/base/...
```

**All tests:**
```bash
RUST_JIT_BUILD=1 cd op-acceptance-tests && just acceptance-test
```

**Gated subset** (e.g. `base`):
```bash
RUST_JIT_BUILD=1 cd op-acceptance-tests && just acceptance-test base
```

### 2. Kona Reproducible Prestate (if needed)

Only for superfaultproofs / interop fault proof tests. Not handled by `build-deps` or `RUST_JIT_BUILD`:
```bash
just reproducible-prestate-kona
```
**Requires Docker.** If Docker is not available, ask the user to run this command manually.

### 3. Interpret Results

When using `just acceptance-test`, structured output is available:
- Logs in `op-acceptance-tests/logs/testrun-<timestamp>/`
- JUnit XML in `op-acceptance-tests/results/results.xml`
- `flaky-tests.txt` in the log directory lists tests marked with `MarkFlaky()`

When using `just test`, output goes to stdout only.

## Tuning (`acceptance-test` only)

When using `just acceptance-test`, override parallelism with environment variables:
- `ACCEPTANCE_TEST_PARALLEL=2` — limit per-package test parallelism
- `ACCEPTANCE_TEST_JOBS=4` — limit concurrent packages
- `ACCEPTANCE_TEST_TIMEOUT=1h` — per-package timeout
- `LOG_LEVEL=debug` — increase log verbosity

## Troubleshooting

| Symptom | Fix |
|---|---|
| Missing `prestate-mt64.bin.gz` | `cd op-acceptance-tests && just build-deps` |
| ABI mismatch / contract errors | `cd op-acceptance-tests && just build-deps` |
| `kona-node` / `op-rbuilder` / `rollup-boost` not found | `cd op-acceptance-tests && just build-deps` |
| `gotestsum` not found | `mise install` |
| `op-reth` binary not available | Ensure `RUST_JIT_BUILD=1` is set |
| Kona prestate hash mismatch | `just reproducible-prestate-kona` (requires Docker) |
