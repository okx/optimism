//! XLayer-specific [`GasParams`] extension for EIP-8130 AA.
//!
//! Builds on revm's [`GasParams`] (a 256-slot fork-aware gas table living on
//! `Cfg`) by reserving a small range of chain-specific slots for EIP-8130
//! native verifier costs and the AA nonce-SSTORE intrinsic prices, then
//! injecting them via [`GasParams::override_gas`] when `XLayer` forks activate.
//!
//! Design parallels tempo's `tempo_gas_params(spec) -> GasParams`:
//!
//! - **Inherited EVM gas** (cold/warm SLOAD, `SSTORE_SET/RESET`, calldata token cost, tx base
//!   stipend, EIP-7702 per-empty-account, …) comes from [`GasParams::new_spec`] which is already
//!   fork-aware via the upstream `SpecId` table. We don't duplicate those.
//! - **XLayer-specific gas** lives in dedicated [`GasId`] slots in the upper range (240+) so future
//!   revm `GasIds` (which currently extend through 39) won't collide. The [`XlayerGasParams`] trait
//!   gives them named getters.
//!
//! Why we don't follow op-revm's "per-fork named function" pattern (used in
//! `l1block.rs::calculate_tx_l1_cost_*`): per-fork branching scatters the
//! gas-pricing decision across many call sites. The table-with-override
//! design keeps the fork-aware mapping in one place and lets handler /
//! conversion code stay fork-agnostic.

use revm::{
    context::CfgEnv,
    context_interface::cfg::{GasId, GasParams},
};

use crate::spec::OpSpecId;

// ── XLayer gas slots ────────────────────────────────────────────────────────
//
// IDs picked from the high end of the [0, 255] range to leave space for both
// (a) future revm upstream additions (currently 1..=39) and (b) op-revm
// chain-extension slots if op ever adopts the GasParams convention.
//
// **Stability**: these IDs are part of the on-chain consensus pricing —
// changing them changes the table layout. Once a fork activates with a given
// ID assignment, the assignment must not be reused. Add new variants by
// allocating a fresh slot.

/// EIP-8130 K1 (secp256k1 ecrecover) per-call verification gas.
pub const fn k1_verification_gas() -> GasId {
    GasId::new(240)
}

/// EIP-8130 P256-raw per-call verification gas.
pub const fn p256_raw_verification_gas() -> GasId {
    GasId::new(241)
}

/// EIP-8130 P256-WebAuthn per-call verification gas (includes the SHA-256
/// challenge hashing + JSON parse + flag validation overhead).
pub const fn p256_webauthn_verification_gas() -> GasId {
    GasId::new(242)
}

/// EIP-8130 Delegate outer verifier per-call gas (added on top of the inner
/// verifier's cost).
pub const fn delegate_outer_verification_gas() -> GasId {
    GasId::new(243)
}

/// EIP-8130 cold nonce-key SSTORE intrinsic cost (first write to the slot).
///
/// Distinct from upstream `cold_storage_cost` (a pure SLOAD price) — this is
/// the conversion-time charge for "we'll do a fresh SSTORE on the nonce slot
/// during execution". `XLayer` prices it as cold-SLOAD (2100) + `SSTORE_SET`
/// (20000) = 22100; the precise breakdown happens in the handler.
pub const fn nonce_cold_gas() -> GasId {
    GasId::new(244)
}

/// EIP-8130 warm nonce-key SSTORE intrinsic cost (subsequent writes).
pub const fn nonce_warm_gas() -> GasId {
    GasId::new(245)
}

/// EIP-8130 expiring-nonce ring-buffer intrinsic cost
/// (`nonce_key == U256::MAX` path).
pub const fn expiring_nonce_gas() -> GasId {
    GasId::new(246)
}

/// EIP-8130 per-account-change-unit gas for `Create` entries.
///
/// One charge for the create itself plus one for each `initial_owner`
/// registration. Mirrors base's `CONFIG_CHANGE_OP_GAS` (`20_000`) which is the
/// SSTORE-set cost of writing each `owner_config` slot.
pub const fn aa_create_per_unit_gas() -> GasId {
    GasId::new(247)
}

/// EIP-8130 per-`OwnerChange` gas inside `ConfigChange` entries.
///
/// Charged per `owner_change` op when the entry targets the local chain (or
/// `chain_id == 0`). Mirrors base's `CONFIG_CHANGE_OP_GAS` (`20_000`).
pub const fn aa_config_change_per_op_gas() -> GasId {
    GasId::new(248)
}

/// EIP-8130 cost for a `ConfigChange` entry whose `chain_id` does not match
/// the tx's chain.
///
/// The handler still issues a single SLOAD to read the sequence and confirm
/// the entry is for a different chain before skipping it. Mirrors base's
/// `CONFIG_CHANGE_SKIP_GAS` = `SLOAD_GAS` = `2_100`.
pub const fn aa_config_change_skip_gas() -> GasId {
    GasId::new(249)
}

/// EIP-8130 per-`Delegation` entry gas.
///
/// Covers writing the EIP-7702-style 23-byte designator
/// (`0xef0100 || target`) into the sender's code slot. Mirrors base's
/// `BYTECODE_PER_BYTE_GAS * 23` = `4_600`.
pub const fn aa_delegation_gas() -> GasId {
    GasId::new(250)
}

/// EIP-8130 SLOAD gas for an authorizer's `owner_config` row read.
///
/// Composed from upstream `cold_storage_cost` at activation time so it
/// tracks Ethereum's spec without manual mirroring; future `XLayer` forks
/// can pin a literal here to decouple.
pub const fn aa_authorizer_sload_gas() -> GasId {
    GasId::new(253)
}

// ── Trait surface ───────────────────────────────────────────────────────────

/// Named getters for XLayer-specific entries in [`GasParams`].
///
/// Implemented for `&GasParams` so call sites read like
/// `cfg.gas_params().k1_verification_gas()` rather than
/// `cfg.gas_params().get(crate::gas_params::k1_verification_gas())`.
pub trait XlayerGasParams {
    /// Per-call gas for the K1 native verifier.
    fn k1_verification_gas(&self) -> u64;

    /// Per-call gas for the P256-raw native verifier.
    fn p256_raw_verification_gas(&self) -> u64;

    /// Per-call gas for the P256 `WebAuthn` native verifier.
    fn p256_webauthn_verification_gas(&self) -> u64;

    /// Outer-shell cost for the Delegate verifier (inner cost added separately).
    fn delegate_outer_verification_gas(&self) -> u64;

    /// Intrinsic cost for the first write to a fresh nonce slot.
    fn nonce_cold_gas(&self) -> u64;

    /// Intrinsic cost for subsequent (warm) nonce slot writes.
    fn nonce_warm_gas(&self) -> u64;

    /// Intrinsic cost for the expiring-nonce ring-buffer path.
    fn expiring_nonce_gas(&self) -> u64;

    /// Per-unit gas for a `Create` account-change entry (1 + `initial_owners`).
    fn aa_create_per_unit_gas(&self) -> u64;

    /// Per-op gas for an `OwnerChange` inside a matching `ConfigChange`.
    fn aa_config_change_per_op_gas(&self) -> u64;

    /// Gas for a `ConfigChange` entry whose `chain_id` doesn't match this chain.
    fn aa_config_change_skip_gas(&self) -> u64;

    /// Gas for a `Delegation` account-change entry.
    fn aa_delegation_gas(&self) -> u64;

    /// SLOAD gas for an authorizer's `owner_config` row read.
    fn aa_authorizer_sload_gas(&self) -> u64;
}

impl XlayerGasParams for GasParams {
    #[inline]
    fn k1_verification_gas(&self) -> u64 {
        self.get(k1_verification_gas())
    }
    #[inline]
    fn p256_raw_verification_gas(&self) -> u64 {
        self.get(p256_raw_verification_gas())
    }
    #[inline]
    fn p256_webauthn_verification_gas(&self) -> u64 {
        self.get(p256_webauthn_verification_gas())
    }
    #[inline]
    fn delegate_outer_verification_gas(&self) -> u64 {
        self.get(delegate_outer_verification_gas())
    }
    #[inline]
    fn nonce_cold_gas(&self) -> u64 {
        self.get(nonce_cold_gas())
    }
    #[inline]
    fn nonce_warm_gas(&self) -> u64 {
        self.get(nonce_warm_gas())
    }
    #[inline]
    fn expiring_nonce_gas(&self) -> u64 {
        self.get(expiring_nonce_gas())
    }
    #[inline]
    fn aa_create_per_unit_gas(&self) -> u64 {
        self.get(aa_create_per_unit_gas())
    }
    #[inline]
    fn aa_config_change_per_op_gas(&self) -> u64 {
        self.get(aa_config_change_per_op_gas())
    }
    #[inline]
    fn aa_config_change_skip_gas(&self) -> u64 {
        self.get(aa_config_change_skip_gas())
    }
    #[inline]
    fn aa_delegation_gas(&self) -> u64 {
        self.get(aa_delegation_gas())
    }
    #[inline]
    fn aa_authorizer_sload_gas(&self) -> u64 {
        self.get(aa_authorizer_sload_gas())
    }
}

// ── Factory ─────────────────────────────────────────────────────────────────

/// Builds a [`GasParams`] for `spec` with EIP-8130 native verifier and
/// AA-nonce intrinsic gas overlaid on top of the upstream EVM table.
///
/// Two classes of `XLayer` slot are populated here:
///
/// - **EVM-derived**: `nonce_cold_gas`, `nonce_warm_gas`, `expiring_nonce_gas` are `SSTORE_SET` /
///   `SSTORE_RESET` compositions from EIP-2929. We **compose them from the upstream `GasId::*`
///   slots** at activation time so the `XLayer` values automatically track Ethereum's spec (e.g., a
///   future upstream SSTORE-price fork that `XLAYER_V1` lands on top of will get the new values, no
///   manual re-mirroring). The composed value is captured into the `XLayer` slot once and stays
///   O(1) lookup at the call site.
///
/// - **XLayer-specific**: K1 / `P256Raw` / `P256WebAuthn` / Delegate verification gas are pure
///   `XLayer` protocol-design numbers (no EVM equivalent), so they're hardcoded as explicit
///   overrides.
///
/// Pre-`XLAYER_V1` returns a default (zeros) for `XLayer` slots — those slots
/// are not exercised because the AA-tx type isn't accepted before that fork.
/// Future `XLayer` forks (V2, …) can re-override individual entries here:
/// passing `(nonce_cold_gas(), 22_100)` on a V2 branch would *pin* the
/// composed value rather than let it drift with upstream.
pub fn xlayer_gas_params(spec: OpSpecId) -> GasParams {
    let mut params = GasParams::new_spec(spec.into_eth_spec());

    if spec.is_enabled_in(OpSpecId::XLAYER_V1) {
        // ── Compose EVM-derived intrinsic costs from the active base table ─
        //
        // Naming note: "cold" / "warm" here refers to whether this is the
        // *first* SSTORE for the `nonce_key` (value 0 → non-zero, SET) or
        // a *subsequent* one (value non-zero → non-zero, RESET). In **both
        // cases the slot is cold** at intrinsic-charge time — every tx's
        // first access to its nonce slot pays the COLD_SLOAD. The variable
        // part is SET vs RESET on top of the cold load.
        //
        // Berlin+ produces:
        //   nonce_cold = COLD_SLOAD (2100) + WARM_SLOAD (100) + (SSTORE_SET - WARM_SLOAD) (19900)
        //              = 22100  (cold + SSTORE_SET)
        //   nonce_warm = COLD_SLOAD (2100) + WARM_SLOAD (100) + (WARM_SSTORE_RESET - WARM_SLOAD)
        // (2800)              = 5000   (cold + SSTORE_RESET)
        //
        // expiring_nonce shares nonce_cold's formula (the ring-buffer write
        // is also cold + SET semantically).
        let cold_sload = params.get(GasId::cold_storage_cost());
        let sstore_static = params.get(GasId::sstore_static());
        let sstore_set_payload = params.get(GasId::sstore_set_without_load_cost());
        let sstore_reset_payload = params.get(GasId::sstore_reset_without_cold_load_cost());

        let derived_nonce_cold =
            cold_sload.saturating_add(sstore_static).saturating_add(sstore_set_payload);
        let derived_nonce_warm =
            cold_sload.saturating_add(sstore_static).saturating_add(sstore_reset_payload);

        params.override_gas([
            // ── XLayer-specific verifier prices ────────────────────────────
            // No EVM equivalent — pure protocol design choice. To bump these
            // at a future fork, add a `spec.is_enabled_in(OpSpecId::XLAYER_V2)`
            // branch with new values.
            (k1_verification_gas(), 6_000),
            (p256_raw_verification_gas(), 9_500),
            (p256_webauthn_verification_gas(), 15_000),
            (delegate_outer_verification_gas(), 6_000),
            // ── EVM-derived intrinsic costs ────────────────────────────────
            // Captured-at-activation composition. To pin a specific value at
            // a future fork (decoupling from upstream), add a literal override
            // on that fork branch.
            (nonce_cold_gas(), derived_nonce_cold),
            (nonce_warm_gas(), derived_nonce_warm),
            (expiring_nonce_gas(), derived_nonce_cold),
            // ── Account-change pricing (base reference values) ─────────────
            // SSTORE_SET on owner_config slot per Create-unit / per
            // ConfigChange op. Same shape as `nonce_cold_gas` (cold SLOAD +
            // SSTORE_SET) so we keep it composable from upstream.
            (aa_create_per_unit_gas(), derived_nonce_cold),
            (aa_config_change_per_op_gas(), derived_nonce_cold),
            // SLOAD-only cost for skipping a wrong-chain ConfigChange.
            (aa_config_change_skip_gas(), cold_sload),
            // EIP-7702 designator write: 23 bytes × per-byte deploy cost.
            // Hardcoded as 4_600 (= 200 × 23) because the per-byte cost is
            // bytecode-specific to AA, not the EVM's CREATE per-byte.
            (aa_delegation_gas(), 4_600),
            // SLOAD for the authorizer's owner_config row read; tracks
            // upstream cold_storage_cost.
            (aa_authorizer_sload_gas(), cold_sload),
        ]);
    }

    params
}

/// Installs the `XLayer` [`GasParams`] overlay on `cfg` when the spec is at or
/// beyond `XLAYER_V1`; otherwise returns `cfg` unchanged.
///
/// Consumed by every EVM construction site (native `OpEvmFactory`, FPVM
/// factory, …) so AA gas accounting is consensus-identical across execution
/// paths. Without this, callers would need to thread `with_gas_params`
/// themselves and `eip8130_gas` would silently bill 0 for the new
/// account-change costs in production.
#[inline]
pub fn install_xlayer_gas_params(cfg: CfgEnv<OpSpecId>) -> CfgEnv<OpSpecId> {
    if cfg.spec.is_enabled_in(OpSpecId::XLAYER_V1) {
        let spec = cfg.spec;
        cfg.with_gas_params(xlayer_gas_params(spec))
    } else {
        cfg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xlayer_v1_overrides_applied() {
        let params = xlayer_gas_params(OpSpecId::XLAYER_V1);
        // Hardcoded XLayer-specific overrides.
        assert_eq!(params.k1_verification_gas(), 6_000);
        assert_eq!(params.p256_raw_verification_gas(), 9_500);
        assert_eq!(params.p256_webauthn_verification_gas(), 15_000);
        assert_eq!(params.delegate_outer_verification_gas(), 6_000);
        // EVM-composed intrinsic costs (Berlin+: cold SLOAD 2100 + static 100
        // + SSTORE_SET 19900 = 22100; static 100 + SSTORE_RESET 4900 = 5000).
        assert_eq!(params.nonce_cold_gas(), 22_100);
        assert_eq!(params.nonce_warm_gas(), 5_000);
        assert_eq!(params.expiring_nonce_gas(), 22_100);
        // Account-change pricing: same SSTORE_SET shape as nonce_cold.
        assert_eq!(params.aa_create_per_unit_gas(), 22_100);
        assert_eq!(params.aa_config_change_per_op_gas(), 22_100);
        // SLOAD-only skip cost.
        assert_eq!(params.aa_config_change_skip_gas(), 2_100);
        // Hardcoded delegation pricing.
        assert_eq!(params.aa_delegation_gas(), 4_600);
        assert_eq!(params.aa_authorizer_sload_gas(), 2_100);
    }

    #[test]
    fn nonce_costs_compose_from_upstream_table() {
        // Pin the composition formula against the underlying GasId values
        // so a regression in either side fails loudly. If revm changes the
        // SSTORE breakdown semantics (e.g., merges sstore_static into
        // cold_storage_cost), this test is the early warning.
        let params = xlayer_gas_params(OpSpecId::XLAYER_V1);
        let cold_sload = params.get(GasId::cold_storage_cost());
        let sstore_static = params.get(GasId::sstore_static());
        let sstore_set_payload = params.get(GasId::sstore_set_without_load_cost());
        let sstore_reset_payload = params.get(GasId::sstore_reset_without_cold_load_cost());

        assert_eq!(
            params.nonce_cold_gas(),
            cold_sload + sstore_static + sstore_set_payload,
            "nonce_cold = cold SLOAD + warm-static + SSTORE_SET payload",
        );
        assert_eq!(
            params.nonce_warm_gas(),
            cold_sload + sstore_static + sstore_reset_payload,
            "nonce_warm = cold SLOAD + warm-static + SSTORE_RESET payload (slot is cold; \"warm\" names *the second use of the nonce_key*, not the slot's warmth)",
        );
        assert_eq!(
            params.expiring_nonce_gas(),
            params.nonce_cold_gas(),
            "expiring_nonce = nonce_cold (same cold-SLOAD + SSTORE_SET composition)",
        );
    }

    // (`nonce_costs_track_upstream_when_base_table_changes` was removed —
    // it claimed to verify "differs across base specs" but only inspected
    // XLAYER_V1, duplicating `nonce_costs_compose_from_upstream_table`. To
    // genuinely verify cross-spec tracking we'd need two OpSpecId variants
    // whose `into_eth_spec()` maps to specs with different SSTORE pricing;
    // none currently exist.)

    #[test]
    fn pre_xlayer_v1_xlayer_slots_zero() {
        // Pre-XLAYER_V1 forks: NONE of the 7 XLayer slots are overridden,
        // all default to 0 (verifier / nonce paths don't activate in this
        // regime). A regression that left even one slot leaking a
        // non-zero default into pre-fork blocks would change consensus on
        // those blocks.
        let params = xlayer_gas_params(OpSpecId::ISTHMUS);
        assert_eq!(params.k1_verification_gas(), 0, "k1 leaked pre-fork");
        assert_eq!(params.p256_raw_verification_gas(), 0, "p256_raw leaked pre-fork");
        assert_eq!(params.p256_webauthn_verification_gas(), 0, "webauthn leaked pre-fork");
        assert_eq!(params.delegate_outer_verification_gas(), 0, "delegate_outer leaked pre-fork",);
        assert_eq!(params.nonce_cold_gas(), 0, "nonce_cold leaked pre-fork");
        assert_eq!(params.nonce_warm_gas(), 0, "nonce_warm leaked pre-fork");
        assert_eq!(params.expiring_nonce_gas(), 0, "expiring_nonce leaked pre-fork");
        // New account-change slots must also be zero pre-fork.
        assert_eq!(params.aa_create_per_unit_gas(), 0, "aa_create_per_unit leaked pre-fork");
        assert_eq!(
            params.aa_config_change_per_op_gas(),
            0,
            "aa_config_change_per_op leaked pre-fork",
        );
        assert_eq!(params.aa_config_change_skip_gas(), 0, "aa_config_change_skip leaked pre-fork",);
        assert_eq!(params.aa_delegation_gas(), 0, "aa_delegation leaked pre-fork");
        assert_eq!(params.aa_authorizer_sload_gas(), 0, "aa_authorizer_sload leaked pre-fork");
    }

    #[test]
    fn upstream_evm_gas_inherited() {
        // Sanity: revm's fork-aware table is preserved through the override.
        // Berlin+ defines warm_storage_read_cost = 100, cold_storage_cost = 2100.
        let params = xlayer_gas_params(OpSpecId::XLAYER_V1);
        // tx_base_stipend is 21000 across all post-Frontier forks.
        assert_eq!(params.get(GasId::tx_base_stipend()), 21_000);
        // cold_storage_cost is 2100 from Berlin onwards (XLAYER_V1 inherits).
        assert_eq!(params.get(GasId::cold_storage_cost()), 2_100);
    }

    #[test]
    fn slot_ids_are_disjoint_from_upstream() {
        // Defense: if revm later adds a named GasId at any of our slots,
        // `GasId::name()` returns the upstream name instead of "unknown" —
        // this test fails with a clear message so we renumber before shipping
        // a fork that would silently overwrite revm's value.
        //
        // We also pin the high-end placement: keeping XLayer slots in 240+
        // gives revm headroom to add new IDs in its current 1..=39 range
        // without colliding. If revm ever crosses 200 it'll show up here.
        let xlayer_ids = [
            ("k1_verification_gas", k1_verification_gas()),
            ("p256_raw_verification_gas", p256_raw_verification_gas()),
            ("p256_webauthn_verification_gas", p256_webauthn_verification_gas()),
            ("delegate_outer_verification_gas", delegate_outer_verification_gas()),
            ("nonce_cold_gas", nonce_cold_gas()),
            ("nonce_warm_gas", nonce_warm_gas()),
            ("expiring_nonce_gas", expiring_nonce_gas()),
            ("aa_create_per_unit_gas", aa_create_per_unit_gas()),
            ("aa_config_change_per_op_gas", aa_config_change_per_op_gas()),
            ("aa_config_change_skip_gas", aa_config_change_skip_gas()),
            ("aa_delegation_gas", aa_delegation_gas()),
            ("aa_authorizer_sload_gas", aa_authorizer_sload_gas()),
        ];
        for (xlayer_name, id) in xlayer_ids {
            assert!(
                id.as_usize() >= 240,
                "XLayer GasId `{xlayer_name}` slot {} below reserved 240+ range; renumber",
                id.as_usize(),
            );
            assert_eq!(
                id.name(),
                "unknown",
                "XLayer GasId `{xlayer_name}` (slot {}) collides with upstream revm's named \
                 GasId `{}`; renumber the XLayer slot before shipping a fork",
                id.as_usize(),
                id.name(),
            );
        }
    }

    #[test]
    fn xlayer_slot_pairwise_distinct() {
        // Defense: prevent two XLayer GasIds from accidentally pointing to
        // the same slot (silent gas-table aliasing). If a future XLayer
        // const fn typo-aliases another, this test catches it before any
        // override masks the bug.
        let ids = [
            k1_verification_gas().as_u8(),
            p256_raw_verification_gas().as_u8(),
            p256_webauthn_verification_gas().as_u8(),
            delegate_outer_verification_gas().as_u8(),
            nonce_cold_gas().as_u8(),
            nonce_warm_gas().as_u8(),
            expiring_nonce_gas().as_u8(),
            aa_create_per_unit_gas().as_u8(),
            aa_config_change_per_op_gas().as_u8(),
            aa_config_change_skip_gas().as_u8(),
            aa_delegation_gas().as_u8(),
            aa_authorizer_sload_gas().as_u8(),
        ];
        let mut sorted = ids.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids.len(), sorted.len(), "duplicate XLayer GasId slot detected: {ids:?}",);
    }
}
