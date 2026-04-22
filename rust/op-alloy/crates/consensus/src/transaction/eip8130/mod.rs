//! EIP-8130 account-abstraction transaction — wire-level types.
//!
//! Scope: the literal wire struct [`TxEip8130`] + its RLP / 2718
//! codec, the `sender_auth` / `payer_auth` shape, verifier
//! classification, and native-verifier ecrecover helpers. Nothing
//! in this module reaches into the EVM or the executor — the
//! "wire → exec" bridge lives downstream in the consumer (see
//! `xlayer-consensus`'s `build` + `gas_schedule` modules, which
//! consume [`TxEip8130`] and produce revm-ready gas / parts).
//!
//! # Why this lives in op-alloy (and not downstream)
//!
//! Because [`crate::OpTxEnvelope`] carries an [`OpTxEnvelope::Eip8130`]
//! variant — Base ship the same EIP in the same envelope slot —
//! [`TxEip8130`] has to live where [`OpTxEnvelope`] lives. Moving
//! the wire struct any further upstream of op-alloy would force a
//! circular dependency; any further downstream, and we'd need an
//! extra newtype wrapper just to satisfy the orphan rule on
//! `From<Signed<TxEip8130>> for OpTxEnvelope`.
//!
//! # Minimal-footprint design
//!
//! Every addition an L2 integrator has to make when they rebase
//! against a future upstream op-alloy is confined to:
//!
//! - this whole directory (8 standalone files, none shared with
//!   existing op-alloy code),
//! - one `pub mod eip8130;` line in [`crate::transaction`], and
//! - one variant + its `From`/`match` arms in each of:
//!   - [`OpTxEnvelope`] (envelope.rs)
//!   - [`OpPooledTransaction`] (pooled.rs)
//!   - [`OpReceiptEnvelope`] / [`OpReceipt`] (receipts/)
//!
//! The variants are the only invasive patches; everything else is
//! add-only. Rebase conflict surface is minimised by design.

mod address;
mod constants;
mod encoding;
mod envelope_ext;
mod native;
mod signature;
mod tx;
mod types;
mod verifier;

pub use envelope_ext::OpEip8130Transaction;
#[cfg(feature = "k256")]
pub(crate) use envelope_ext::recover_eip8130_signer;

pub use address::{
    create2_address, deployment_code, deployment_header, derive_account_address, effective_salt,
};
pub use constants::{
    AA_BASE_COST, AA_PAYER_TYPE, AA_TX_TYPE_ID, BYTECODE_BASE_GAS, BYTECODE_PER_BYTE_GAS,
    CONFIG_CHANGE_OP_GAS, CONFIG_CHANGE_SKIP_GAS, CUSTOM_VERIFIER_GAS_CAP, DEPLOYMENT_HEADER_SIZE,
    EOA_AUTH_GAS, MAX_ACCOUNT_CHANGES_PER_TX, MAX_CALLS_PER_TX, MAX_CONFIG_OPS_PER_TX,
    MAX_SIGNATURE_SIZE, NONCE_KEY_COLD_GAS, NONCE_KEY_MAX, NONCE_KEY_WARM_GAS, SLOAD_GAS,
};
pub use native::{
    delegate_recover, k1_owner_id, k1_recover, native_verify, p256_raw_recover,
    p256_webauthn_recover, NativeVerifyError, NativeVerifyResult,
};
pub use signature::{
    config_change_digest, parse_sender_auth, payer_signature_hash, sender_signature_hash,
    ParsedSenderAuth,
};
pub use tx::TxEip8130;
pub use types::{
    AccountChangeEntry, Call, ConfigChangeEntry, CreateEntry, DelegationEntry, Owner, OwnerChange,
    OwnerScope, CHANGE_TYPE_CONFIG, CHANGE_TYPE_CREATE, CHANGE_TYPE_DELEGATION, OP_AUTHORIZE_OWNER,
    OP_REVOKE_OWNER,
};
pub use verifier::{
    auth_verifier_kind, verifier_kind, NativeVerifier, VerifierKind, DELEGATE_VERIFIER_ADDRESS,
    K1_VERIFIER_ADDRESS, P256_RAW_VERIFIER_ADDRESS, P256_WEBAUTHN_VERIFIER_ADDRESS,
};
