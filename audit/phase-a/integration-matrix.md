# EIP-8130 Port Audit — Integration Matrix (Phase C)

- **Generated**: 2026-05-05
- **Scope**: EVM factory + Precompile registry + 0x7b codec + txpool/RPC integration

## Summary

| Wiring point | OURS | BASE | Status |
|---|---|---|---|
| `extend_precompiles([(TX_CONTEXT_ADDRESS, …), (NONCE_MANAGER_ADDRESS, …)])` | **0 sites** | 1 site (`crates/common/evm/src/factory.rs:99`) | 🔴 **BUG-008 — missing** |
| `PrecompilesMap::from_static(...)` (initial map) | 2 sites (`alloy-op-evm/src/lib.rs:249, 273`) | 1 site (factory.rs:96) | OK structurally |
| `with_precompiles(...)` (apply to EVM) | 2 sites in alloy-op-evm + 2 in kona | 2 sites in factory.rs + 2 in fpvm-precompiles | OK — same call sites |
| `evm_with_env(...)` (single chokepoint for AA precompile inheritance) | 8 sites, all routed through `OpEvmFactory` | 6 sites, all routed through `OpEvmFactory` | OK — single fix point in alloy-op-evm/lib.rs unblocks ALL |
| `tx.ty() == AA_TX_TYPE_ID` (txpool branch) | `op-reth/crates/txpool/src/validator.rs:267` | `crates/txpool/src/validator.rs:260` | ✅ both have it |
| `is_base_v1_active_at_timestamp` / `is_native_aa_active_at_timestamp` (spec gate before AA validation) | (need to check) | `validator.rs:261` | 🟡 audit-pending — verify ours has equivalent gate |
| `AA_TX_TYPE_ID` codec encode/decode (0x7b ↔ `OpTxType::Eip8130`) | `op-alloy/crates/consensus/src/reth_codec.rs:54,72` | `crates/common/consensus/src/reth_compat.rs:264,284` | ✅ both have it |
| RLP encode/decode round-trip | `op-alloy/crates/consensus/src/transaction/eip8130/tx.rs` | `crates/common/consensus/src/transaction/eip8130/tx.rs` | ✅ structurally aligned |
| `AA_TX_TYPE_ID` rejection at non-AA hardforks | (need to check) | `txpool/src/validator.rs:262-265` (rejects with `TxTypeNotSupported` if not BASE_V1) | 🟡 audit-pending |
| `client/metering` AA branch | **no `client/metering` crate in OURS** | `crates/client/metering/src/transaction.rs` | 🟡 likely Coinbase-specific telemetry; assess relevance |
| Receipt encoding (`payer`, `phaseStatuses`) | `op-alloy/crates/consensus/src/receipts/{envelope,receipt}.rs` | `crates/common/consensus/src/receipts/{envelope,receipt}.rs` | ✅ paired files exist |

## Detailed: BUG-008 region

### What base has (`crates/common/evm/src/factory.rs:22-105`)

```rust
fn make_tx_context_precompile() -> DynPrecompile {
    DynPrecompile::new_stateful(PrecompileId::custom("tx_context"), |input| {
        // reads thread-local `EIP8130_TX_CONTEXT` via get_eip8130_tx_context()
        // dispatches on selector (getSender/getPayer/getOwnerId/getMaxCost/getGasLimit/getCalls)
    })
}

fn make_nonce_manager_precompile() -> DynPrecompile {
    DynPrecompile::new_stateful(PrecompileId::custom("nonce_manager"), |mut input| {
        // reads from input.internals.sload(NONCE_MANAGER_ADDRESS, slot)
        // returns 8-byte right-aligned u64 nonce
    })
}

fn op_precompiles_map(spec: OpSpecId) -> PrecompilesMap {
    let precompiles = BasePrecompiles::new_with_spec(spec);
    let mut map = PrecompilesMap::from_static(precompiles.precompiles());
    if spec == OpSpecId::BASE_V1 {
        map.extend_precompiles([
            (TX_CONTEXT_ADDRESS, make_tx_context_precompile()),
            (NONCE_MANAGER_ADDRESS, make_nonce_manager_precompile()),
        ]);
    }
    map
}
```

### What ours has (`alloy-op-evm/src/lib.rs:249-275`)

```rust
.with_precompiles(PrecompilesMap::from_static(
    OpPrecompiles::new_with_spec(spec_id).precompiles(),  // <-- only static set
))
```

**No `extend_precompiles([(TX_CONTEXT_ADDRESS, …), (NONCE_MANAGER_ADDRESS, …)])` call anywhere in OURS.**

### Why hidden previously

1. File path mismatch: base puts wiring in `common/evm/factory.rs`, ours embeds inline in `alloy-op-evm/src/lib.rs`. **File-by-file diff would not catch it** because there's no `factory.rs` in ours' alloy-op-evm to diff against.
2. `OpPrecompiles::new_with_spec(spec).precompiles()` LOOKS like it should produce a complete map — the `OpPrecompiles` wrapper struct has an `<OpPrecompiles as PrecompileProvider>::run` method that contains the AA dispatch (`precompiles.rs:493-516`), but that wrapper is bypassed by extracting just `.precompiles()` (the inner static `Precompiles` set).
3. Comment in `precompiles.rs:370-374` misleadingly states "dispatched via call interception at the AA execution layer in handler.rs" — but handler.rs uses the standard EVM execution path which uses the `PrecompilesMap`, never the OpPrecompiles wrapper. **Comment was wrong.**
4. NonceManager (`0x…aa02`) is used by tests as `T_REVERT` (relying on `0xfe` stub bytecode), so the symptom of the bug coincides with intended INVALID-opcode behavior — actively masking the issue.
5. TxContext (`0x…aa03`) has no callers in deployed Solidity (`AccountConfiguration` and `DefaultAccount` don't use it), so no production code path triggers the bug.

## Other audit-pending items found in this phase

### 🟡 No `client/metering/transaction.rs` equivalent in OURS
- BASE: `crates/client/metering/src/transaction.rs:28` checks `tx.ty() == AA_TX_TYPE_ID` for telemetry
- OURS: no equivalent file
- **Likely Coinbase-specific** — internal observability layer, not a protocol bug
- **Action**: confirm with chain-team; if we need similar metering, port it

### 🟡 Pre-fork rejection gate (`is_base_v1_active_at_timestamp`)
- BASE: `txpool/src/validator.rs:261-266` rejects AA txs as `TxTypeNotSupported` before fork activation
- OURS: line 267 enters the AA branch but **need to verify the activation gate is present**
- **Action**: read `op-reth/crates/txpool/src/validator.rs` lines 260-280 and confirm

### 🟢 Codec & RLP — confirmed equivalent
- Both forks have `AA_TX_TYPE_ID = 0x7B` mapped to `OpTxType::Eip8130` in encode/decode round-trip
- Both have `from_compact` / `to_compact` AA branches

### 🟢 EVM construction call-site count — confirmed single chokepoint
- All `evm_with_env` paths in OURS route through `OpEvmFactory::create_evm[_with_inspector]`
- Fixing `alloy-op-evm/src/lib.rs:249-275` (the BUG-008 site) propagates to all 8 callers

## Next steps for full audit

After this Phase C subset:

1. **Audit-pending** items above (3 items)
2. **Phase B — symbol-level cross-ref** for the consensus types layer (25 vs 25 files, structurally aligned but may have subtle method-level divergences)
3. **Phase D — behavioral sanity** for the wiring matrix (write 6-10 minimal e2e tests exercising each integration point)
4. **Phase E — write the audit skill** based on what we learned

## Methodology notes for the skill

What worked:
- Listing files via marker grep gave us the inventory cheaply
- Grouping by top-level directory exposed the structural divergences (BASE has 5 categories OURS doesn't have direct equivalents for)
- Scanning for `extend_precompiles` / `with_precompiles` / `register_*` calls bidirectionally surfaced the BUG-008 candidate

What didn't work:
- Plain `0x7b` matched lots of non-AA hex constants in test fixtures — needs anchoring (e.g., `AA_TX_TYPE_ID|0x7B` instead of bare `0x7b`)
- `account_changes` matched EIP-7928 Block Access Lists in `crates/common/access-lists/` — false-positive markers need a denylist
- Wide grep produced 40KB+ outputs — need staged filtering pipelines
- `rg -E` is `--encoding`, not extended regex — bash habit broke initial commands

What to encode in the skill:
- **Marker set** (positive markers + denylist for FlashBlocks/EIP-7928 false positives)
- **Top-level dir mapping** (OURS ↔ BASE structural alignment, with known-divergent categories flagged)
- **Wiring-pattern catalog** (`extend_precompiles`, `register_method`, `from_compact` codec maps, `is_*_active_at_timestamp` gates, `tx.ty() ==` branches, `evm_with_env` chokepoints)
- **Methodology**: bidirectional symbol ref, single-chokepoint identification, false-positive filtering
