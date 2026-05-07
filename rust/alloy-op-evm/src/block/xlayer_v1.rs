//! XLayer V1 hardfork: irregular state transition that force-deploys stub
//! bytecode at the EIP-8130 precompile addresses (`NonceManager`,
//! `TxContext`).
//!
//! # Why a stub is needed
//!
//! The handler writes storage directly to `NONCE_MANAGER_ADDRESS` (per-tx
//! 2D nonce slots and the expiring-nonce ring buffer). Per EIP-161 a
//! "touched empty" account — `nonce == 0 && balance == 0 && code is
//! empty` — is removed at end-of-tx **regardless of storage**. Storage
//! writes alone don't keep the account alive, so without code we'd lose
//! every AA tx's nonce mutation. Giving these addresses a single-byte
//! `INVALID` (`0xFE`) opcode makes them non-empty under EIP-161 and the
//! storage persists.
//!
//! Direct calls (a contract calling into `NonceManager` /  `TxContext`)
//! still trip `INVALID` and revert — the real precompile logic lives in
//! the handler / EVM context, not in this stub. Only the protocol writes
//! are honored.
//!
//! # Why AccountConfiguration is excluded
//!
//! `ACCOUNT_CONFIG_ADDRESS` is deployed by an upgrade `TxDeposit` at the
//! same hardfork activation — see
//! [`kona_hardforks::XlayerV1::deposits`]. It receives real Solidity
//! bytecode, so the EIP-161 cleanup risk doesn't apply.
//!
//! # OP convention
//!
//! Mirrors the canyon-era [`super::canyon::ensure_create2_deployer`]:
//! invoked from [`super::OpBlockExecutor::apply_pre_execution_changes`],
//! idempotent (no-op once code is already present), gated on the
//! `is_xlayer_v1_active_at_timestamp` predicate from
//! [`alloy_op_hardforks::OpHardforks`].

use alloy_evm::Database;
use alloy_op_hardforks::OpHardforks;
use alloy_primitives::{Address, Bytes};
use op_revm::precompiles_xlayer::{NONCE_MANAGER_ADDRESS, TX_CONTEXT_ADDRESS};
use revm::{DatabaseCommit, primitives::HashMap, state::Bytecode};

/// Stub bytecode deployed at AA precompile addresses to keep them
/// non-empty under EIP-161. `0xFE` is the `INVALID` opcode — direct calls
/// revert immediately; the real precompile logic is handled by the
/// handler.
const AA_STUB_BYTECODE: &[u8] = &[0xFE];

/// Addresses that need stub bytecode at XLayer V1 activation.
///
/// `AccountConfiguration` is intentionally excluded; it gets a real
/// Solidity contract via `XlayerV1` upgrade deposit transactions.
const AA_PRECOMPILE_ADDRESSES: [Address; 2] = [NONCE_MANAGER_ADDRESS, TX_CONTEXT_ADDRESS];

/// Force-deploys [`AA_STUB_BYTECODE`] at every [`AA_PRECOMPILE_ADDRESSES`]
/// entry on the first XLayer V1 block, then is a no-op for subsequent
/// blocks.
///
/// # Why irregular state transition (not an upgrade deposit tx)
///
/// The targets sit in the precompile-style namespace (`0x000...aa0X`),
/// which is outside any `deployer.create(N)` derivation — so a deposit
/// `CREATE` tx physically can't land bytecode there. Meanwhile the handler
/// writes per-tx storage at these addresses, and EIP-161 garbage-collects
/// touched-empty accounts regardless of storage, so we need *some* code to
/// keep the slots alive. A 1-byte `0xFE` stub is the minimal fix; the real
/// logic stays native in the handler.
///
/// Unlike [`super::canyon::ensure_create2_deployer`], we do **not** use the
/// `!active_at(timestamp - block_time)` first-block heuristic. That trick
/// hardcodes OP mainnet's 2-second block time and is wrong for chains with
/// other cadences (XLayer is 1s) and for genesis-activated devnets where
/// `block 0` has no `timestamp - delta` to compare against. Instead we gate
/// on observed code state: if the sentinel address already has non-empty
/// code, the stubs were deployed on a prior block and we skip.
pub(crate) fn ensure_aa_predeploys<DB>(
    chain_spec: impl OpHardforks,
    timestamp: u64,
    db: &mut DB,
) -> Result<(), DB::Error>
where
    DB: Database + DatabaseCommit,
{
    if !chain_spec.is_xlayer_v1_active_at_timestamp(timestamp) {
        return Ok(());
    }

    // Idempotent: only deploy when the sentinel slot is still code-less.
    // We don't use canyon's `!active_at(timestamp - block_time)` heuristic
    // because it bakes in a fork-specific block time and breaks on
    // genesis-activated devnets.
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
    use alloy_hardforks::ForkCondition;
    use alloy_op_hardforks::{OpChainHardforks, OpHardfork};
    use revm::Database;

    /// Builds an `OpChainHardforks` where XLayer V1 (and every prior fork)
    /// activates at timestamp 0.
    ///
    /// `OpChainHardforks` looks up `op_fork_activation(fork)` by positional
    /// index into its internal `Vec` (`fork.idx()`). `OpHardfork::op_mainnet()`
    /// only ships 9 entries (Bedrock..Jovian), so we have to explicitly
    /// chain `Karst` and `XLayerV1` to fill positions 9 and 10 — without
    /// them the indexed lookup returns `ForkCondition::Never` for XLayer V1
    /// regardless of any "active at 0" intent.
    fn xlayer_v1_active_from_genesis() -> OpChainHardforks {
        OpChainHardforks::new(
            OpHardfork::op_mainnet()
                .into_iter()
                .map(|(fork, _)| (fork, ForkCondition::Timestamp(0)))
                .chain([
                    (OpHardfork::Karst, ForkCondition::Timestamp(0)),
                    (OpHardfork::XLayerV1, ForkCondition::Timestamp(0)),
                ]),
        )
    }

    /// Capability: at XLayer V1 activation, both AA precompile addresses
    /// receive `0xFE` stub bytecode so subsequent storage writes survive
    /// EIP-161 cleanup. Fixture: hardforks scheduled with XLayer V1 active
    /// at timestamp 0. Entry: `ensure_aa_predeploys`. Assert: stub
    /// bytecode at every entry of `AA_PRECOMPILE_ADDRESSES`.
    #[test]
    fn deploys_stubs_at_xlayer_v1_activation() {
        let spec = xlayer_v1_active_from_genesis();
        let mut db = revm::database::CacheDB::<revm::database::EmptyDB>::default();

        // Pre-flight: confirm the fixture really has XLayer V1 active at t=0.
        assert!(spec.is_xlayer_v1_active_at_timestamp(0), "fixture mis-scheduled");

        ensure_aa_predeploys(&spec, 0, &mut db).unwrap();

        for addr in AA_PRECOMPILE_ADDRESSES {
            let info = db.basic(addr).unwrap().expect("account should exist after deploy");
            let code = info.code.as_ref().expect("code present");
            assert_eq!(
                code.original_bytes().as_ref(),
                AA_STUB_BYTECODE,
                "wrong stub at {addr}",
            );
        }
    }

    /// Idempotency: a second invocation must not clobber any state the
    /// account picked up between calls (balance, nonce, additional
    /// storage).
    #[test]
    fn second_invocation_is_idempotent() {
        let spec = xlayer_v1_active_from_genesis();
        let mut db = revm::database::CacheDB::<revm::database::EmptyDB>::default();

        ensure_aa_predeploys(&spec, 0, &mut db).unwrap();

        // Mutate the sentinel's balance after the first deploy, then
        // re-run. The second run must not revert this change.
        let mut info = db.basic(NONCE_MANAGER_ADDRESS).unwrap().unwrap_or_default();
        info.balance = alloy_primitives::U256::from(42);
        db.insert_account_info(NONCE_MANAGER_ADDRESS, info);

        ensure_aa_predeploys(&spec, 2, &mut db).unwrap();

        let info = db
            .basic(NONCE_MANAGER_ADDRESS)
            .unwrap()
            .expect("account should still exist after second invocation");
        assert_eq!(info.balance, alloy_primitives::U256::from(42));
    }

    /// Pre-fork no-op: chain specs without XLayer V1 scheduled get an
    /// empty state — the function silently returns without touching the
    /// database.
    #[test]
    fn no_op_when_fork_inactive() {
        // op_mainnet() does not schedule XLayer V1 at genesis.
        let spec = OpChainHardforks::op_mainnet();
        let mut db = revm::database::CacheDB::<revm::database::EmptyDB>::default();

        ensure_aa_predeploys(&spec, 0, &mut db).unwrap();

        let info = db.basic(NONCE_MANAGER_ADDRESS).unwrap();
        assert!(info.is_none(), "account must remain absent pre-fork");
    }
}
