//! Module containing a [`TxDeposit`] builder for the XLayer V1 network upgrade.
//!
//! The XLayer V1 fork activates EIP-8130 Account Abstraction support
//! (tx-type `0x7B`). At activation we deploy two system contracts via
//! deposit transactions:
//!
//! 1. `AccountConfiguration` — owner registrations, account creation, config changes, locks. The
//!    handler emits all account-creation / owner-change logs from this address.
//! 2. `DefaultAccount` — the wallet implementation bare EOAs are auto-delegated to via EIP-7702
//!    (`0xef0100 || DEFAULT_ACCOUNT_ADDRESS`).
//!
//! The four native verifiers (K1 / P256 / WebAuthn / Delegate) are
//! **not** deployed here — verification happens natively inside the
//! handler (`alloy_op_evm::eip8130::native_verifier::try_native_verify`)
//! against fixed sentinel addresses. Their addresses are reserved but
//! carry no on-chain code.
//!
//! Both deployer addresses + final deployed addresses are deterministic:
//! `XlayerV1::*_DEPLOYER.create(0)` matches `op_revm::constants`.

use alloc::{string::String, vec::Vec};
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{Address, B256, Bytes, TxKind, U256, address, hex};
use op_alloy_consensus::{TxDeposit, UpgradeDepositSource};

use crate::Hardfork;

/// The XLayer V1 network upgrade transactions.
#[derive(Debug, Default, Clone, Copy)]
pub struct XlayerV1;

impl XlayerV1 {
    /// Deployer for [`AccountConfiguration`]. Mirrors base's
    /// `BASE_V1_ACCOUNT_CONFIGURATION` numerically so cross-chain tooling that
    /// inspects the deposit-tx `from` field sees a consistent address.
    pub const ACCOUNT_CONFIGURATION_DEPLOYER: Address =
        address!("0x421000000000000000000000000000000000000b");

    /// Deployer for [`DefaultAccount`]. Mirrors base's
    /// `BASE_V1_DEFAULT_ACCOUNT`.
    pub const DEFAULT_ACCOUNT_DEPLOYER: Address =
        address!("0x421000000000000000000000000000000000000d");

    /// Source-hash intent for the `AccountConfiguration` deployment.
    pub fn deploy_account_configuration_source() -> B256 {
        UpgradeDepositSource { intent: String::from("XLayer V1: Account Configuration Deployment") }
            .source_hash()
    }

    /// Source-hash intent for the `DefaultAccount` deployment.
    pub fn deploy_default_account_source() -> B256 {
        UpgradeDepositSource { intent: String::from("XLayer V1: Default Account Deployment") }
            .source_hash()
    }

    /// `AccountConfiguration` deployed address. `deployer.create(0)`.
    pub fn account_configuration_address() -> Address {
        Self::ACCOUNT_CONFIGURATION_DEPLOYER.create(0)
    }

    /// `DefaultAccount` deployed address. `deployer.create(0)`.
    pub fn default_account_address() -> Address {
        Self::DEFAULT_ACCOUNT_DEPLOYER.create(0)
    }

    /// Creation bytecode for `AccountConfiguration`. No constructor args.
    pub fn account_configuration_bytecode() -> Bytes {
        hex::decode(
            include_str!("./bytecode/xlayer-v1-account-configuration-deployment.hex")
                .replace('\n', ""),
        )
        .expect("Expected hex byte string")
        .into()
    }

    /// Creation bytecode for `DefaultAccount`, with the `accountConfiguration`
    /// constructor argument appended (32-byte ABI-encoded address).
    pub fn default_account_bytecode() -> Bytes {
        let mut input = hex::decode(
            include_str!("./bytecode/xlayer-v1-default-account-deployment.hex").replace('\n', ""),
        )
        .expect("Expected hex byte string");
        input.extend_from_slice(Self::account_configuration_address().into_word().as_slice());
        input.into()
    }

    /// Returns the list of [`TxDeposit`]s injected at activation block.
    ///
    /// Order matters: `DefaultAccount`'s constructor stores the
    /// `accountConfiguration` address, so `AccountConfiguration` must deploy
    /// first.
    pub fn deposits() -> impl Iterator<Item = TxDeposit> {
        [
            TxDeposit {
                source_hash: Self::deploy_account_configuration_source(),
                from: Self::ACCOUNT_CONFIGURATION_DEPLOYER,
                to: TxKind::Create,
                mint: 0,
                value: U256::ZERO,
                gas_limit: 2_000_000,
                is_system_transaction: false,
                input: Self::account_configuration_bytecode(),
            },
            TxDeposit {
                source_hash: Self::deploy_default_account_source(),
                from: Self::DEFAULT_ACCOUNT_DEPLOYER,
                to: TxKind::Create,
                mint: 0,
                value: U256::ZERO,
                gas_limit: 500_000,
                is_system_transaction: false,
                input: Self::default_account_bytecode(),
            },
        ]
        .into_iter()
    }
}

impl Hardfork for XlayerV1 {
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
    use crate::test_utils::check_deployment_code;
    use alloy_primitives::{address, keccak256};

    /// Capability: deployed addresses match `op_revm::constants` so the
    /// handler's hardcoded predeploy addresses align with what the upgrade
    /// txs actually produce on-chain. Pure deployer-derivation check; no
    /// EVM run needed.
    #[test]
    fn deployed_addresses_match_op_revm_constants() {
        // Fixture: predeploy address constants from op-revm.
        let expected_account_config: Address =
            address!("0xf946601D5424118A4e4054BB0B13133f216b4FeE");
        let expected_default_account: Address =
            address!("0xAb4eE49EE97e49807e180BD5Fb9D9F35783b84F2");

        // Entry: deployer.create(0) — the same derivation the deposit-tx
        // execution does.
        let derived_account_config = XlayerV1::account_configuration_address();
        let derived_default_account = XlayerV1::default_account_address();

        // Assert: derived addresses match the handler's compile-time
        // constants. Drift here would silently break auto-delegation +
        // ConfigChange storage routing, so we pin both directions.
        assert_eq!(derived_account_config, expected_account_config);
        assert_eq!(derived_default_account, expected_default_account);
    }

    /// Capability: running the AccountConfiguration deposit tx through an
    /// in-memory EVM deploys runtime bytecode at the expected address with
    /// a deterministic code hash. Establishes the full
    /// "fixture + entry + assert" pattern for downstream deposit-tx tests.
    #[test]
    fn account_configuration_deploys_to_expected_address() {
        // Fixture: the account-configuration deposit tx, sourced from
        // XlayerV1::deposits().
        let txs = XlayerV1::deposits().collect::<Vec<_>>();
        let account_config_tx = txs[0].clone();

        // Entry + Assert: shared `check_deployment_code` runs the deposit tx
        // in a fresh in-memory EVM and verifies (a) the deployed address and
        // (b) the deployed code hash.
        check_deployment_code(
            account_config_tx,
            XlayerV1::account_configuration_address(),
            keccak256(deployed_account_configuration_runtime_bytecode()),
        );
    }

    /// Reads the AccountConfiguration runtime bytecode from the forge
    /// artifact for the deployed-bytecode hash assertion. Reading from the
    /// artifact (not a hardcoded hash) keeps the test honest against
    /// recompilations: a contract change re-derives its hash here and in
    /// the EVM run, so they continue to match.
    ///
    /// Helper kept inside `tests` so the artifact path doesn't leak into
    /// production code.
    fn deployed_account_configuration_runtime_bytecode() -> Bytes {
        const ARTIFACT: &str = include_str!(
            "../../../../../../packages/contracts-xlayer/EIP-8130/out/AccountConfiguration.sol/AccountConfiguration.json"
        );
        runtime_bytecode_from_artifact(ARTIFACT)
    }

    /// Same shape as the AccountConfiguration helper, for DefaultAccount.
    fn deployed_default_account_runtime_bytecode() -> Bytes {
        const ARTIFACT: &str = include_str!(
            "../../../../../../packages/contracts-xlayer/EIP-8130/out/DefaultAccount.sol/DefaultAccount.json"
        );
        runtime_bytecode_from_artifact(ARTIFACT)
    }

    /// Locates `deployedBytecode.object` within a forge artifact and decodes
    /// the hex string. We avoid a full `serde_json` dev-dep — the artifact
    /// shape is stable.
    fn runtime_bytecode_from_artifact(artifact: &str) -> Bytes {
        let needle = "\"deployedBytecode\":{\"object\":\"";
        let start =
            artifact.find(needle).expect("artifact missing deployedBytecode") + needle.len();
        let rest = &artifact[start..];
        let end = rest.find('"').expect("unterminated deployedBytecode");
        let hex_str = rest[..end].strip_prefix("0x").unwrap_or(&rest[..end]);
        hex::decode(hex_str).expect("invalid runtime bytecode hex").into()
    }

    /// Capability: the DefaultAccount deposit tx deploys non-empty runtime
    /// bytecode at the address op-revm hardcodes as `DEFAULT_ACCOUNT_ADDRESS`.
    ///
    /// Unlike AccountConfiguration, DefaultAccount has an `immutable`
    /// constructor arg (the AccountConfiguration address) baked into its
    /// runtime code, so the actual deployed bytecode differs from the forge
    /// artifact's `deployedBytecode` (which carries an unfilled immutable
    /// placeholder). We can't pin a code hash; we pin "deployed at the
    /// expected address with non-empty code" instead, which is what the
    /// EL+CL boundary actually requires.
    #[test]
    fn default_account_deploys_to_expected_address() {
        let txs = XlayerV1::deposits().collect::<Vec<_>>();
        let default_account_tx = txs[1].clone();

        check_deployment_succeeded(default_account_tx, XlayerV1::default_account_address());
        // Suppress the unused-helper warning when only one test reads the
        // artifact-derived bytecode.
        let _ = deployed_default_account_runtime_bytecode;
    }

    /// Lighter twin of [`check_deployment_code`]: runs the deposit tx in a
    /// fresh in-memory EVM and asserts the contract deployed to
    /// `expected_address` with **non-empty** bytecode. Use this for
    /// contracts with `immutable` constructor args whose runtime code
    /// can't be hash-pinned against the forge artifact.
    fn check_deployment_succeeded(
        deployment_tx: op_alloy_consensus::TxDeposit,
        expected_address: Address,
    ) {
        use op_revm::{DefaultOp, OpSpecId, transaction::deposit::DepositTransactionParts};
        use revm::{
            Context, ExecuteCommitEvm, MainBuilder,
            context::{
                CfgEnv,
                result::{ExecutionResult, Output},
            },
            database::{CacheDB, EmptyDB},
            interpreter::Host,
        };

        let ctx = Context::op()
            .with_cfg(CfgEnv::new_with_spec(OpSpecId::INTEROP))
            .modify_tx_chained(|tx| {
                tx.deposit = DepositTransactionParts {
                    source_hash: deployment_tx.source_hash,
                    mint: Some(deployment_tx.mint),
                    is_system_transaction: deployment_tx.is_system_transaction,
                };
                tx.enveloped_tx = Some(deployment_tx.encoded_2718().into());
                tx.base.tx_type = op_alloy_consensus::OpTxType::Deposit as u8;
                tx.base.caller = deployment_tx.from;
                tx.base.kind = deployment_tx.to;
                tx.base.value = deployment_tx.value;
                tx.base.gas_limit = deployment_tx.gas_limit;
                tx.base.data = deployment_tx.input;
            })
            .with_db(CacheDB::<EmptyDB>::default());
        let mut evm = ctx.build_mainnet();
        let res = evm.replay_commit().expect("Failed to run deployment transaction");

        let address = match res {
            ExecutionResult::Success { output: Output::Create(_, Some(addr)), .. } => addr,
            res => panic!("Failed to deploy contract: {res:?}"),
        };
        assert_eq!(address, expected_address, "Contract deployed to an unexpected address");

        let code = evm.load_account_code(address).expect("Account does not exist after deployment");
        assert!(!code.is_empty(), "Deployed code is empty at {address}");
    }
}
