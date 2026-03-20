# kona-engine — Essential Dev Commands

All commands run from:
```
okx-optimism/rust/
```

---

## Check

```bash
# check kona-engine only (fast)
cargo check -p kona-engine

# check entire workspace
cargo check --workspace --all-features
```

---

## Format

```bash
# fix formatting
cargo +nightly fmt --all

# check formatting (CI mode, no changes)
cargo +nightly fmt --all -- --check
```

---

## Lint (clippy)

```bash
# kona-engine only
cargo clippy -p kona-engine --all-features --all-targets -- -D warnings

# entire workspace
cargo clippy --workspace --all-features --all-targets -- -D warnings
```

---

## Test

```bash
# kona-engine only (fast)
cargo test -p kona-engine

# kona-engine with nextest (faster output)
cargo nextest run -p kona-engine

# entire workspace (excludes online tests)
cargo nextest run --release --workspace --all-features -E '!test(test_online)'
```

---

## Build

```bash
# debug build — kona-engine
cargo build -p kona-engine

# release build — kona-engine
cargo build --release -p kona-engine

# release build — kona-node binary
cargo build --release --bin kona-node
```

---

## Clean

```bash
# clean build artifacts
cargo clean

# clean only kona-engine artifacts
cargo clean -p kona-engine
```

---

## All-in-one check before commit

```bash
cargo +nightly fmt --all -- --check \
  && cargo clippy -p kona-engine --all-features --all-targets -- -D warnings \
  && cargo test -p kona-engine \
  && cargo build --release -p kona-engine
```
