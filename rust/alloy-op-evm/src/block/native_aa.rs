use alloy_evm::Database;
use alloy_op_hardforks::OpHardforks;
use alloy_primitives::{Address, Bytes};
use op_alloy::consensus::transaction::eip8130::{NONCE_MANAGER_ADDRESS, TX_CONTEXT_ADDRESS};
use revm::{DatabaseCommit, primitives::HashMap, state::Bytecode};

/// Precompile addresses that need stub bytecode to prevent EIP-161 cleanup.
///
/// The protocol writes storage directly to these addresses (nonces, tx
/// context). Without code, EIP-161 state clearing would remove the accounts
/// and their storage after each transaction.
///
/// `AccountConfiguration` is NOT included — it is a real Solidity contract
/// deployed via deposit transactions at NativeAA activation (see
/// `op-node/rollup/derive/native_aa_upgrade_transactions.go`). The node gates
/// AA validation on its presence: before deployment, only the implicit EOA
/// rule applies.
const AA_PRECOMPILE_ADDRESSES: [Address; 2] = [NONCE_MANAGER_ADDRESS, TX_CONTEXT_ADDRESS];

/// Stub bytecode deployed to precompile addresses.
///
/// `0xFE` is the `INVALID` opcode — any direct call reverts immediately.
/// The real logic is handled by the node as native precompiles in the EVM
/// handler. This stub ensures the accounts are non-empty under EIP-161,
/// preventing state cleanup from deleting their storage.
const AA_STUB_BYTECODE: &[u8] = &[0xFE];

/// The NativeAA hardfork issues an irregular state transition that
/// force-deploys stub bytecode to the EIP-8130 precompile addresses.
///
/// This mirrors `ensure_create2_deployer` for Canyon: code is set directly
/// via `DatabaseCommit` on the first block where the fork is active.
///
/// Uses a sentinel check (NonceManager code presence) rather than a
/// `timestamp - block_time` heuristic so the function is idempotent across
/// blocks and handles genesis-activated devnets correctly.
pub(crate) fn ensure_aa_predeploys<DB>(
    chain_spec: impl OpHardforks,
    timestamp: u64,
    db: &mut DB,
) -> Result<(), DB::Error>
where
    DB: Database + DatabaseCommit,
{
    if !chain_spec.is_native_aa_active_at_timestamp(timestamp) {
        return Ok(());
    }

    // Sentinel check: only deploy if the NonceManager still has no code.
    // This handles both the first NativeAA block and idempotent re-runs.
    let sentinel = db.basic(NONCE_MANAGER_ADDRESS)?;
    let already_deployed =
        sentinel.as_ref().is_some_and(|info| info.code_hash != revm::primitives::KECCAK_EMPTY);

    if already_deployed {
        return Ok(());
    }

    let code = Bytecode::new_raw(Bytes::from_static(AA_STUB_BYTECODE));
    let code_hash = code.hash_slow();

    let mut accounts = HashMap::default();
    for addr in AA_PRECOMPILE_ADDRESSES {
        let mut acc_info = db.basic(addr)?.unwrap_or_default();
        acc_info.code_hash = code_hash;
        acc_info.code = Some(code.clone());

        let mut revm_acc: revm::state::Account = acc_info.into();
        revm_acc.mark_touch();
        accounts.insert(addr, revm_acc);
    }

    db.commit(accounts);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_op_hardforks::{ForkCondition, OpChainHardforks, OpHardfork};
    use alloy_primitives::U256;
    use revm::{Database, database::InMemoryDB};

    fn devnet_spec() -> OpChainHardforks {
        OpChainHardforks::new([(OpHardfork::NativeAA, ForkCondition::Timestamp(0))])
    }

    fn make_db() -> InMemoryDB {
        InMemoryDB::default()
    }

    #[test]
    fn idempotent_when_already_deployed() {
        let mut db = make_db();
        let spec = devnet_spec();

        ensure_aa_predeploys(&spec, 0, &mut db).unwrap();

        let mut info = db.basic(NONCE_MANAGER_ADDRESS).unwrap().unwrap_or_default();
        info.balance = U256::from(42);
        db.insert_account_info(NONCE_MANAGER_ADDRESS, info);

        ensure_aa_predeploys(&spec, 2, &mut db).unwrap();
        let info = db.basic(NONCE_MANAGER_ADDRESS).unwrap().expect("account should exist");
        assert_eq!(info.balance, U256::from(42));
    }

    #[test]
    fn no_op_when_fork_inactive() {
        let mut db = make_db();
        let spec = OpChainHardforks::op_mainnet();

        ensure_aa_predeploys(&spec, 0, &mut db).unwrap();

        let info = db.basic(NONCE_MANAGER_ADDRESS).unwrap();
        assert!(info.is_none());
    }
}
