# EIP-8130 Port Audit — File Inventory

- **Generated**: 2026-05-05
- **OURS**: `https://github.com/okx/optimism/tree/cf38ca5666/rust` + `https://github.com/okx/optimism/tree/cf38ca5666/op-node` (Go)
- **BASE**: `https://github.com/base/base/tree/a33ab4d09/crates`
- **Markers**: `NATIVE_AA|BASE_V1|EIP8130|NativeAA|TX_CONTEXT|NONCE_MANAGER|0x.*aa02|0x.*aa03|owner_config|account_changes|AccountChange|EIP-8130|eip8130|eip_8130`

## Top-level directory mapping

| Category | OURS path | BASE path | OURS files | BASE files | Notes |
|---|---|---|---:|---:|---|
| Consensus types (tx envelope / accessors / signature / storage / abi) | `op-alloy/crates/consensus/src/transaction/eip8130/` + receipts | `crates/common/consensus/src/transaction/eip8130/` + receipts | 25 | 25 | **structurally aligned** — file names match 1:1 |
| EVM factory / wiring (BUG-008 region) | `alloy-op-evm/src/` | `crates/common/evm/src/` | 7 | 5 | **file structures differ** — ours embeds factory in `lib.rs`, base splits to `factory.rs`/`evm.rs` |
| revm extensions | `op-revm/src/` | `crates/execution/revm/` | 10 | 10 | structurally similar |
| Hardforks | `alloy-op-hardforks/src/` | `crates/consensus/upgrades/` | 1 | 2 | base has more — investigate |
| TxPool | `op-reth/crates/txpool/src/` | `crates/txpool/src/` | 18 | 10 | both have eip8130_validate / eip8130_invalidation |
| RPC types | `op-alloy/crates/rpc-types/`, `rpc-types-engine/` | `crates/common/rpc-types/` | (mixed) | 4 | base has consolidated rpc-types |
| RPC handlers | (in op-reth) | `crates/execution/rpc/`, `client/` | (mixed) | (mixed) | needs separate file-level audit |
| Genesis bytecode (GO) | `op-node/rollup/derive/native_aa_*` | (no Go in base) | 7 (.go + .hex) | 0 | **base inherits op-node from optimism** |
| Block / payload builder | `alloy-op-evm/src/block/` | `crates/execution/payload/`, `flashblocks/`, `builder/` | 2 | 5+ | base has FlashBlocks-specific paths |
| Engine / consensus | n/a | `crates/consensus/protocol/`, `derive/`, `genesis/` | 0 | 10 | base monorepo includes derive logic; ours uses op-node Go |
| Access-lists (FlashBlocks BAL — false positive) | n/a | `crates/common/access-lists/` | 0 | 8 | **NOT AA** — EIP-7928 Block Access Lists, FlashBlocks-only feature; marker over-matched |
| Misc / Cargo.toml | scattered | scattered | 4 | 5 | non-code |

**Total**: OURS = 77 files (including some kona/proof testdata), BASE = 96 files.

## Categories where layouts diverge significantly (audit risk)

### 🔴 EVM factory / wiring — BUG-008 confirmed here
- BASE has `crates/common/evm/src/factory.rs` (155 lines) with explicit `op_precompiles_map(spec)` + `make_tx_context_precompile()` + `make_nonce_manager_precompile()` (lines 22-90).
- OURS has the equivalent logic split: address constants live in `op-revm/src/precompiles.rs`, but the **wiring into `PrecompilesMap` is omitted** in `alloy-op-evm/src/lib.rs:249-251`.
- **Status**: BUG-008 already filed.

### 🟡 Block / payload — FlashBlocks-specific
- BASE has 4+ files in `crates/execution/flashblocks/` and `crates/execution/payload/` that touch EIP-8130.
- OURS only has `alloy-op-evm/src/block/native_aa.rs` and `block/mod.rs`.
- **Risk**: if FlashBlocks integration is part of our chain (xlayer-reth uses it), block-builder paths may have AA-handling gaps. **Confirm with our reth fork**, not optimism Rust workspace.
- **Action**: separate audit pass against `xlayer-reth` repo (out of optimism scope).

### 🟡 Hardforks — base has 2 files, ours has 1
- BASE: `crates/consensus/upgrades/src/{base_consensus_upgrades,native_aa_upgrade}.rs` (or similar split).
- OURS: `alloy-op-hardforks/src/lib.rs` only.
- **Risk**: base may have additional hardfork-gating logic we missed. **Need symbol-level diff**.

### 🟡 TxPool — ours has 18, base has 10
- OURS has more files. Some may be from xlayer-specific extensions. Need to check **all 10 base files have direct equivalents**.
- BUG-002 / BUG-005 are already in this region; possible other liveness bugs.

### 🟢 Consensus types — perfectly aligned
- 25 vs 25, file names match 1:1. **Symbol-level diff would be high-confidence here**.
- BUG-001 / BUG-003 / BUG-004 / BUG-005 / BUG-006 already covered, but symbol-level pass might find another wiring miss.

## False-positive markers

- `account_changes` matches base's EIP-7928 (FlashBlocks BAL) `AccountChanges` struct in `common/access-lists/`. **Not AA**.
- Some `0x7b` matches non-AA hex constants in test data.

These need an explicit denylist in the audit skill: when a file mentions ONLY `account_changes` (lowercase) but no other AA marker, classify as "potentially false positive — verify".
