//! EIP-8130 (`XLayer` AA, tx-type 0x7B) EL-side machinery.
//!
//! This module is the home for EIP-8130 work that needs cryptography (K1 /
//! P256 / SHA-256) and tx-conversion glue. Wire-format types
//! (`TxEip8130`, `sender_signature_hash`, etc.) stay in `op-alloy-consensus`;
//! the per-component intrinsic gas computation lives in
//! [`op_revm::eip8130_gas`] (computed on-demand by the handler from
//! `cfg.gas_params()`); everything else — native verifier dispatch,
//! [`auth_state`] resolution, and the [`parts`] builder used by `tx.rs` —
//! lives here so the consensus crate stays free of crypto deps.
//!
//! Layout:
//! - [`native_verifier`]: dispatch for the four EIP-8130 native verifier addresses (K1, P256-raw,
//!   P256-WebAuthn, Delegate).
//! - [`auth_state`]: builders that turn `tx.sender_auth` / `tx.payer_auth` into the
//!   [`op_revm::transaction::eip8130::AuthState`] enum the handler matches on.
//! - [`address`]: CREATE2 address derivation for Create entries.
//! - [`storage`]: storage-slot derivation for `AccountConfiguration` and `NonceManager`.
//! - [`parts`]: the [`op_revm::transaction::eip8130::Eip8130Parts`] builder, called from
//!   `crate::tx::eip8130_parts`.

pub mod address;
pub mod auth_state;
pub mod native_verifier;
pub mod parts;
pub mod storage;

pub use address::derive_account_address;
pub use auth_state::{
    build_payer_auth_state, build_sender_auth_state, build_sender_auth_state_with_recovered,
};
pub use native_verifier::{NativeVerifier, NativeVerifyResult, try_native_verify};
pub use parts::{
    account_change_units, eip8130_parts, eip8130_parts_with_auth_states,
    eip8130_parts_with_nonce_free_hash,
};
pub use storage::{account_state_slot, encode_owner_config, owner_config_slot};
