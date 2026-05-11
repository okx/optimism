//! EIP-8130 intrinsic gas computation.
//!
//! Protocol-fixed gas charged before any phase execution. The handler invokes
//! these helpers on demand at validation / execution time, reading the active
//! fork's [`GasParams`] from the cfg context (`cfg.gas_params()`); the values
//! are no longer cached as scalars on `Eip8130Parts`. Split into one fn per
//! cost component (auth payload, sender/payer auth load, sender/payer
//! verification, nonce key, account changes, bytecode, authorizer
//! verification) plus an aggregator [`aa_intrinsic_gas`].
//!
//! ## Inputs
//!
//! Every public fn takes `&Eip8130Parts`. Conversion (in
//! `alloy_op_evm::eip8130::parts::eip8130_parts`) precomputes the
//! fork-independent inputs the gas path needs:
//! [`Eip8130Parts::sender_payload_calldata_cost`] (the EIP-2028 calldata
//! cost of `tx.encoded_for_sender_signing()`),
//! [`Eip8130Parts::sender_auth`] / [`Eip8130Parts::payer_auth`] (the raw
//! blobs whose 20-byte verifier prefix dispatches per-side verification
//! gas), [`Eip8130Parts::is_eoa`] (forces K1 cost on the bare 65-byte sig
//! path), and [`Eip8130Parts::is_self_pay`] (zeroes payer-side costs).
//! With those captured, the gas path no longer needs to thread the
//! underlying `TxEip8130` through `OpTransaction`.
//!
//! ## Fork-aware pricing
//!
//! All gas values are read from a [`revm::context_interface::cfg::GasParams`]
//! handle threaded through every public fn. Inherited EVM gas (cold SLOAD,
//! tx base stipend, EIP-2028 token cost, …) comes from revm's per-fork
//! table; XLayer-specific verifier costs and AA-nonce intrinsic prices come
//! from `XlayerGasParams::*` named getters backed by reserved slot IDs.
//! See [`crate::gas_params`] for the slot allocation and the
//! `xlayer_gas_params(spec)` factory.
//!
//! Callers obtain `GasParams` from the cfg context: `cfg.gas_params()`.
//!
//! ## Placeholders
//!
//! account-changes / bytecode / authorizer-verification gas depend on
//! parsing `tx.account_changes` (slice 4-6 work), so those fns currently
//! return `0` with `TODO` comments marking the integration points.
//! `nonce_key_cost` returns the cold cost conservatively because warm/cold
//! determination requires storage state that's not available here; the
//! handler refunds the cold/warm delta later via [`nonce_warm_refund`].

use alloy_primitives::Address;
use revm::context_interface::cfg::GasParams;

use crate::{
    constants::{
        DELEGATE_VERIFIER_ADDRESS, K1_VERIFIER_ADDRESS, P256_RAW_VERIFIER_ADDRESS,
        P256_WEBAUTHN_VERIFIER_ADDRESS,
    },
    gas_params::XlayerGasParams,
    transaction::eip8130::Eip8130Parts,
};

/// Returns the per-verifier verification gas for a known native verifier.
///
/// `inner_native` is consulted only when `verifier == DELEGATE_VERIFIER_ADDRESS`;
/// it should be the address of the inner native verifier. `None` falls back
/// to the K1 cost as a conservative default for unknown / not-yet-determined
/// inner verifiers (matches base's behavior).
///
/// Custom verifiers return `0` here — they're charged separately against the
/// runtime-tunable [`crate::constants::XLAYER_AA_CUSTOM_VERIFIER_GAS_CAP`].
#[inline]
pub fn verifier_verification_gas(
    verifier: Address,
    inner_native: Option<Address>,
    params: &GasParams,
) -> u64 {
    if verifier == K1_VERIFIER_ADDRESS {
        params.k1_verification_gas()
    } else if verifier == P256_RAW_VERIFIER_ADDRESS {
        params.p256_raw_verification_gas()
    } else if verifier == P256_WEBAUTHN_VERIFIER_ADDRESS {
        params.p256_webauthn_verification_gas()
    } else if verifier == DELEGATE_VERIFIER_ADDRESS {
        let inner = inner_native.unwrap_or(K1_VERIFIER_ADDRESS);
        params
            .delegate_outer_verification_gas()
            .saturating_add(verifier_verification_gas(inner, None, params))
    } else {
        0
    }
}

/// Per-byte gas for the EIP-2028 calldata cost: 4 for zero, 16 for non-zero.
///
/// Public so [`alloy_op_evm::eip8130::parts::eip8130_parts`] can precompute
/// `Eip8130Parts::sender_payload_calldata_cost` once per tx at conversion
/// time (the encoding doesn't change after the tx is fixed, and the rates
/// are part of EIP-2028's locked baseline).
//
// Calldata pricing is fork-aware in revm via `tx_token_cost` (4) +
// `tx_token_non_zero_byte_multiplier` (4 → bytes contribute 4 or 4*4 = 16
// tokens). We keep the simple "4/16 per byte" formulation here because
// EIP-8130's signing preimage is a fixed encoding (not a free-form calldata
// payload) and the spec is locked to EIP-2028 values forever; if a future
// fork ever shifts these we'll plumb `params` through this fn too.
#[inline]
pub fn calldata_cost(bytes: &[u8]) -> u64 {
    // `bytes.iter().filter(...).count()` over a `bytecount` dep — the AA
    // signing preimages are typically <1 KiB and not on the hot per-call path.
    #[allow(clippy::naive_bytecount)]
    let zeros = bytes.iter().filter(|&&b| b == 0).count() as u64;
    let non_zeros = (bytes.len() as u64).saturating_sub(zeros);
    zeros.saturating_mul(4).saturating_add(non_zeros.saturating_mul(16))
}

/// SLOAD charged for the sender's `owner_config` row.
///
/// `0` when `sender_auth` is empty (estimateGas escape — caller adds the
/// missing-calldata overhead separately); cold SLOAD otherwise (Berlin+ →
/// `cold_storage_cost`, currently 2100).
#[inline]
pub fn sender_auth_cost(parts: &Eip8130Parts, params: &GasParams) -> u64 {
    if parts.sender_auth.is_empty() {
        0
    } else {
        params.get(revm::context_interface::cfg::GasId::cold_storage_cost())
    }
}

/// SLOAD charged for the payer's `owner_config` row.
///
/// `0` for self-pay (the sender's row already covers it); cold SLOAD
/// otherwise. Note: gating is `is_self_pay()`, not `payer_auth.is_empty()`
/// — sponsored estimateGas (empty `payer_auth` on a non-self-pay tx) still
/// pays the SLOAD because the row gets read at execution time.
#[inline]
pub fn payer_auth_cost(parts: &Eip8130Parts, params: &GasParams) -> u64 {
    if parts.is_self_pay() {
        0
    } else {
        params.get(revm::context_interface::cfg::GasId::cold_storage_cost())
    }
}

/// Per-verifier verification gas charged for `parts.sender_auth`.
///
/// EOA mode (`parts.is_eoa`): always K1 (the bare 65-byte sig path).
/// Explicit-from with an empty auth blob: K1 (the estimateGas escape; the
/// real auth that fills the slot will be K1-shaped).
/// Otherwise: derived from the auth blob's verifier prefix.
#[inline]
pub fn sender_verification_gas(parts: &Eip8130Parts, params: &GasParams) -> u64 {
    auth_verification_gas(parts.is_eoa, &parts.sender_auth, params)
}

/// Per-verifier verification gas charged for `parts.payer_auth`.
///
/// `0` for self-pay. Otherwise derived from the auth blob's verifier prefix.
#[inline]
pub fn payer_verification_gas(parts: &Eip8130Parts, params: &GasParams) -> u64 {
    if parts.is_self_pay() {
        return 0;
    }
    // Payer side has no EOA mode — always explicit-from / sponsored shape.
    auth_verification_gas(false, &parts.payer_auth, params)
}

/// Shared per-side verification gas helper. `is_eoa` short-circuits to K1
/// (the bare 65-byte sig path); explicit-from / sponsored mode parses the
/// 20-byte verifier prefix.
#[inline]
fn auth_verification_gas(is_eoa: bool, auth: &[u8], params: &GasParams) -> u64 {
    if is_eoa || auth.len() < 20 {
        return params.k1_verification_gas();
    }
    let verifier = Address::from_slice(&auth[..20]);
    let inner = delegate_inner_verifier_addr(verifier, &auth[20..]);
    verifier_verification_gas(verifier, inner, params)
}

/// Returns the inner verifier address inside a Delegate auth blob, or [`None`]
/// if the blob isn't a Delegate or the inner payload is too short.
///
/// The returned address is whatever the inner 20-byte prefix names —
/// native (K1 / `P256Raw` / `WebAuthn`) or custom. Custom inner is fine here:
/// `verifier_verification_gas(DELEGATE, Some(custom))` recurses into the
/// custom-verifier case which returns 0, so the only charge is the outer
/// delegate cost (the inner custom STATICCALL is paid separately against
/// `XLAYER_AA_CUSTOM_VERIFIER_GAS_CAP`).
///
/// Slice-only — no `Bytes` allocation.
#[inline]
fn delegate_inner_verifier_addr(verifier: Address, verifier_data: &[u8]) -> Option<Address> {
    if verifier != DELEGATE_VERIFIER_ADDRESS || verifier_data.len() < 20 {
        return None;
    }
    Some(Address::from_slice(&verifier_data[..20]))
}

/// Payer-only intrinsic gas portion (used by the payer precompile to report
/// `getMaxCost()`).
#[inline]
pub fn payer_intrinsic_gas(parts: &Eip8130Parts, params: &GasParams) -> u64 {
    payer_auth_cost(parts, params).saturating_add(payer_verification_gas(parts, params))
}

/// Nonce-key SLOAD/SSTORE cost (intrinsic, charged at validate-time).
///
/// Returns the expiring-nonce ring-buffer cost for `nonce_key == U256::MAX`.
/// Otherwise returns the cold cost: at intrinsic-gas time we don't have
/// state access to tell warm from cold, so we charge the worst case and let
/// the handler refund the difference at execution time via
/// [`nonce_warm_refund`].
#[inline]
pub fn nonce_key_cost(parts: &Eip8130Parts, params: &GasParams) -> u64 {
    use revm::primitives::U256;
    if parts.nonce_key == U256::MAX { params.expiring_nonce_gas() } else { params.nonce_cold_gas() }
}

/// Gas refunded into the per-phase execution budget when the nonce slot
/// turns out to be warm (its current value is > 1, meaning it's been
/// written by a prior tx).
///
/// Equals `nonce_cold_gas - nonce_warm_gas` — the amount overpaid by the
/// conservative cold charge in [`nonce_key_cost`]. Computing it from
/// [`GasParams`] keeps the refund in lockstep with whatever cold/warm
/// values the active fork is using; a future `XLayer` fork that re-prices
/// either cost flows through here automatically without a separate const
/// to update.
///
/// The handler reads the nonce slot during `execution()` and:
///
/// - if the slot value is `> 1`: adds this refund back to `gas_remaining` (the cold charge was
///   over-conservative);
/// - if `<= 1`: no adjustment (the cold charge was correct — slot was genuinely fresh / never
///   written).
///
/// Returns 0 in regimes where either nonce slot is zero (e.g., pre-XLAYER_V1
/// when neither is overridden), so callers don't need to special-case forks.
#[inline]
pub fn nonce_warm_refund(params: &GasParams) -> u64 {
    params.nonce_cold_gas().saturating_sub(params.nonce_warm_gas())
}

/// Per-entry account-change cost.
///
/// - **Create**: `aa_create_per_unit_gas * (1 + initial_owners_count)`.
///   At most one Create entry per tx (validated by `validate_env`); the
///   parser projects the create's `initial_owners` into
///   `pre_writes` (one per owner) and onto
///   [`Eip8130AccountChanges::create_initial_owners_count`][crate::transaction::eip8130::Eip8130AccountChanges::create_initial_owners_count].
/// - **`ConfigChange` (matching)**: `aa_config_change_per_op_gas * sum_owner_changes`. The parser
///   projects each matching entry into one
///   [`Eip8130AuthorizerValidation`][crate::transaction::eip8130::Eip8130AuthorizerValidation]
///   carrying its `owner_changes`; we sum across all of them.
/// - **`ConfigChange` (skipped)**: `aa_config_change_skip_gas` per entry.
///   The parser doesn't keep skipped entries; their count lives in
///   [`Eip8130AccountChanges::skipped_config_change_count`][crate::transaction::eip8130::Eip8130AccountChanges::skipped_config_change_count].
/// - **Delegation**: `aa_delegation_gas` per entry, summed via
///   [`Eip8130AccountChanges::delegation_entry_count`][crate::transaction::eip8130::Eip8130AccountChanges::delegation_entry_count].
///
/// Returns 0 pre-XLAYER_V1 because every slot defaults to 0; AA txs aren't
/// admitted before that fork in any case.
#[inline]
pub fn account_changes_cost(parts: &Eip8130Parts, params: &GasParams) -> u64 {
    let mut total: u64 = 0;

    // Create entry: 1 (the create itself) + N initial-owner registrations.
    if parts.account_changes.has_create_entry {
        let units = 1u64.saturating_add(parts.account_changes.create_initial_owners_count as u64);
        total = total.saturating_add(params.aa_create_per_unit_gas().saturating_mul(units));
    }

    // Matching ConfigChange entries: sum owner_changes across all kept
    // authorizer_validations.
    let matching_ops: u64 = parts
        .account_changes
        .authorizer_validations
        .iter()
        .map(|v| v.owner_changes.len() as u64)
        .sum();
    total = total.saturating_add(params.aa_config_change_per_op_gas().saturating_mul(matching_ops));

    // Skipped ConfigChange entries: one SLOAD per skip.
    total = total.saturating_add(
        params
            .aa_config_change_skip_gas()
            .saturating_mul(parts.account_changes.skipped_config_change_count as u64),
    );

    // Delegation entries: fixed per-entry charge.
    total = total.saturating_add(
        params
            .aa_delegation_gas()
            .saturating_mul(parts.account_changes.delegation_entry_count as u64),
    );

    total
}

/// Bytecode cost for the (≤1) Create entry.
///
/// `CREATE base + CODEDEPOSIT * code.len()`, sourced directly from the
/// upstream EVM gas table (`GasId::create()` = `32_000`, `GasId::code_deposit_cost()`
/// = 200) — EIP-8130 mirrors the EVM CREATE schedule here. Returns 0 when
/// no Create entry is present (no `code_placements`).
///
/// EIP-8130 doesn't split zero/nonzero per-byte for deployment code (unlike
/// EIP-2028 calldata), matching base's `CODEDEPOSIT` flat-rate schedule.
#[inline]
pub fn bytecode_cost(parts: &Eip8130Parts, params: &GasParams) -> u64 {
    let placement = match parts.account_changes.code_placements.first() {
        Some(p) => p,
        None => return 0,
    };

    let base = params.get(revm::context_interface::cfg::GasId::create());
    let per_byte = params.get(revm::context_interface::cfg::GasId::code_deposit_cost());
    base.saturating_add(per_byte.saturating_mul(placement.code.len() as u64))
}

/// Authorizer verification gas inside `ConfigChange` entries.
///
/// For each [`Eip8130AuthorizerValidation`][crate::transaction::eip8130::Eip8130AuthorizerValidation]:
///   - per-verifier verification gas via [`verifier_verification_gas`] (`0` for custom verifiers —
///     they're billed against [`crate::constants::XLAYER_AA_CUSTOM_VERIFIER_GAS_CAP`] at runtime),
///   - one SLOAD for the authorizer's `owner_config` row read.
///
/// Empty `authorizer_auth` blobs are surfaced by the parser as a
/// `verifier == Address::ZERO` placeholder; both the verifier-cost and the
/// SLOAD here charge 0 in that case (the handler rejects at the auth-state
/// stage before the row is read).
#[inline]
pub fn authorizer_verification_gas(parts: &Eip8130Parts, params: &GasParams) -> u64 {
    let mut total: u64 = 0;
    for validation in &parts.account_changes.authorizer_validations {
        if validation.verifier == Address::ZERO {
            // Empty / malformed authorizer_auth: no cost contribution.
            continue;
        }
        // Authorizer auth doesn't currently support nested Delegate (parser
        // rejects it as malformed → verifier == ZERO above). Pass `None` for
        // the inner native to match.
        total = total.saturating_add(verifier_verification_gas(validation.verifier, None, params));
        // SLOAD for the authorizer's owner_config row.
        total = total.saturating_add(params.aa_authorizer_sload_gas());
    }
    total
}

/// EIP-8130 intrinsic gas: AA-base + per-component costs aggregated.
///
/// The handler reads this back during `validate_initial_tx_gas`, gas
/// deduction, and the AA execution-gas-limit / max-cost computations.
/// Computed on demand from `(parts, cfg.gas_params())`; the
/// fork-independent payload-cost input is precomputed at conversion time
/// into [`Eip8130Parts::sender_payload_calldata_cost`].
#[inline]
pub fn aa_intrinsic_gas(parts: &Eip8130Parts, params: &GasParams) -> u64 {
    use revm::context_interface::cfg::GasId;
    params
        .get(GasId::tx_base_stipend())
        .saturating_add(parts.sender_payload_calldata_cost)
        .saturating_add(sender_auth_cost(parts, params))
        .saturating_add(payer_auth_cost(parts, params))
        .saturating_add(sender_verification_gas(parts, params))
        .saturating_add(payer_verification_gas(parts, params))
        .saturating_add(authorizer_verification_gas(parts, params))
        .saturating_add(nonce_key_cost(parts, params))
        .saturating_add(bytecode_cost(parts, params))
        .saturating_add(account_changes_cost(parts, params))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OpSpecId, gas_params::xlayer_gas_params};
    use alloy_primitives::Bytes;
    use revm::{
        context_interface::cfg::GasId,
        primitives::{Address, U256},
    };

    /// Test [`GasParams`] for `XLAYER_V1` — cached once for the test module.
    fn params() -> GasParams {
        xlayer_gas_params(OpSpecId::XLAYER_V1)
    }

    /// Build a sender-auth blob: `verifier (20 bytes) || verifier_data`.
    fn auth_blob(verifier: Address, data: &[u8]) -> Bytes {
        let mut buf = Vec::with_capacity(20 + data.len());
        buf.extend_from_slice(verifier.as_slice());
        buf.extend_from_slice(data);
        Bytes::from(buf)
    }

    /// Mirrors what `eip8130_parts` produces for an explicit-from sponsored
    /// tx with non-empty `sender_auth` and empty `payer_auth`. Tests override
    /// individual fields per scenario.
    fn empty_parts() -> Eip8130Parts {
        Eip8130Parts {
            sender: Address::repeat_byte(0x01),
            payer: Address::repeat_byte(0x33),
            sender_auth: Bytes::from(vec![0u8; 85]),
            payer_auth: Bytes::new(),
            // Non-zero, fixed payload cost so aggregator tests can pin the
            // exact contribution without re-encoding RLP. Real conversion
            // computes this from `calldata_cost(tx.encoded_for_sender_signing())`.
            sender_payload_calldata_cost: 100,
            is_eoa: false,
            ..Default::default()
        }
    }

    fn parts_self_pay(sender_auth: Bytes) -> Eip8130Parts {
        let mut p = empty_parts();
        p.payer = p.sender; // self-pay
        p.sender_auth = sender_auth;
        p.payer_auth = Bytes::new();
        p
    }

    fn parts_with_sender_auth(sender_auth: Bytes) -> Eip8130Parts {
        let mut p = empty_parts();
        p.sender_auth = sender_auth;
        p
    }

    fn parts_sponsored(payer_auth: Bytes) -> Eip8130Parts {
        let mut p = empty_parts();
        p.payer_auth = payer_auth;
        p
    }

    // ── verifier_verification_gas ──

    #[test]
    fn k1_verification_gas() {
        let p = params();
        assert_eq!(
            verifier_verification_gas(K1_VERIFIER_ADDRESS, None, &p),
            p.k1_verification_gas()
        );
    }

    #[test]
    fn p256_raw_verification_gas() {
        let p = params();
        assert_eq!(
            verifier_verification_gas(P256_RAW_VERIFIER_ADDRESS, None, &p),
            p.p256_raw_verification_gas(),
        );
    }

    #[test]
    fn webauthn_verification_gas() {
        let p = params();
        assert_eq!(
            verifier_verification_gas(P256_WEBAUTHN_VERIFIER_ADDRESS, None, &p),
            p.p256_webauthn_verification_gas(),
        );
    }

    #[test]
    fn delegate_native_inner_verification_gas() {
        let p = params();
        let g = verifier_verification_gas(DELEGATE_VERIFIER_ADDRESS, Some(K1_VERIFIER_ADDRESS), &p);
        assert_eq!(g, p.delegate_outer_verification_gas() + p.k1_verification_gas());
    }

    #[test]
    fn delegate_inner_p256_raw_verification_gas() {
        let p = params();
        let g = verifier_verification_gas(
            DELEGATE_VERIFIER_ADDRESS,
            Some(P256_RAW_VERIFIER_ADDRESS),
            &p,
        );
        assert_eq!(g, p.delegate_outer_verification_gas() + p.p256_raw_verification_gas());
    }

    #[test]
    fn delegate_inner_webauthn_verification_gas() {
        let p = params();
        let g = verifier_verification_gas(
            DELEGATE_VERIFIER_ADDRESS,
            Some(P256_WEBAUTHN_VERIFIER_ADDRESS),
            &p,
        );
        assert_eq!(g, p.delegate_outer_verification_gas() + p.p256_webauthn_verification_gas());
    }

    #[test]
    fn delegate_none_inner_falls_back_to_k1() {
        // `inner_native = None` (auth-state hasn't determined the inner
        // native yet) → fall back to K1 cost. Distinct from the
        // unknown-address case (covered by
        // `delegate_inner_custom_verifier_falls_back_to_zero` below) where
        // inner is `Some(custom)` and recurses to 0.
        let p = params();
        let g = verifier_verification_gas(DELEGATE_VERIFIER_ADDRESS, None, &p);
        assert_eq!(g, p.delegate_outer_verification_gas() + p.k1_verification_gas());
    }

    #[test]
    fn delegate_inner_custom_verifier_falls_back_to_zero() {
        let p = params();
        // Inner is custom (unknown native): inner cost is 0, only outer is charged.
        let g = verifier_verification_gas(
            DELEGATE_VERIFIER_ADDRESS,
            Some(Address::repeat_byte(0xAB)),
            &p,
        );
        assert_eq!(g, p.delegate_outer_verification_gas());
    }

    /// Defense-only: a Delegate-inside-Delegate auth structure recurses and
    /// double-charges the outer cost. The auth-state-level 1-hop guard makes
    /// this path unreachable in practice (validation rejects nested delegates
    /// before they reach gas computation).
    #[test]
    fn delegate_nested_delegate_inner_double_charges_outer() {
        let p = params();
        let g = verifier_verification_gas(
            DELEGATE_VERIFIER_ADDRESS,
            Some(DELEGATE_VERIFIER_ADDRESS),
            &p,
        );
        assert_eq!(g, p.delegate_outer_verification_gas() * 2 + p.k1_verification_gas(),);
    }

    #[test]
    fn custom_verifier_returns_zero() {
        let p = params();
        assert_eq!(verifier_verification_gas(Address::repeat_byte(0xAB), None, &p), 0);
    }

    // ── calldata_cost ──

    #[test]
    fn calldata_cost_empty_is_zero() {
        assert_eq!(calldata_cost(&[]), 0);
    }

    #[test]
    fn calldata_cost_all_zeros() {
        assert_eq!(calldata_cost(&[0u8; 10]), 40);
    }

    #[test]
    fn calldata_cost_all_non_zeros() {
        assert_eq!(calldata_cost(&[0xFFu8; 10]), 160);
    }

    #[test]
    fn calldata_cost_mixed() {
        // 5 zeros + 3 non-zeros = 5*4 + 3*16 = 68.
        let bytes = [0u8, 0, 0, 0, 0, 1, 2, 3];
        assert_eq!(calldata_cost(&bytes), 68);
    }

    // ── sender_auth_cost / payer_auth_cost ──

    #[test]
    fn sender_auth_cost_empty_is_zero() {
        let p = params();
        let mut parts = empty_parts();
        parts.sender_auth = Bytes::new();
        assert_eq!(sender_auth_cost(&parts, &p), 0);
    }

    #[test]
    fn sender_auth_cost_non_empty_is_cold_sload() {
        let p = params();
        let parts = empty_parts();
        assert!(!parts.sender_auth.is_empty());
        assert_eq!(sender_auth_cost(&parts, &p), p.get(GasId::cold_storage_cost()));
    }

    #[test]
    fn payer_auth_cost_self_pay_is_zero() {
        let p = params();
        let parts = parts_self_pay(Bytes::new());
        assert!(parts.is_self_pay());
        assert_eq!(payer_auth_cost(&parts, &p), 0);
    }

    #[test]
    fn payer_auth_cost_sponsored_with_empty_auth_is_cold_sload() {
        // Sponsored gating is `is_self_pay()`, NOT `payer_auth.is_empty()`.
        let p = params();
        let parts = parts_sponsored(Bytes::new());
        assert!(!parts.is_self_pay());
        assert_eq!(payer_auth_cost(&parts, &p), p.get(GasId::cold_storage_cost()));
    }

    #[test]
    fn payer_auth_cost_sponsored_non_empty_is_cold_sload() {
        let p = params();
        let parts = parts_sponsored(auth_blob(K1_VERIFIER_ADDRESS, &[0u8; 65]));
        assert_eq!(payer_auth_cost(&parts, &p), p.get(GasId::cold_storage_cost()));
    }

    // ── sender_verification_gas ──

    #[test]
    fn sender_verification_gas_eoa_uses_k1() {
        let p = params();
        let mut parts = empty_parts();
        parts.is_eoa = true; // EOA mode
        // Even with non-K1-shaped auth, EOA mode forces K1 cost.
        parts.sender_auth = auth_blob(P256_WEBAUTHN_VERIFIER_ADDRESS, &[0u8; 200]);
        assert_eq!(sender_verification_gas(&parts, &p), p.k1_verification_gas());
    }

    #[test]
    fn sender_verification_gas_explicit_from_short_auth_defaults_to_k1() {
        let p = params();
        let parts = parts_with_sender_auth(Bytes::from(vec![0u8; 5]));
        assert_eq!(sender_verification_gas(&parts, &p), p.k1_verification_gas());
    }

    #[test]
    fn sender_verification_gas_explicit_from_empty_auth_defaults_to_k1() {
        let p = params();
        let parts = parts_with_sender_auth(Bytes::new());
        assert_eq!(sender_verification_gas(&parts, &p), p.k1_verification_gas());
    }

    #[test]
    fn sender_verification_gas_k1_prefix() {
        let p = params();
        let parts = parts_with_sender_auth(auth_blob(K1_VERIFIER_ADDRESS, &[0u8; 65]));
        assert_eq!(sender_verification_gas(&parts, &p), p.k1_verification_gas());
    }

    #[test]
    fn sender_verification_gas_p256_raw_prefix() {
        let p = params();
        let parts = parts_with_sender_auth(auth_blob(P256_RAW_VERIFIER_ADDRESS, &[0u8; 128]));
        assert_eq!(sender_verification_gas(&parts, &p), p.p256_raw_verification_gas());
    }

    #[test]
    fn sender_verification_gas_webauthn_prefix() {
        let p = params();
        let parts = parts_with_sender_auth(auth_blob(P256_WEBAUTHN_VERIFIER_ADDRESS, &[0u8; 200]));
        assert_eq!(sender_verification_gas(&parts, &p), p.p256_webauthn_verification_gas());
    }

    #[test]
    fn sender_verification_gas_delegate_with_k1_inner() {
        let p = params();
        let mut inner = Vec::new();
        inner.extend_from_slice(K1_VERIFIER_ADDRESS.as_slice());
        inner.extend_from_slice(&[0u8; 65]);
        let parts = parts_with_sender_auth(auth_blob(DELEGATE_VERIFIER_ADDRESS, &inner));
        assert_eq!(
            sender_verification_gas(&parts, &p),
            p.delegate_outer_verification_gas() + p.k1_verification_gas(),
        );
    }

    #[test]
    fn sender_verification_gas_delegate_with_custom_inner_charges_outer_only() {
        let p = params();
        let custom_inner = Address::repeat_byte(0xAB);
        let mut inner = Vec::new();
        inner.extend_from_slice(custom_inner.as_slice());
        let parts = parts_with_sender_auth(auth_blob(DELEGATE_VERIFIER_ADDRESS, &inner));
        assert_eq!(sender_verification_gas(&parts, &p), p.delegate_outer_verification_gas());
    }

    #[test]
    fn sender_verification_gas_delegate_with_short_inner_falls_back_to_k1() {
        let p = params();
        let parts = parts_with_sender_auth(auth_blob(DELEGATE_VERIFIER_ADDRESS, &[0u8; 5]));
        assert_eq!(
            sender_verification_gas(&parts, &p),
            p.delegate_outer_verification_gas() + p.k1_verification_gas(),
        );
    }

    #[test]
    fn sender_verification_gas_custom_verifier_is_zero() {
        let p = params();
        let custom = Address::repeat_byte(0xAB);
        let parts = parts_with_sender_auth(auth_blob(custom, &[0u8; 100]));
        assert_eq!(sender_verification_gas(&parts, &p), 0);
    }

    // ── payer_verification_gas ──

    #[test]
    fn self_pay_zero_payer_costs() {
        let p = params();
        let parts = parts_self_pay(Bytes::from(vec![0u8; 85]));
        assert!(parts.is_self_pay());
        assert_eq!(payer_auth_cost(&parts, &p), 0);
        assert_eq!(payer_verification_gas(&parts, &p), 0);
        assert_eq!(payer_intrinsic_gas(&parts, &p), 0);
    }

    #[test]
    fn payer_verification_gas_sponsored_short_auth_defaults_to_k1() {
        let p = params();
        let parts = parts_sponsored(Bytes::from(vec![0u8; 5]));
        assert_eq!(payer_verification_gas(&parts, &p), p.k1_verification_gas());
    }

    #[test]
    fn payer_verification_gas_sponsored_k1_prefix() {
        let p = params();
        let parts = parts_sponsored(auth_blob(K1_VERIFIER_ADDRESS, &[0u8; 65]));
        assert_eq!(payer_verification_gas(&parts, &p), p.k1_verification_gas());
    }

    #[test]
    fn payer_verification_gas_sponsored_webauthn_prefix() {
        let p = params();
        let parts = parts_sponsored(auth_blob(P256_WEBAUTHN_VERIFIER_ADDRESS, &[0u8; 200]));
        assert_eq!(payer_verification_gas(&parts, &p), p.p256_webauthn_verification_gas());
    }

    #[test]
    fn payer_verification_gas_sponsored_custom_is_zero() {
        let p = params();
        let custom = Address::repeat_byte(0xCD);
        let parts = parts_sponsored(auth_blob(custom, &[0u8; 100]));
        assert_eq!(payer_verification_gas(&parts, &p), 0);
    }

    // ── payer_intrinsic_gas ──

    #[test]
    fn payer_intrinsic_gas_self_pay_zero() {
        let p = params();
        let parts = parts_self_pay(Bytes::from(vec![0u8; 85]));
        assert_eq!(payer_intrinsic_gas(&parts, &p), 0);
    }

    #[test]
    fn payer_intrinsic_gas_sponsored_k1() {
        let p = params();
        let parts = parts_sponsored(auth_blob(K1_VERIFIER_ADDRESS, &[0u8; 65]));
        assert_eq!(
            payer_intrinsic_gas(&parts, &p),
            p.get(GasId::cold_storage_cost()) + p.k1_verification_gas(),
        );
    }

    #[test]
    fn payer_intrinsic_gas_sponsored_custom() {
        let p = params();
        // SLOAD still charged for the row read, but verification cost is 0.
        let custom = Address::repeat_byte(0xCD);
        let parts = parts_sponsored(auth_blob(custom, &[0u8; 100]));
        assert_eq!(payer_intrinsic_gas(&parts, &p), p.get(GasId::cold_storage_cost()));
    }

    // ── nonce_key_cost ──

    #[test]
    fn nonce_free_returns_expiring_cost() {
        let p = params();
        let mut parts = empty_parts();
        parts.nonce_key = U256::MAX;
        assert_eq!(nonce_key_cost(&parts, &p), p.expiring_nonce_gas());
    }

    #[test]
    fn nonce_key_zero_returns_cold_cost() {
        let p = params();
        let mut parts = empty_parts();
        parts.nonce_key = U256::ZERO;
        assert_eq!(nonce_key_cost(&parts, &p), p.nonce_cold_gas());
    }

    #[test]
    fn nonce_key_non_zero_returns_cold_cost() {
        let p = params();
        let mut parts = empty_parts();
        parts.nonce_key = U256::from(42u64);
        assert_eq!(nonce_key_cost(&parts, &p), p.nonce_cold_gas());
    }

    // ── account_changes_cost / bytecode_cost / authorizer_verification_gas ──

    #[test]
    fn account_changes_cost_empty_parts_is_zero() {
        let p = params();
        let parts = empty_parts();
        // No create / matching / skipped / delegation entries → 0.
        assert_eq!(account_changes_cost(&parts, &p), 0);
    }

    #[test]
    fn account_changes_cost_create_entry() {
        let p = params();
        let mut parts = empty_parts();
        parts.account_changes.has_create_entry = true;
        parts.account_changes.create_initial_owners_count = 2;
        // 1 (create) + 2 (owners) units, each at aa_create_per_unit_gas.
        let expected = p.aa_create_per_unit_gas() * 3;
        assert_eq!(account_changes_cost(&parts, &p), expected);
    }

    #[test]
    fn account_changes_cost_config_change_matching() {
        use crate::transaction::eip8130::{Eip8130AuthorizerValidation, Eip8130ConfigOp};
        let p = params();
        let mut parts = empty_parts();
        parts.account_changes.authorizer_validations = vec![Eip8130AuthorizerValidation {
            verifier: Address::ZERO,
            owner_id: alloy_primitives::B256::ZERO,
            verify_call: None,
            owner_changes: vec![
                Eip8130ConfigOp::default(),
                Eip8130ConfigOp::default(),
                Eip8130ConfigOp::default(),
            ],
        }];
        // Three matching ops at aa_config_change_per_op_gas.
        let expected = p.aa_config_change_per_op_gas() * 3;
        assert_eq!(account_changes_cost(&parts, &p), expected);
    }

    #[test]
    fn account_changes_cost_config_change_skipped() {
        let p = params();
        let mut parts = empty_parts();
        parts.account_changes.skipped_config_change_count = 4;
        let expected = p.aa_config_change_skip_gas() * 4;
        assert_eq!(account_changes_cost(&parts, &p), expected);
    }

    #[test]
    fn account_changes_cost_delegation() {
        let p = params();
        let mut parts = empty_parts();
        parts.account_changes.delegation_entry_count = 1;
        assert_eq!(account_changes_cost(&parts, &p), p.aa_delegation_gas());
    }

    #[test]
    fn account_changes_cost_pre_fork_zero() {
        // Pre-XLAYER_V1 forks zero out every per-entry slot, so even a
        // populated parts struct charges 0.
        let p = xlayer_gas_params(OpSpecId::ISTHMUS);
        let mut parts = empty_parts();
        parts.account_changes.has_create_entry = true;
        parts.account_changes.create_initial_owners_count = 5;
        parts.account_changes.skipped_config_change_count = 2;
        parts.account_changes.delegation_entry_count = 1;
        assert_eq!(account_changes_cost(&parts, &p), 0);
    }

    #[test]
    fn bytecode_cost_no_create_is_zero() {
        let p = params();
        let parts = empty_parts();
        assert_eq!(bytecode_cost(&parts, &p), 0);
    }

    #[test]
    fn bytecode_cost_create_entry() {
        use crate::transaction::eip8130::Eip8130CodePlacement;
        let p = params();
        let mut parts = empty_parts();
        parts.account_changes.has_create_entry = true;
        parts.account_changes.code_placements = vec![Eip8130CodePlacement {
            address: Address::repeat_byte(0xDE),
            code: Bytes::from_static(&[0x60u8; 100]),
        }];
        // 32_000 + 200 * 100 (sourced from upstream `GasId::create` /
        // `GasId::code_deposit_cost`).
        let base = p.get(revm::context_interface::cfg::GasId::create());
        let per_byte = p.get(revm::context_interface::cfg::GasId::code_deposit_cost());
        assert_eq!(bytecode_cost(&parts, &p), base + per_byte * 100);
    }

    #[test]
    fn bytecode_cost_create_entry_zero_byte_code_charges_only_base() {
        // EIP-8130's base cost applies even to a zero-length deployment.
        use crate::transaction::eip8130::Eip8130CodePlacement;
        let p = params();
        let mut parts = empty_parts();
        parts.account_changes.has_create_entry = true;
        parts.account_changes.code_placements =
            vec![Eip8130CodePlacement { address: Address::repeat_byte(0xDE), code: Bytes::new() }];
        let base = p.get(revm::context_interface::cfg::GasId::create());
        assert_eq!(bytecode_cost(&parts, &p), base);
    }

    #[test]
    fn authorizer_verification_gas_per_validation() {
        use crate::transaction::eip8130::{Eip8130AuthorizerValidation, Eip8130ConfigOp};
        let p = params();
        let mut parts = empty_parts();
        parts.account_changes.authorizer_validations = vec![
            Eip8130AuthorizerValidation {
                verifier: K1_VERIFIER_ADDRESS,
                owner_id: alloy_primitives::B256::ZERO,
                verify_call: None,
                owner_changes: vec![Eip8130ConfigOp::default()],
            },
            Eip8130AuthorizerValidation {
                verifier: P256_RAW_VERIFIER_ADDRESS,
                owner_id: alloy_primitives::B256::ZERO,
                verify_call: None,
                owner_changes: vec![Eip8130ConfigOp::default()],
            },
        ];
        // Each validation contributes verifier_gas + aa_authorizer_sload_gas.
        let expected = (p.k1_verification_gas() + p.aa_authorizer_sload_gas()) +
            (p.p256_raw_verification_gas() + p.aa_authorizer_sload_gas());
        assert_eq!(authorizer_verification_gas(&parts, &p), expected);
    }

    #[test]
    fn authorizer_verification_gas_empty_verifier_is_skipped() {
        use crate::transaction::eip8130::{Eip8130AuthorizerValidation, Eip8130ConfigOp};
        let p = params();
        let mut parts = empty_parts();
        parts.account_changes.authorizer_validations = vec![Eip8130AuthorizerValidation {
            // Parser placeholder for empty / malformed authorizer_auth.
            verifier: Address::ZERO,
            owner_id: alloy_primitives::B256::ZERO,
            verify_call: None,
            owner_changes: vec![Eip8130ConfigOp::default()],
        }];
        assert_eq!(authorizer_verification_gas(&parts, &p), 0);
    }

    #[test]
    fn authorizer_verification_gas_custom_verifier_charges_only_sload() {
        use crate::transaction::eip8130::{Eip8130AuthorizerValidation, Eip8130ConfigOp};
        let p = params();
        let mut parts = empty_parts();
        parts.account_changes.authorizer_validations = vec![Eip8130AuthorizerValidation {
            verifier: Address::repeat_byte(0xAB),
            owner_id: alloy_primitives::B256::ZERO,
            verify_call: None,
            owner_changes: vec![Eip8130ConfigOp::default()],
        }];
        // verifier_verification_gas returns 0 for unknown addresses; only
        // the SLOAD shows up.
        assert_eq!(authorizer_verification_gas(&parts, &p), p.aa_authorizer_sload_gas());
    }

    #[test]
    fn authorizer_verification_gas_pre_fork_zero() {
        use crate::transaction::eip8130::{Eip8130AuthorizerValidation, Eip8130ConfigOp};
        let p = xlayer_gas_params(OpSpecId::ISTHMUS);
        let mut parts = empty_parts();
        parts.account_changes.authorizer_validations = vec![Eip8130AuthorizerValidation {
            verifier: K1_VERIFIER_ADDRESS,
            owner_id: alloy_primitives::B256::ZERO,
            verify_call: None,
            owner_changes: vec![Eip8130ConfigOp::default()],
        }];
        assert_eq!(authorizer_verification_gas(&parts, &p), 0);
    }

    // ── aa_intrinsic_gas (aggregator) ──

    #[test]
    fn intrinsic_gas_includes_base_cost() {
        let p = params();
        let parts = empty_parts();
        assert!(aa_intrinsic_gas(&parts, &p) >= p.get(GasId::tx_base_stipend()));
    }

    #[test]
    fn intrinsic_gas_default_parts_at_least_base_plus_k1_plus_nonce_cold() {
        // Default Eip8130Parts: sender == payer == ZERO (self-pay), is_eoa
        // = false, empty sender_auth, payload_cost = 0, nonce_key = 0, no
        // account_changes. Result should be:
        // tx_base_stipend + 0 (payload) + 0 (sender_auth empty)
        // + 0 (self-pay) + K1 (auth empty → fallback) + 0 (self-pay)
        // + 0 (placeholders) + nonce_cold_gas.
        let p = params();
        let parts = Eip8130Parts::default();
        let expected =
            p.get(GasId::tx_base_stipend()) + p.k1_verification_gas() + p.nonce_cold_gas();
        assert_eq!(aa_intrinsic_gas(&parts, &p), expected);
    }

    #[test]
    fn intrinsic_gas_explicit_from_k1_matches_components() {
        let p = params();
        // Explicit-from sender (K1 prefix), sponsored payer (empty payer_auth
        // → K1 fallback in sender_verification_gas helper).
        let mut parts = parts_with_sender_auth(auth_blob(K1_VERIFIER_ADDRESS, &[0u8; 65]));
        parts.sender_payload_calldata_cost = 1_234;
        let cold = p.get(GasId::cold_storage_cost());
        let expected = p.get(GasId::tx_base_stipend())
            + parts.sender_payload_calldata_cost
            + cold                          // sender_auth_cost (non-empty)
            + cold                          // payer_auth_cost (sponsored)
            + p.k1_verification_gas()       // sender_verification_gas (K1 prefix)
            + p.k1_verification_gas()       // payer_verification_gas (empty payer_auth → K1)
            + p.nonce_cold_gas();
        assert_eq!(aa_intrinsic_gas(&parts, &p), expected);
    }

    #[test]
    fn intrinsic_gas_self_pay_webauthn_matches_components() {
        let p = params();
        let mut parts = parts_self_pay(auth_blob(P256_WEBAUTHN_VERIFIER_ADDRESS, &[0u8; 200]));
        parts.sender_payload_calldata_cost = 2_500;
        let expected = p.get(GasId::tx_base_stipend())
            + parts.sender_payload_calldata_cost
            + p.get(GasId::cold_storage_cost())  // sender_auth_cost
            + p.p256_webauthn_verification_gas() // sender_verification_gas
            + p.nonce_cold_gas(); // payer side all zero
        assert_eq!(aa_intrinsic_gas(&parts, &p), expected);
    }

    // ── nonce_warm_refund ──

    #[test]
    fn nonce_warm_refund_xlayer_v1_matches_composition() {
        // XLAYER_V1 / Berlin+: cold = 22100, warm = 5000, refund = 17100.
        // Pin the exact value so a regression that breaks either side of
        // the composition (or the refund formula) fails loudly here rather
        // than silently mispricing every AA tx with a warm nonce.
        let p = params();
        assert_eq!(p.nonce_cold_gas(), 22_100, "Berlin+ cold");
        assert_eq!(p.nonce_warm_gas(), 5_000, "Berlin+ warm");
        assert_eq!(nonce_warm_refund(&p), 17_100, "Berlin+ refund");
    }

    #[test]
    fn nonce_warm_refund_equals_cold_minus_warm() {
        // Algebraic invariant: refund + warm == cold. If a future fork
        // re-prices either side, this still holds as long as the helper
        // reads from params.
        let p = params();
        assert_eq!(
            nonce_warm_refund(&p) + p.nonce_warm_gas(),
            p.nonce_cold_gas(),
            "refund + warm must reconstruct cold",
        );
    }

    #[test]
    fn nonce_warm_refund_pre_xlayer_v1_is_zero() {
        // Pre-fork: both nonce slots are 0 → refund is 0 (no overcharge to
        // refund). Saturating subtraction means it never goes negative even
        // if a future fork accidentally inverted cold/warm.
        let p = xlayer_gas_params(OpSpecId::ISTHMUS);
        assert_eq!(nonce_warm_refund(&p), 0);
    }

    #[test]
    fn nonce_warm_refund_saturates_when_warm_exceeds_cold() {
        // Defense-in-depth: if a buggy fork override set warm > cold (the
        // refund would mathematically be negative), `saturating_sub` clamps
        // to 0 rather than wrapping to a huge u64 that would over-credit
        // the gas budget. Construct such an inverted table by hand and
        // confirm the helper returns 0.
        let mut p = xlayer_gas_params(OpSpecId::XLAYER_V1);
        p.override_gas([
            (crate::gas_params::nonce_cold_gas(), 1_000),
            (crate::gas_params::nonce_warm_gas(), 5_000),
        ]);
        assert_eq!(nonce_warm_refund(&p), 0, "must clamp to 0 not wrap");
    }

    /// Sanity: pre-XLAYER_V1 forks have the `XLayer` slots zeroed, so the
    /// aggregator computes only `tx_base_stipend + payload + auth_cost +
    /// payer_auth_cost` — every XLayer-specific component (verification,
    /// nonce) is 0. The path is unreachable at runtime because `validate_env`
    /// gates on `XLAYER_V1`, but pinning the aggregator output catches a
    /// regression where any `XLayer` slot leaked a non-zero default into a
    /// pre-fork block (which would change consensus on those blocks).
    #[test]
    fn pre_xlayer_v1_params_zero_xlayer_slots() {
        let p = xlayer_gas_params(OpSpecId::ISTHMUS);
        let parts = empty_parts();

        // Per-component: every XLayer-specific gas is 0 pre-fork.
        assert_eq!(sender_verification_gas(&parts, &p), 0);
        assert_eq!(payer_verification_gas(&parts, &p), 0);
        assert_eq!(nonce_key_cost(&parts, &p), 0);

        // Aggregator: only the upstream-EVM components contribute.
        // empty_parts() has explicit-from (is_eoa=false), sender_auth = 85
        // zero bytes, sponsored payer with empty payer_auth.
        let expected = p.get(GasId::tx_base_stipend())
            + parts.sender_payload_calldata_cost
            + p.get(GasId::cold_storage_cost())  // sender_auth_cost (non-empty)
            + p.get(GasId::cold_storage_cost()); // payer_auth_cost (sponsored)
        assert_eq!(
            aa_intrinsic_gas(&parts, &p),
            expected,
            "pre-fork aggregator must equal base + payload + auth_costs (no XLayer components)",
        );
    }
}
