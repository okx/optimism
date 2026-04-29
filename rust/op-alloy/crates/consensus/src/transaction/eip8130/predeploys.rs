//! Addresses for EIP-8130 system contracts and precompiles.
//!
//! # Deployment model
//!
//! **Precompiles** (native code, no EVM bytecode, fixed addresses):
//!   - `NonceManager` (`0x…aa02`)  — 2D nonce reads
//!   - `TxContext`     (`0x…aa03`)  — AA transaction metadata
//!
//! **Deployed contracts** (Solidity, deployed at BASE_V1 activation via
//! `TxDeposit` upgrade transactions — see `base_consensus_upgrades::BaseV1`):
//!   - `AccountConfiguration` — owner registrations, account creation, locks
//!   - `P256Verifier`, `WebAuthnVerifier`, `DelegateVerifier`
//!   - `DefaultAccount` — wallet implementation for EIP-7702 auto-delegation
//!
//! All deployed contract addresses are deterministic: `Deployers::BASE_V1_*.create(0)`.
//! On devnets with BASE_V1 active from genesis, the derivation pipeline injects
//! the upgrade deposit transactions at block 0.

use alloy_primitives::{Address, address};
use core::sync::atomic::{AtomicBool, Ordering};

use super::verifier::NativeVerifier;

/// Sentinel verifier address written on self-ownerId revocation.
///
/// When the implicit EOA owner (`ownerId == bytes32(bytes20(account))`) is
/// revoked, the contract writes
/// `OwnerConfig{verifier: address(type(uint160).max), scopes: 0}`
/// instead of deleting the slot. This prevents the protocol's implicit EOA
/// rule from re-authorizing the account on an empty slot. Non-self owners
/// are simply deleted back to `address(0)`.
///
/// Storage interpretation:
///   - `verifier == address(0)` → empty slot (implicit EOA rule may apply)
///   - `verifier == address(1)` → explicit native K1/ecrecover verifier
///   - `verifier == address(type(uint160).max)` → explicitly revoked sentinel
///   - `verifier` in `[2..max-1]` → registered custom verifier contract
pub const REVOKED_VERIFIER: Address = address!("0xffffffffffffffffffffffffffffffffffffffff");

// ── AccountConfiguration deployment cache ─────────────────────────
//
// The AccountConfiguration contract is deployed via CREATE2 (not a
// precompile). Before it has real bytecode, storage reads return zeros
// and the implicit EOA rule handles sender/payer authorization. Config
// changes must be rejected until the contract is deployed.
//
// This flag is monotonic: once set to `true` it never reverts to `false`.
// A stale `false` just triggers one extra DB code-existence check.

static ACCOUNT_CONFIG_DEPLOYED: AtomicBool = AtomicBool::new(false);

/// Returns `true` if AccountConfiguration has been detected as deployed.
///
/// Callers should fall back to a DB code check when this returns `false`,
/// then call [`mark_account_config_deployed`] on a positive result.
pub fn is_account_config_known_deployed() -> bool {
    ACCOUNT_CONFIG_DEPLOYED.load(Ordering::Relaxed)
}

/// Records that AccountConfiguration has real bytecode. Future calls to
/// [`is_account_config_known_deployed`] return `true` without a DB lookup.
pub fn mark_account_config_deployed() {
    ACCOUNT_CONFIG_DEPLOYED.store(true, Ordering::Relaxed);
}

// ── Precompiles (native, fixed addresses) ─────────────────────────

/// Nonce Manager precompile. Read-only 2D nonce access; writes are
/// protocol-only (handler pre-execution storage writes).
pub const NONCE_MANAGER_ADDRESS: Address = address!("0x000000000000000000000000000000000000aa02");

/// Transaction context precompile. Exposes the current AA transaction's
/// `owner_id`, phase index, and call metadata during execution.
pub const TX_CONTEXT_ADDRESS: Address = address!("0x000000000000000000000000000000000000aa03");

// ── Deployed contracts (TxDeposit at BASE_V1 activation) ─────────
//
// All addresses are deterministic: `Deployers::BASE_V1_*.create(0)`.
// See `crates/consensus/upgrades/src/base_v1.rs` for the deposit
// transactions that deploy these contracts.
//
// On devnets where BASE_V1 is active from genesis, these are deployed
// by the derivation pipeline's upgrade transactions at block 0.

/// Default account (wallet) implementation contract. Bare EOAs that submit
/// AA transactions are auto-delegated to this address via EIP-7702.
pub const DEFAULT_ACCOUNT_ADDRESS: Address = address!("0x31914Dd8C3901448D787b2097744Bf7D3241E85A");

/// Account configuration system contract.
/// Manages owner registrations, account creation, config changes, and locks.
pub const ACCOUNT_CONFIG_ADDRESS: Address = address!("0x4F20618Cf5c160e7AA385268721dA968F86F0e61");

/// Explicit native K1/ecrecover verifier sentinel.
///
/// `address(0)` remains the implicit EOA mode.
pub const K1_VERIFIER_ADDRESS: Address = address!("0x0000000000000000000000000000000000000001");

/// P256 raw ECDSA verifier contract.
pub const P256_RAW_VERIFIER_ADDRESS: Address =
    address!("0x75E9779603e826f2D8d4dD7Edee3F0a737e4228d");

/// P256 WebAuthn verifier contract.
pub const P256_WEBAUTHN_VERIFIER_ADDRESS: Address =
    address!("0xb2c8b7ec119882fBcc32FDe1be1341e19a5Bd53E");

/// Delegate verifier contract (1-hop delegation).
pub const DELEGATE_VERIFIER_ADDRESS: Address =
    address!("0x30A76831b27732087561372f6a1bef6Fc391d805");

/// Default high-rate account variant. Blocks outbound ETH value transfers
/// when locked, enabling higher mempool rate limits.
pub const DEFAULT_HIGH_RATE_ACCOUNT_ADDRESS: Address =
    address!("0x42Ebc02d3D7aaff19226D96F83C376B304BD25Cf");

/// Sentinel verifier address for external caller authorization in
/// `DefaultAccount`. Deterministic: `address(uint160(uint256(keccak256("externalCaller"))))`.
/// No contract exists at this address; registered as a verifier to mark
/// EntryPoints, PolicyManagers, and other authorized external callers.
pub const EXTERNAL_CALLER_VERIFIER: Address =
    address!("0x345249274ee98994abbf79ef955319e4cb3f6849");

/// Returns `true` if the given address is a known native verifier
/// (K1, P256 raw, P256 WebAuthn, or Delegate).
pub fn is_native_verifier(addr: Address) -> bool {
    NativeVerifier::from_address(addr).is_some()
}
