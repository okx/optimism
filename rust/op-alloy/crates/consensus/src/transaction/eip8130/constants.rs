//! EIP-8130 constants and verifier gas costs.

use alloy_primitives::{Address, U256};

use super::verifier::NativeVerifier;

/// The EIP-2718 transaction type byte for AA transactions.
pub const AA_TX_TYPE_ID: u8 = 0x7B;

/// Payer signature domain separator byte. Ensures cryptographic domain separation
/// so a valid sender signature cannot be replayed as a payer signature.
pub const AA_PAYER_TYPE: u8 = 0x7C;

/// Base intrinsic gas cost for an AA transaction (replaces the standard 21 000).
pub const AA_BASE_COST: u64 = 15_000;

/// Size in bytes of the EVM deployment header prepended to bytecode during CREATE2.
pub const DEPLOYMENT_HEADER_SIZE: usize = 14;

/// Maximum allowed size of `sender_auth` or `payer_auth` to bound DoS surface.
pub const MAX_SIGNATURE_SIZE: usize = 2048;

// ---------------------------------------------------------------------------
// Intrinsic gas sub-components
// ---------------------------------------------------------------------------

/// Gas charged when the `nonce_key` channel has not been used before (cold SSTORE).
pub const NONCE_KEY_COLD_GAS: u64 = 22_100;

/// Gas charged for an existing (warm) `nonce_key` channel.
pub const NONCE_KEY_WARM_GAS: u64 = 5_000;

/// Base gas for a CREATE2 deployment triggered by a create entry.
pub const BYTECODE_BASE_GAS: u64 = 32_000;

/// Per-byte gas for deployed bytecode.
pub const BYTECODE_PER_BYTE_GAS: u64 = 200;

/// Gas for each applied account-change unit (SSTORE).
///
/// Account-change units are:
/// - each config operation in a matching-chain config change,
/// - each create entry (1 unit),
/// - each initial owner in a create entry.
pub const CONFIG_CHANGE_OP_GAS: u64 = 20_000;

/// Gas for each config change entry that is skipped (wrong chain, SLOAD only).
pub const CONFIG_CHANGE_SKIP_GAS: u64 = 2_100;

/// Cost of a single SLOAD during auth resolution.
pub const SLOAD_GAS: u64 = 2_100;

/// Flat gas cost for EOA (ecrecover) authentication.
pub const EOA_AUTH_GAS: u64 = 6_000;

/// Maximum number of calls across all `calls` phases in one transaction.
///
/// Bounds AA execution fanout and prevents oversized phased call graphs from
/// creating disproportionate mempool/inclusion validation work.
pub const MAX_CALLS_PER_TX: usize = 100;

/// Maximum number of account-change units in one transaction.
///
/// Counting rules:
/// - each create entry counts as 1,
/// - each create entry initial owner counts as 1,
/// - each config operation counts as 1.
pub const MAX_ACCOUNT_CHANGES_PER_TX: usize = 10;

/// Maximum number of total `OwnerChange`s across all `ConfigChangeEntry`s
/// in a single transaction. Bounds the DoS surface of owner change validation.
pub const MAX_CONFIG_OPS_PER_TX: usize = 5;

/// Maximum gas allowed for a custom verifier STATICCALL.
///
/// Custom verifiers are metered via an on-chain STATICCALL whose gas is
/// charged to the payer separately from the sender's `gas_limit` (which is
/// execution-only). This cap bounds the DoS surface of arbitrary verifier
/// contracts.
pub const CUSTOM_VERIFIER_GAS_CAP: u64 = 200_000;

/// Maximum nonce key value (`2^256 - 1`), enabling nonce-free mode.
///
/// When `nonce_key == NONCE_KEY_MAX`, the protocol enters nonce-free mode:
/// no nonce state is read or incremented. `nonce_sequence` must be `0` and
/// `expiry` must be non-zero. Replay protection relies on short-lived
/// expiry windows and transaction hash deduplication.
///
/// Nodes should reject nonce-free transactions whose `expiry` exceeds a
/// short window (e.g. 30 seconds from the current timestamp).
pub const NONCE_KEY_MAX: U256 = U256::MAX;

/// Maximum allowed expiry window (in seconds) for nonce-free transactions.
///
/// The mempool rejects nonce-free transactions whose `expiry` is more than
/// this many seconds into the future, bounding the replay-protection window.
pub const NONCE_FREE_MAX_EXPIRY_WINDOW: u64 = 30;

/// Capacity of the expiring-nonce circular buffer.
///
/// Sized for 10 000 TPS × 30-second window = 300 000 entries. Entries are
/// evicted once the pointer wraps and the old entry has expired.
pub const EXPIRING_NONCE_SET_CAPACITY: u32 = 300_000;

/// Intrinsic gas charged for expiring-nonce (nonce-free) transactions.
///
/// Covers the on-chain circular-buffer replay protection state operations:
/// 2 cold SLOADs + 1 warm SLOAD + 3 warm SSTORE resets = 14 000.
pub const EXPIRING_NONCE_GAS: u64 = 14_000;

// ---------------------------------------------------------------------------
// Verifier gas cost table
// ---------------------------------------------------------------------------

/// Configurable gas costs for native signature verification.
///
/// Each native verifier address has a fixed gas charge included in intrinsic
/// gas. These costs account for the CPU work performed by native (Rust)
/// verifiers that would otherwise be "free" relative to on-chain
/// STATICCALL verification.
///
/// `DELEGATE` acts as a 1-hop indirection: its cost is additive with the
/// cost of the target verifier it resolves to. For example, a delegate
/// wrapping K1 costs `DELEGATE + K1 = 3_000 + 6_000 = 9_000`.
///
/// Custom verifiers (non-native addresses) return 0 here because their gas
/// is metered at runtime via STATICCALL, capped at [`CUSTOM_VERIFIER_GAS_CAP`],
/// and charged to the payer separately from the sender's execution `gas_limit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifierGasCosts {
    /// secp256k1 ECDSA recovery.
    pub k1: u64,
    /// secp256r1 / P-256 raw ECDSA verification.
    pub p256_raw: u64,
    /// secp256r1 / P-256 WebAuthn assertion verification.
    pub p256_webauthn: u64,
    /// Delegate hop overhead (added to the target verifier's cost).
    pub delegate: u64,
}

impl VerifierGasCosts {
    /// Default gas costs for BASE_V1.
    pub const BASE_V1: Self =
        Self { k1: 6_000, p256_raw: 9_500, p256_webauthn: 15_000, delegate: 3_000 };

    /// Returns the verification gas for a native verifier.
    pub fn gas_for_native_verifier(
        &self,
        verifier: NativeVerifier,
        inner_verifier: Option<NativeVerifier>,
    ) -> u64 {
        match verifier {
            NativeVerifier::K1 => self.k1,
            NativeVerifier::P256Raw => self.p256_raw,
            NativeVerifier::P256WebAuthn => self.p256_webauthn,
            NativeVerifier::Delegate => {
                self.delegate
                    + inner_verifier
                        .map(|inner| self.gas_for_native_verifier(inner, None))
                        .unwrap_or(0)
            }
        }
    }

    /// Returns the verification gas for a given verifier address.
    ///
    /// - For native verifiers (K1, P256_RAW, P256_WEBAUTHN): returns the flat
    ///   cost.
    /// - For DELEGATE: returns `delegate + inner_verifier_cost`. The
    ///   `inner_verifier` must be provided by the caller after resolving the
    ///   delegation target. If the inner verifier is unknown or custom, only
    ///   the delegate overhead is returned.
    /// - For custom verifiers (any other address): returns 0 (metered at
    ///   runtime via STATICCALL).
    pub fn gas_for_verifier(&self, verifier: Address, inner_verifier: Option<Address>) -> u64 {
        match NativeVerifier::from_address(verifier) {
            Some(native) => self.gas_for_native_verifier(
                native,
                inner_verifier.and_then(NativeVerifier::from_address),
            ),
            None => 0,
        }
    }
}

impl Default for VerifierGasCosts {
    fn default() -> Self {
        Self::BASE_V1
    }
}
