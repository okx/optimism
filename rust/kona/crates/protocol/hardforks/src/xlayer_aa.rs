//! Module containing a [`TxDeposit`] builder for the XLayerAA (EIP-8130) network upgrade
//! transactions.
//!
//! XLayerAA is an XLayer-specific hardfork. On activation, 7 deposit transactions deploy
//! the EIP-8130 system contracts at canonical `CREATE(deployer, 0)` addresses. The deployer
//! addresses are fresh accounts (nonce 0 at activation), guaranteeing deterministic predeploy
//! addresses across devnet / testnet / mainnet.

use alloc::{string::String, vec::Vec};
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{Address, Bytes, TxKind, U256, address, hex};
use op_alloy_consensus::{TxDeposit, UpgradeDepositSource};

use crate::Hardfork;

/// Fork name used to namespace intent strings for source-hash derivation.
///
/// This must match the `forkName` constant in the Go op-node counterpart
/// (`xlayer_aa_upgrade_transactions.go`) so that source hashes are identical.
const FORK_NAME: &str = "xlayer_aa";

/// The XLayerAA (EIP-8130) network upgrade transactions.
#[derive(Debug, Default, Clone, Copy)]
pub struct XLayerAA;

impl XLayerAA {
    // ── Deployer addresses (nonce-0 CREATE → deterministic predeploy) ──────────────

    /// AccountConfiguration deployer address.
    pub const ACCOUNT_CONFIG_DEPLOYER: Address =
        address!("4210000000000000000000000000000000000010");
    /// DefaultAccount deployer address.
    pub const DEFAULT_ACCOUNT_DEPLOYER: Address =
        address!("4210000000000000000000000000000000000011");
    /// DefaultHighRateAccount deployer address.
    pub const DEFAULT_HIGH_RATE_ACCOUNT_DEPLOYER: Address =
        address!("4210000000000000000000000000000000000012");
    /// K1Verifier deployer address.
    pub const K1_VERIFIER_DEPLOYER: Address = address!("4210000000000000000000000000000000000013");
    /// P256Verifier deployer address.
    pub const P256_VERIFIER_DEPLOYER: Address =
        address!("4210000000000000000000000000000000000014");
    /// WebAuthnVerifier deployer address.
    pub const WEBAUTHN_VERIFIER_DEPLOYER: Address =
        address!("4210000000000000000000000000000000000015");
    /// DelegateVerifier deployer address.
    pub const DELEGATE_VERIFIER_DEPLOYER: Address =
        address!("4210000000000000000000000000000000000016");

    // ── Gas limits (per NUT bundle JSON) ───────────────────────────────────────────

    /// Gas limit for AccountConfiguration deployment.
    pub const ACCOUNT_CONFIG_GAS: u64 = 2_500_000;
    /// Gas limit for DefaultAccount deployment.
    pub const DEFAULT_ACCOUNT_GAS: u64 = 1_000_000;
    /// Gas limit for DefaultHighRateAccount deployment.
    pub const DEFAULT_HIGH_RATE_ACCOUNT_GAS: u64 = 1_000_000;
    /// Gas limit for K1Verifier deployment.
    pub const K1_VERIFIER_GAS: u64 = 750_000;
    /// Gas limit for P256Verifier deployment.
    pub const P256_VERIFIER_GAS: u64 = 1_500_000;
    /// Gas limit for WebAuthnVerifier deployment.
    pub const WEBAUTHN_VERIFIER_GAS: u64 = 2_000_000;
    /// Gas limit for DelegateVerifier deployment.
    pub const DELEGATE_VERIFIER_GAS: u64 = 750_000;

    // ── Source-hash helpers ────────────────────────────────────────────────────────

    fn source_hash(index: usize, base_intent: &str) -> alloy_primitives::B256 {
        let intent = alloc::format!("{} {}: {}", FORK_NAME, index, base_intent);
        UpgradeDepositSource { intent: String::from(intent) }.source_hash()
    }

    /// Source hash for AccountConfiguration deployment.
    pub fn account_config_source() -> alloy_primitives::B256 {
        Self::source_hash(0, "XLayerAA: AccountConfiguration Deployment")
    }

    /// Source hash for DefaultAccount deployment.
    pub fn default_account_source() -> alloy_primitives::B256 {
        Self::source_hash(1, "XLayerAA: DefaultAccount Deployment")
    }

    /// Source hash for DefaultHighRateAccount deployment.
    pub fn default_high_rate_account_source() -> alloy_primitives::B256 {
        Self::source_hash(2, "XLayerAA: DefaultHighRateAccount Deployment")
    }

    /// Source hash for K1Verifier deployment.
    pub fn k1_verifier_source() -> alloy_primitives::B256 {
        Self::source_hash(3, "XLayerAA: K1Verifier Deployment")
    }

    /// Source hash for P256Verifier deployment.
    pub fn p256_verifier_source() -> alloy_primitives::B256 {
        Self::source_hash(4, "XLayerAA: P256Verifier Deployment")
    }

    /// Source hash for WebAuthnVerifier deployment.
    pub fn webauthn_verifier_source() -> alloy_primitives::B256 {
        Self::source_hash(5, "XLayerAA: WebAuthnVerifier Deployment")
    }

    /// Source hash for DelegateVerifier deployment.
    pub fn delegate_verifier_source() -> alloy_primitives::B256 {
        Self::source_hash(6, "XLayerAA: DelegateVerifier Deployment")
    }

    // ── Bytecode helpers ───────────────────────────────────────────────────────────

    fn decode_hex(s: &str) -> Bytes {
        hex::decode(s.replace('\n', "")).expect("valid hex bytecode").into()
    }

    /// Returns the AccountConfiguration deployment bytecode.
    pub fn account_config_bytecode() -> Bytes {
        Self::decode_hex(include_str!("./bytecode/xlayer_aa_account_config.hex"))
    }

    /// Returns the DefaultAccount deployment bytecode.
    pub fn default_account_bytecode() -> Bytes {
        Self::decode_hex(include_str!("./bytecode/xlayer_aa_default_account.hex"))
    }

    /// Returns the DefaultHighRateAccount deployment bytecode.
    pub fn default_high_rate_account_bytecode() -> Bytes {
        Self::decode_hex(include_str!("./bytecode/xlayer_aa_default_high_rate_account.hex"))
    }

    /// Returns the K1Verifier deployment bytecode.
    pub fn k1_verifier_bytecode() -> Bytes {
        Self::decode_hex(include_str!("./bytecode/xlayer_aa_k1_verifier.hex"))
    }

    /// Returns the P256Verifier deployment bytecode.
    pub fn p256_verifier_bytecode() -> Bytes {
        Self::decode_hex(include_str!("./bytecode/xlayer_aa_p256_verifier.hex"))
    }

    /// Returns the WebAuthnVerifier deployment bytecode.
    pub fn webauthn_verifier_bytecode() -> Bytes {
        Self::decode_hex(include_str!("./bytecode/xlayer_aa_webauthn_verifier.hex"))
    }

    /// Returns the DelegateVerifier deployment bytecode.
    pub fn delegate_verifier_bytecode() -> Bytes {
        Self::decode_hex(include_str!("./bytecode/xlayer_aa_delegate_verifier.hex"))
    }

    // ── Deposit builder ────────────────────────────────────────────────────────────

    /// Returns the 7 [`TxDeposit`]s for the XLayerAA network upgrade.
    pub fn deposits() -> impl Iterator<Item = TxDeposit> {
        let mk = |source_hash, from, gas_limit, input| TxDeposit {
            source_hash,
            from,
            to: TxKind::Create,
            mint: 0,
            value: U256::ZERO,
            gas_limit,
            is_system_transaction: false,
            input,
        };
        [
            mk(
                Self::account_config_source(),
                Self::ACCOUNT_CONFIG_DEPLOYER,
                Self::ACCOUNT_CONFIG_GAS,
                Self::account_config_bytecode(),
            ),
            mk(
                Self::default_account_source(),
                Self::DEFAULT_ACCOUNT_DEPLOYER,
                Self::DEFAULT_ACCOUNT_GAS,
                Self::default_account_bytecode(),
            ),
            mk(
                Self::default_high_rate_account_source(),
                Self::DEFAULT_HIGH_RATE_ACCOUNT_DEPLOYER,
                Self::DEFAULT_HIGH_RATE_ACCOUNT_GAS,
                Self::default_high_rate_account_bytecode(),
            ),
            mk(
                Self::k1_verifier_source(),
                Self::K1_VERIFIER_DEPLOYER,
                Self::K1_VERIFIER_GAS,
                Self::k1_verifier_bytecode(),
            ),
            mk(
                Self::p256_verifier_source(),
                Self::P256_VERIFIER_DEPLOYER,
                Self::P256_VERIFIER_GAS,
                Self::p256_verifier_bytecode(),
            ),
            mk(
                Self::webauthn_verifier_source(),
                Self::WEBAUTHN_VERIFIER_DEPLOYER,
                Self::WEBAUTHN_VERIFIER_GAS,
                Self::webauthn_verifier_bytecode(),
            ),
            mk(
                Self::delegate_verifier_source(),
                Self::DELEGATE_VERIFIER_DEPLOYER,
                Self::DELEGATE_VERIFIER_GAS,
                Self::delegate_verifier_bytecode(),
            ),
        ]
        .into_iter()
    }
}

impl Hardfork for XLayerAA {
    /// Constructs the 7 XLayerAA network upgrade transactions.
    fn txs(&self) -> impl Iterator<Item = Bytes> + '_ {
        Self::deposits().map(|tx| {
            let mut encoded = Vec::new();
            tx.encode_2718(&mut encoded);
            Bytes::from(encoded)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn test_xlayer_aa_tx_count() {
        let txs: Vec<_> = XLayerAA.txs().collect();
        assert_eq!(txs.len(), 7);
    }

    #[test]
    fn test_xlayer_aa_source_hashes_are_unique() {
        let sources = [
            XLayerAA::account_config_source(),
            XLayerAA::default_account_source(),
            XLayerAA::default_high_rate_account_source(),
            XLayerAA::k1_verifier_source(),
            XLayerAA::p256_verifier_source(),
            XLayerAA::webauthn_verifier_source(),
            XLayerAA::delegate_verifier_source(),
        ];
        for i in 0..sources.len() {
            for j in (i + 1)..sources.len() {
                assert_ne!(sources[i], sources[j], "source hashes [{i}] and [{j}] collide");
            }
        }
    }

    #[test]
    fn test_xlayer_aa_bytecodes_non_empty() {
        assert!(!XLayerAA::account_config_bytecode().is_empty());
        assert!(!XLayerAA::default_account_bytecode().is_empty());
        assert!(!XLayerAA::default_high_rate_account_bytecode().is_empty());
        assert!(!XLayerAA::k1_verifier_bytecode().is_empty());
        assert!(!XLayerAA::p256_verifier_bytecode().is_empty());
        assert!(!XLayerAA::webauthn_verifier_bytecode().is_empty());
        assert!(!XLayerAA::delegate_verifier_bytecode().is_empty());
    }
}
