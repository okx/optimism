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

// Deterministic via `Deployers::BASE_V1_*.create(0)` — see
// `op-node/rollup/derive/native_aa_upgrade_transactions.go` for the Go
// deployer addresses (`0x4210…0008` … `0x4210…000d`) and base's
// `crates/consensus/upgrades/src/base_v1.rs::deployed_addresses_are_deterministic`
// test for the canonical asserted values.
//
// The earlier hardcoded values (e.g. `0x4F20618C…` for AccountConfig,
// `0x31914Dd8…` for DefaultAccount) appeared in spec docs / `send-aa-tx.mjs`
// fixtures but did NOT match the addresses produced by `create(deployer, 0)`
// — base's own unit tests assert the CREATE-derived ones, and the protocol
// reads/writes storage at this constant. Pre-fix, the protocol storage path
// pointed at an address with no contract code, breaking ERC-1271 and
// applySignedOwnerChanges() while masking failures via the implicit-EOA
// fallback. See test report BUG-006 for the discovery + reasoning.

/// Default account (wallet) implementation contract. Bare EOAs that submit
/// AA transactions are auto-delegated to this address via EIP-7702.
/// `BASE_V1_DEFAULT_ACCOUNT (0x4210…000d).create(0)`.
pub const DEFAULT_ACCOUNT_ADDRESS: Address = address!("0xAb4eE49EE97e49807e180BD5Fb9D9F35783b84F2");

/// Account configuration system contract.
/// Manages owner registrations, account creation, config changes, and locks.
/// `BASE_V1_ACCOUNT_CONFIGURATION (0x4210…000b).create(0)`.
pub const ACCOUNT_CONFIG_ADDRESS: Address = address!("0xf946601D5424118A4e4054BB0B13133f216b4FeE");

/// Explicit native K1/ecrecover verifier sentinel — protocol-reserved at
/// `address(1)` (the standard ECRECOVER precompile). Different from the
/// on-chain ERC-1271 K1 verifier contract at `0x5Be482Da…`; this constant
/// is only ever interpreted natively, never STATICCALL'd.
///
/// `address(0)` remains the implicit EOA mode.
pub const K1_VERIFIER_ADDRESS: Address = address!("0x0000000000000000000000000000000000000001");

/// P256 raw ECDSA verifier contract. `BASE_V1_P256_VERIFIER (0x4210…0009).create(0)`.
pub const P256_RAW_VERIFIER_ADDRESS: Address =
    address!("0x6751c7ED0C58319e75437f8E6Dafa2d7F6b8306F");

/// P256 WebAuthn verifier contract. `BASE_V1_WEBAUTHN_VERIFIER (0x4210…000a).create(0)`.
pub const P256_WEBAUTHN_VERIFIER_ADDRESS: Address =
    address!("0x3572bb3F611a40DDcA70e5b55Cc797D58357AD44");

/// Delegate verifier contract (1-hop delegation).
/// `BASE_V1_DELEGATE_VERIFIER (0x4210…000c).create(0)`.
pub const DELEGATE_VERIFIER_ADDRESS: Address =
    address!("0xc758A89C53542164aaB7f6439e8c8cAcf628fF62");

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
