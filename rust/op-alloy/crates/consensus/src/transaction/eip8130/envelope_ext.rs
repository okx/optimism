//! Envelope-side helpers for [`crate::OpTxEnvelope`]'s `Eip8130`
//! variant.
//!
//! Everything here is **add-only** — the patch to `envelope.rs`
//! itself is limited to the variant declaration plus the arm
//! fixups that `match`-exhaustiveness forces. Anything that could
//! in principle live outside the enum definition (downstream
//! trait impls, signer recovery helper, conversions) lives here
//! instead, to keep `envelope.rs`'s diff against upstream small
//! and keep future rebases mechanical.

use alloy_consensus::Sealed;
use alloy_primitives::Address;

use super::TxEip8130;
use crate::OpTxEnvelope;

/// Trait bolted onto [`OpTxEnvelope`] to surface the `Eip8130`
/// variant without making the enum's inherent methods grow.
///
/// Shape mirrors Base's `OpEip8130Transaction` (same method names,
/// same signatures) so AA tooling built against Base compiles
/// here unchanged.
pub trait OpEip8130Transaction {
    /// Returns `true` if this envelope carries a `TxEip8130`.
    fn is_eip8130(&self) -> bool;

    /// Returns the inner [`TxEip8130`] if this is an AA transaction.
    fn as_eip8130(&self) -> Option<&Sealed<TxEip8130>>;
}

impl OpEip8130Transaction for OpTxEnvelope {
    #[inline]
    fn is_eip8130(&self) -> bool {
        matches!(self, Self::Eip8130(_))
    }

    #[inline]
    fn as_eip8130(&self) -> Option<&Sealed<TxEip8130>> {
        match self {
            Self::Eip8130(tx) => Some(tx),
            _ => None,
        }
    }
}

/// Recovers the sender address from an EIP-8130 transaction.
///
/// - **Configured owner** (`from` is `Some`): the sender is the
///   declared `from` field — no ecrecover is performed. The
///   `sender_auth` blob is validated for owner-set membership at
///   execution time, not at the signer-recovery boundary.
/// - **EOA mode** (`from` is `None`): ecrecovers the sender from
///   the 65-byte K1 ECDSA signature in `sender_auth` over
///   [`sender_signature_hash`].
///
/// `pub(crate)` so [`crate::transaction::envelope`]'s
/// [`SignerRecoverable`] impl can delegate here in a one-line
/// match arm — keeps the envelope.rs patch minimal.
///
/// [`sender_signature_hash`]: super::sender_signature_hash
/// [`SignerRecoverable`]: alloy_consensus::transaction::SignerRecoverable
#[cfg(feature = "k256")]
pub(crate) fn recover_eip8130_signer(
    tx: &TxEip8130,
) -> Result<Address, alloy_consensus::crypto::RecoveryError> {
    use alloy_consensus::crypto::RecoveryError;
    use alloy_primitives::{Signature, U256};

    // Configured-account AA tx: the sender is the declared `from`.
    if !tx.is_eoa() {
        return tx.from.ok_or_else(RecoveryError::new);
    }

    // EOA-mode: sender_auth is exactly 65 bytes `r ‖ s ‖ v`.
    if tx.sender_auth.len() != 65 {
        return Err(RecoveryError::new());
    }

    let sig_hash = super::sender_signature_hash(tx);
    let v = tx.sender_auth[64];
    // Accept both legacy (27/28) and typed-tx (0/1) parity bytes —
    // same convention as alloy's generic recover. EIP-8130 is a
    // typed tx, so clients will typically send 0/1, but tooling
    // that recycles an EIP-155 signing path may send 27/28.
    let parity = match v {
        0 | 27 => false,
        1 | 28 => true,
        _ => return Err(RecoveryError::new()),
    };
    let signature = Signature::new(
        U256::from_be_slice(&tx.sender_auth[..32]),
        U256::from_be_slice(&tx.sender_auth[32..64]),
        parity,
    );
    alloy_consensus::crypto::secp256k1::recover_signer(&signature, sig_hash)
}

/// Ergonomic conversion so downstream callers can go from a bare
/// [`TxEip8130`] straight to [`OpTxEnvelope`] without the explicit
/// `Sealed::new` dance. Sibling to `impl From<TxDeposit> for
/// OpTxEnvelope` in `envelope.rs`.
impl From<TxEip8130> for OpTxEnvelope {
    fn from(v: TxEip8130) -> Self {
        use alloy_consensus::Sealable;
        Self::Eip8130(v.seal_slow())
    }
}

impl From<Sealed<TxEip8130>> for OpTxEnvelope {
    fn from(v: Sealed<TxEip8130>) -> Self {
        Self::Eip8130(v)
    }
}
