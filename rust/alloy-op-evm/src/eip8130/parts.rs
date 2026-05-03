//! Builds [`Eip8130Parts`] for the handler from a [`TxEip8130`] + caller.
//!
//! Called once per tx-bytes-decoding from `crate::tx::eip8130_parts`.
//! Symmetric on sequencer + validator: both run the same auth resolution, so
//! a sequencer can't ship a tx the validator would later reject for a forged
//! signature — the [`AuthState::Invalid`] verdict happens once, here, and
//! `validate_env` rejects on it.
//!
//! Intrinsic gas is **not** computed here. The handler computes
//! `aa_intrinsic_gas` / `payer_intrinsic_gas` on demand via
//! [`op_revm::eip8130_gas`] using the active fork's
//! [`revm::context_interface::cfg::GasParams`] from `cfg.gas_params()`. The
//! cap on aggregate custom-verifier gas is similarly fork-bound and read
//! from [`op_revm::constants::XLAYER_AA_CUSTOM_VERIFIER_GAS_CAP`] at the
//! call site.
//!
//! Fields not populated here (account-change parsing, code placements,
//! pre-writes, config writes, sequence updates, authorizer validations,
//! account-creation logs, config-change logs, auto-delegation code,
//! nonce-free hash) are slice 4-6 work and remain `Default::default()`.

use alloy_primitives::{Address, U256};
use op_alloy::consensus::TxEip8130;
use op_revm::{
    eip8130_gas::calldata_cost,
    transaction::eip8130::{Eip8130Call, Eip8130Parts},
};

use super::auth_state::{build_payer_auth_state, build_sender_auth_state};

/// Builds [`Eip8130Parts`] for a tx, given the caller (effective sender) address.
///
/// `caller` comes from upstream sender recovery (`recover_eip8130_sender` for
/// EOA-mode txs; `tx.from` otherwise) and is used to fill `parts.sender`.
///
/// Precomputes the gas-path inputs the handler later combines with the active
/// fork's [`revm::context_interface::cfg::GasParams`]:
///
/// - `sender_payload_calldata_cost` — EIP-2028 cost of
///   `tx.encoded_for_sender_signing()` (RLP-fixed once the tx is sealed,
///   so a constant input to the per-fork formula).
/// - `sender_auth` / `payer_auth` — raw blobs whose 20-byte verifier prefix
///   the gas path uses to pick K1 / P256Raw / WebAuthn / Delegate-outer
///   pricing.
/// - `is_eoa` — `tx.is_eoa()` (`from.is_none()`); forces K1 cost on the
///   bare-65-byte-sig path.
///
/// With these captured the underlying `TxEip8130` is no longer needed by
/// the gas path, so we don't have to thread `eip8130_tx: Option<TxEip8130>`
/// through `OpTransaction` / `OpTxTr`.
pub fn eip8130_parts(tx: &TxEip8130, caller: Address) -> Eip8130Parts {
    let sender_authstate = build_sender_auth_state(tx);
    let payer_authstate = build_payer_auth_state(tx);

    Eip8130Parts {
        expiry: tx.expiry,
        sender: caller,
        payer: tx.payer.unwrap_or(caller),
        sender_authstate,
        payer_authstate,
        nonce_key: tx.nonce_key,
        sender_payload_calldata_cost: calldata_cost(&tx.encoded_for_sender_signing()),
        is_eoa: tx.is_eoa(),
        sender_auth: tx.sender_auth.clone(),
        payer_auth: tx.payer_auth.clone(),
        call_phases: tx
            .calls
            .iter()
            .map(|phase| {
                phase
                    .iter()
                    .map(|call| Eip8130Call {
                        to: call.to,
                        data: call.data.clone(),
                        value: U256::ZERO,
                    })
                    .collect()
            })
            .collect(),
        account_change_units: tx.account_changes.len(),
        ..Default::default()
    }
}
