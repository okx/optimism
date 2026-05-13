//! Builds [`Eip8130Parts`] for the handler from a [`TxEip8130`] + caller.
//!
//! Called once per tx-bytes-decoding from `crate::tx::eip8130_parts`.
//! Symmetric on sequencer + validator: both run the same auth resolution, so
//! a sequencer can't ship a tx the validator would later reject for a forged
//! signature — the [`AuthState::Invalid`] verdict happens once, here, and
//! `validate_env` rejects on it.
//!
//! Intrinsic gas is **not** computed here. The handler computes
//! `aa_intrinsic_gas` / `payer_intrinsic_gas` on demand via
//! [`op_revm::eip8130_gas`] using the active fork's
//! [`revm::context_interface::cfg::GasParams`] from `cfg.gas_params()`. The
//! cap on aggregate custom-verifier gas is similarly fork-bound and read
//! from [`op_revm::constants::XLAYER_AA_CUSTOM_VERIFIER_GAS_CAP`] at the
//! call site.
//!
//! ## Account-change parsing
//!
//! Each [`AccountChangeEntry`] variant projects onto distinct
//! [`Eip8130Parts`] fields:
//!
//! - `Create` → `code_placements` (1) + `pre_writes` (one per initial owner) +
//!   `account_creation_logs` (1) + `has_create_entry = true`. The deployed address is
//!   CREATE2-derived from `(ACCOUNT_CONFIG_ADDRESS, effective_salt, header || bytecode)` with
//!   `effective_salt = keccak256(user_salt || keccak256(sorted owners))`.
//!
//! - `ConfigChange` → `config_writes` (one per `OP_AUTHORIZE`/`OP_REVOKE` on a matching chain),
//!   `sequence_updates` (one per matching entry), `authorizer_validations` (one per matching
//!   entry), and `config_change_logs`. Entries whose `chain_id` is neither `0` (multichain) nor
//!   `tx.chain_id` are skipped here — the gas path bills them at `aa_config_change_skip_gas` to
//!   cover the SLOAD.
//!
//! - `Delegation` → `delegation_target = Some(target)`. EIP-8130 has no type for "at most one
//!   Delegation entry", so the handler defends with a structural check: a second `Delegation` entry
//!   overwrites the first here and the duplicate-account-change path is rejected by the spec's
//!   per-tx unit cap (`MAX_ACCOUNT_CHANGES_PER_TX`).
//!
//! Auto-delegation: when the sender has neither an explicit `Delegation`
//! entry nor a `Create` entry, we emit a candidate
//! `auto_delegation_code = 0xef0100 || DEFAULT_ACCOUNT_ADDRESS`. The
//! handler's `validate_against_state_and_deduct_caller` only applies the
//! candidate when the sender's on-chain code is empty (i.e., a fresh EOA).

use alloc::vec::Vec;

use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use op_alloy::consensus::{
    AccountChangeEntry, ConfigChangeEntry, CreateEntry, Owner, TxEip8130, config_change_digest,
};
use op_revm::{
    constants::{
        ACCOUNT_CONFIG_ADDRESS, DEFAULT_ACCOUNT_ADDRESS, DELEGATE_VERIFIER_ADDRESS,
        OP_AUTHORIZE_OWNER, OP_REVOKE_OWNER, OWNER_SCOPE_CONFIG, REVOKED_VERIFIER,
    },
    eip8130_gas::calldata_cost,
    transaction::eip8130::{
        Eip8130AccountChanges, Eip8130AuthorizerValidation, Eip8130Call, Eip8130CodePlacement,
        Eip8130ConfigLog, Eip8130ConfigOp, Eip8130Parts, Eip8130SequenceUpdate,
        Eip8130StorageWrite, Eip8130VerifyCall,
    },
};
// Note: `Bytes::slice(range)` is zero-copy (Arc-backed) — strictly preferred
// over copying. Use it on every per-entry parsing path that hands a sub-slice
// to the handler.

use super::{
    address::derive_account_address,
    auth_state::{build_payer_auth_state, build_sender_auth_state},
    native_verifier::{NativeVerifyResult, address_to_owner_id, try_native_verify},
    storage::{account_state_slot, encode_owner_config, owner_config_slot},
};

use op_alloy::consensus::encode_verify_call;

/// Auto-delegation designator: `0xef0100 || DEFAULT_ACCOUNT_ADDRESS` (23 bytes).
fn auto_delegation_designator() -> Bytes {
    let mut buf = [0u8; 23];
    buf[..3].copy_from_slice(&[0xef, 0x01, 0x00]);
    buf[3..].copy_from_slice(DEFAULT_ACCOUNT_ADDRESS.as_slice());
    Bytes::copy_from_slice(&buf)
}

/// Builds [`Eip8130Parts`] for a tx, given the caller (effective sender) address.
///
/// `caller` comes from upstream sender recovery (`recover_eip8130_sender` for
/// EOA-mode txs; `tx.sender` otherwise) and is used to fill `parts.sender`.
pub fn eip8130_parts(tx: &TxEip8130, caller: Address) -> Eip8130Parts {
    let sender_authstate = build_sender_auth_state(tx);
    let payer_authstate = build_payer_auth_state(tx);
    let sender = caller;
    let payer = tx.payer.unwrap_or(caller);

    // Walk `account_changes` once, projecting each variant onto the
    // appropriate `Eip8130Parts` fields. We accumulate into local Vecs so
    // we can fold them into the final struct in one shot at the end.
    let mut has_create_entry = false;
    let mut delegation_target: Option<Address> = None;
    let mut pre_writes: Vec<Eip8130StorageWrite> = Vec::new();
    let mut config_writes: Vec<Eip8130StorageWrite> = Vec::new();
    let mut sequence_updates: Vec<Eip8130SequenceUpdate> = Vec::new();
    let mut code_placements: Vec<Eip8130CodePlacement> = Vec::new();
    let mut authorizer_validations: Vec<Eip8130AuthorizerValidation> = Vec::new();
    let mut account_creation_logs: Vec<Eip8130ConfigLog> = Vec::new();
    let mut config_change_logs: Vec<Eip8130ConfigLog> = Vec::new();
    // Per-entry counts the gas path needs but can't derive from the
    // populated Vecs alone (skipped entries leave no Vec residue, etc.).
    let mut skipped_config_change_count: usize = 0;
    let mut delegation_entry_count: usize = 0;
    let mut create_initial_owners_count: usize = 0;
    // EIP-8130: a `Create` entry, if present, MUST be the first entry.
    // We don't short-circuit here — flag the violation so `validate_env`
    // returns the structural error after the full parse.
    let mut seen_non_create = false;
    let mut create_not_first_entry = false;
    let mut matching_config_change_with_zero_valid_ops = false;

    for entry in &tx.account_changes {
        match entry {
            AccountChangeEntry::Create(create) => {
                if seen_non_create {
                    create_not_first_entry = true;
                }
                create_initial_owners_count =
                    create_initial_owners_count.saturating_add(create.initial_owners.len());
                process_create_entry(
                    create,
                    &mut has_create_entry,
                    &mut pre_writes,
                    &mut code_placements,
                    &mut account_creation_logs,
                );
            }
            AccountChangeEntry::ConfigChange(change) => {
                seen_non_create = true;
                if !change_targets_us(change.chain_id, tx.chain_id) {
                    skipped_config_change_count = skipped_config_change_count.saturating_add(1);
                } else if !has_any_valid_op(change) {
                    // Matching entry with no effective op (empty owner_changes
                    // or every op of unknown change_type) would touch
                    // `_accountState` and run authorizer validation without
                    // producing a single `_ownerConfig` write. Flag for
                    // `validate_env` to reject — see
                    // `Eip8130AccountChanges::matching_config_change_with_zero_valid_ops`.
                    matching_config_change_with_zero_valid_ops = true;
                }
                process_config_change_entry(
                    sender,
                    tx.chain_id,
                    change,
                    &mut config_writes,
                    &mut sequence_updates,
                    &mut authorizer_validations,
                    &mut config_change_logs,
                );
            }
            AccountChangeEntry::Delegation(d) => {
                seen_non_create = true;
                // Spec invariant: at most one `Delegation` entry per account.
                // Each tx targets one account (the sender), so this collapses
                // to "at most one Delegation per tx". The parser captures the
                // count; `validate_env` rejects when `> 1` so the duplicate
                // is caught structurally rather than silently overwritten.
                delegation_target = Some(d.target);
                delegation_entry_count = delegation_entry_count.saturating_add(1);
            }
        }
    }

    // Auto-delegation candidate: emitted whenever there is no explicit
    // delegation and no create entry. The handler's
    // `validate_against_state_and_deduct_caller` only applies it when the
    // on-chain sender code is empty — i.e. a fresh EOA.
    let auto_delegation_code = if delegation_target.is_none() && !has_create_entry {
        auto_delegation_designator()
    } else {
        Bytes::new()
    };

    // Nonce-free hash: the sealed tx hash. Only meaningful when the
    // mempool admits the tx as nonce-free (`nonce_key == U256::MAX`); the
    // handler's `validate_env` enforces presence via `nonce_free_hash` for
    // those txs.
    let nonce_free_hash = (tx.nonce_key == U256::MAX).then(|| tx.tx_hash());

    let account_changes = Eip8130AccountChanges {
        has_create_entry,
        delegation_target,
        account_change_units: account_change_units(tx),
        skipped_config_change_count,
        delegation_entry_count,
        create_initial_owners_count,
        auto_delegation_code,
        pre_writes,
        config_writes,
        sequence_updates,
        code_placements,
        create_not_first_entry,
        matching_config_change_with_zero_valid_ops,
        authorizer_validations,
        account_creation_logs,
        config_change_logs,
    };

    Eip8130Parts {
        expiry: tx.expiry,
        sender,
        payer,
        sender_authstate,
        payer_authstate,
        nonce_key: tx.nonce_key,
        nonce_free_hash,
        account_changes,
        sender_payload_calldata_cost: calldata_cost(&tx.encoded_for_sender_signing()),
        is_eoa: tx.is_eoa(),
        sender_auth: tx.sender_auth.clone(),
        payer_auth: tx.payer_auth.clone(),
        call_phases: tx
            .calls
            .iter()
            .map(|phase| {
                phase
                    .iter()
                    .map(|call| Eip8130Call {
                        to: call.to,
                        data: call.data.clone(),
                        value: U256::ZERO,
                    })
                    .collect()
            })
            .collect(),
    }
}

/// Counts account-change units in a tx.
///
/// Spec rule: each Create entry counts as `1 + initial_owners.len()` units,
/// each `ConfigChange` counts as `owner_changes.len()` units, each Delegation
/// counts as `1`. The handler caps the sum at
/// [`op_revm::constants::MAX_ACCOUNT_CHANGES_PER_TX`].
///
/// Exposed (`pub`) so admission can mirror execution's accounting byte-for-byte;
/// see `op_revm::handler` cap check at the comparison against
/// `MAX_ACCOUNT_CHANGES_PER_TX`.
pub fn account_change_units(tx: &TxEip8130) -> usize {
    tx.account_changes
        .iter()
        .map(|entry| match entry {
            AccountChangeEntry::Create(c) => 1 + c.initial_owners.len(),
            AccountChangeEntry::ConfigChange(cc) => cc.owner_changes.len(),
            AccountChangeEntry::Delegation(_) => 1,
        })
        .sum()
}

/// Projects a Create entry onto `code_placements` / `pre_writes` / logs.
///
/// Why CREATE2 here: the spec deploys a contract whose address is a pure
/// function of `(deployer, user_salt, owners, bytecode)` so two nodes
/// processing the same tx land on the same address without observing prior
/// chain state. `deployer` is `ACCOUNT_CONFIG_ADDRESS` (the system contract
/// that performs the equivalent CREATE2 on Solidity).
fn process_create_entry(
    create: &CreateEntry,
    has_create_entry: &mut bool,
    pre_writes: &mut Vec<Eip8130StorageWrite>,
    code_placements: &mut Vec<Eip8130CodePlacement>,
    account_creation_logs: &mut Vec<Eip8130ConfigLog>,
) {
    let account_addr = derive_account_address(
        ACCOUNT_CONFIG_ADDRESS,
        create.user_salt,
        &create.bytecode,
        &create.initial_owners,
    );

    // Runtime-bytecode placement at the derived address. The CREATE2 init
    // code RETURNs `bytecode` so the runtime is exactly `create.bytecode`.
    code_placements
        .push(Eip8130CodePlacement { address: account_addr, code: create.bytecode.clone() });

    // Owner registrations into AccountConfiguration._ownerConfig.
    for owner in &create.initial_owners {
        pre_writes.push(owner_registration_write(account_addr, owner));
    }

    // Protocol-injected log so block-explorers can index account creation
    // without parsing tx.account_changes.
    account_creation_logs.push(Eip8130ConfigLog::AccountCreated {
        account: account_addr,
        user_salt: create.user_salt,
        code_hash: keccak256(&create.bytecode),
    });

    *has_create_entry = true;
}

/// `(verifier, scope)` storage write at `_ownerConfig[account][owner_id]`.
fn owner_registration_write(account: Address, owner: &Owner) -> Eip8130StorageWrite {
    Eip8130StorageWrite {
        address: ACCOUNT_CONFIG_ADDRESS,
        slot: U256::from_be_bytes(owner_config_slot(account, owner.owner_id).0),
        value: U256::from_be_bytes(encode_owner_config(owner.verifier, owner.scope).0),
    }
}

/// Projects a `ConfigChange` entry onto `config_writes` / `sequence_updates` /
/// `authorizer_validations` / `config_change_logs`.
///
/// Skipping rule: an entry whose `chain_id` is neither `0` (multichain) nor
/// `tx.chain_id` (local) is dropped here. The gas path still bills it at
/// `aa_config_change_skip_gas` (one SLOAD) to cover the read the handler
/// would have issued before noticing the mismatch.
fn process_config_change_entry(
    sender: Address,
    tx_chain_id: u64,
    change: &ConfigChangeEntry,
    config_writes: &mut Vec<Eip8130StorageWrite>,
    sequence_updates: &mut Vec<Eip8130SequenceUpdate>,
    authorizer_validations: &mut Vec<Eip8130AuthorizerValidation>,
    config_change_logs: &mut Vec<Eip8130ConfigLog>,
) {
    if !change_targets_us(change.chain_id, tx_chain_id) {
        return;
    }

    let self_owner_id = address_to_owner_id(sender);
    let mut owner_changes = Vec::with_capacity(change.owner_changes.len());

    for op in &change.owner_changes {
        // Each on-chain `OP_AUTHORIZE` writes the encoded `(verifier, scope)`;
        // each `OP_REVOKE` writes either the REVOKED sentinel (when revoking
        // the implicit-EOA owner) or zero (deleting any other owner). Mirrors
        // base's `eip8130/execution.rs::config_change_writes`.
        let value = match op.change_type {
            OP_AUTHORIZE_OWNER => encode_owner_config(op.verifier, op.scope),
            OP_REVOKE_OWNER => {
                if op.owner_id == self_owner_id {
                    encode_owner_config(REVOKED_VERIFIER, 0)
                } else {
                    B256::ZERO
                }
            }
            // Unknown op codes are silently ignored at parse time. The spec
            // forbids them; the consensus-layer RLP decoder lets them through
            // because the entry struct doesn't know which codes are valid.
            // Emitting no write here mirrors base.
            _ => continue,
        };

        config_writes.push(Eip8130StorageWrite {
            address: ACCOUNT_CONFIG_ADDRESS,
            slot: U256::from_be_bytes(owner_config_slot(sender, op.owner_id).0),
            value: U256::from_be_bytes(value.0),
        });

        config_change_logs.push(match op.change_type {
            OP_AUTHORIZE_OWNER => Eip8130ConfigLog::OwnerAuthorized {
                account: sender,
                owner_id: op.owner_id,
                verifier: op.verifier,
                scope: op.scope,
            },
            OP_REVOKE_OWNER => {
                Eip8130ConfigLog::OwnerRevoked { account: sender, owner_id: op.owner_id }
            }
            // Unreachable due to the `continue` above, but stay defensive
            // and emit no log if a future op code lands without a logging
            // path.
            _ => continue,
        });

        owner_changes.push(Eip8130ConfigOp {
            change_type: op.change_type,
            verifier: op.verifier,
            owner_id: op.owner_id,
            scope: op.scope,
        });
    }

    // Sequence bump is read-modify-write on the packed `_accountState[sender]`
    // slot. `is_multichain == (cc.chain_id == 0)` selects which packed sub-field
    // to update; the other sub-field stays untouched.
    sequence_updates.push(Eip8130SequenceUpdate {
        slot: U256::from_be_bytes(account_state_slot(sender).0),
        is_multichain: change.chain_id == 0,
        new_value: change.sequence.saturating_add(1),
    });

    authorizer_validations.push(build_authorizer_validation(sender, change, owner_changes));
}

#[inline]
const fn change_targets_us(entry_chain_id: u64, tx_chain_id: u64) -> bool {
    entry_chain_id == 0 || entry_chain_id == tx_chain_id
}

/// `true` when `change` contains at least one op with a known `change_type`.
///
/// `process_config_change_entry` silently skips unknown `change_type` codes
/// (`_ => continue`) so they produce no storage write. We need to know
/// up-front whether any effective op survives so an entry that lists only
/// unknown ops (or none at all) can be rejected at `validate_env` rather
/// than triggering authorizer validation + sequence bump for nothing.
#[inline]
fn has_any_valid_op(change: &ConfigChangeEntry) -> bool {
    change
        .owner_changes
        .iter()
        .any(|op| matches!(op.change_type, OP_AUTHORIZE_OWNER | OP_REVOKE_OWNER))
}

/// Builds an [`Eip8130AuthorizerValidation`] from a config change.
///
/// EIP-712 digest: `config_change_digest(account=sender, change)`. Native
/// verifiers run eagerly here; custom verifiers defer the STATICCALL to the
/// handler via `verify_call`.
fn build_authorizer_validation(
    sender: Address,
    change: &ConfigChangeEntry,
    owner_changes: Vec<Eip8130ConfigOp>,
) -> Eip8130AuthorizerValidation {
    if change.authorizer_auth.is_empty() {
        // Empty authorizer auth: the handler treats it as missing-auth and
        // the change is rejected at the authorizer-validation stage. We
        // produce a `verifier = ZERO` placeholder so the gas path doesn't
        // misattribute it to a native cost.
        return Eip8130AuthorizerValidation {
            verifier: Address::ZERO,
            owner_id: B256::ZERO,
            verify_call: None,
            owner_changes,
        };
    }

    if change.authorizer_auth.len() < 20 {
        // Malformed: too short for a verifier prefix. Same fallback shape.
        return Eip8130AuthorizerValidation {
            verifier: Address::ZERO,
            owner_id: B256::ZERO,
            verify_call: None,
            owner_changes,
        };
    }

    let verifier_addr = Address::from_slice(&change.authorizer_auth[..20]);
    let verifier_data = change.authorizer_auth.slice(20..);
    let sig_hash = config_change_digest(sender, change);

    // Delegate-as-authorizer is rejected here: nesting Delegate within a
    // CONFIG-scope auth is not part of EIP-8130's authorizer surface (only
    // sender/payer). Treat as malformed.
    if verifier_addr == DELEGATE_VERIFIER_ADDRESS {
        return Eip8130AuthorizerValidation {
            verifier: verifier_addr,
            owner_id: B256::ZERO,
            verify_call: None,
            owner_changes,
        };
    }

    match try_native_verify(verifier_addr, &verifier_data, sig_hash) {
        NativeVerifyResult::Verified(owner_id) => Eip8130AuthorizerValidation {
            verifier: verifier_addr,
            owner_id,
            verify_call: None,
            owner_changes,
        },
        NativeVerifyResult::Invalid(_) => Eip8130AuthorizerValidation {
            verifier: verifier_addr,
            owner_id: B256::ZERO,
            verify_call: None,
            owner_changes,
        },
        NativeVerifyResult::Unsupported => Eip8130AuthorizerValidation {
            verifier: verifier_addr,
            owner_id: B256::ZERO,
            verify_call: Some(Eip8130VerifyCall {
                verifier: verifier_addr,
                calldata: encode_verify_call(sig_hash, &verifier_data),
                account: sender,
                required_scope: OWNER_SCOPE_CONFIG,
            }),
            owner_changes,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{U256, b256, hex};
    use op_alloy::consensus::{
        ConfigChangeEntry, CreateEntry, DelegationEntry, Eip8130CallEntry, OwnerChange,
    };

    fn sample_owner(seed: u8) -> Owner {
        Owner {
            verifier: Address::repeat_byte(seed),
            owner_id: B256::repeat_byte(seed.wrapping_add(0x10)),
            scope: 0x02,
        }
    }

    fn empty_tx() -> TxEip8130 {
        TxEip8130 {
            chain_id: 8453,
            sender: Some(Address::repeat_byte(0x01)),
            nonce_key: U256::ZERO,
            nonce_sequence: 0,
            expiry: 0,
            max_priority_fee_per_gas: 1,
            max_fee_per_gas: 10,
            gas_limit: 100_000,
            account_changes: Vec::new(),
            calls: Vec::new(),
            payer: None,
            sender_auth: Bytes::new(),
            payer_auth: Bytes::new(),
        }
    }

    #[test]
    fn empty_account_changes_yields_default_fields() {
        let tx = empty_tx();
        let parts = eip8130_parts(&tx, Address::repeat_byte(0x01));
        assert!(!parts.account_changes.has_create_entry);
        assert!(parts.account_changes.delegation_target.is_none());
        assert!(parts.account_changes.pre_writes.is_empty());
        assert!(parts.account_changes.config_writes.is_empty());
        assert!(parts.account_changes.sequence_updates.is_empty());
        assert!(parts.account_changes.code_placements.is_empty());
        assert!(parts.account_changes.authorizer_validations.is_empty());
        assert!(parts.account_changes.account_creation_logs.is_empty());
        assert!(parts.account_changes.config_change_logs.is_empty());
        assert_eq!(parts.account_changes.account_change_units, 0);
        // No create / no delegation / non-empty auto-delegation candidate.
        assert_eq!(parts.account_changes.auto_delegation_code.len(), 23);
        assert_eq!(&parts.account_changes.auto_delegation_code[..3], &[0xef, 0x01, 0x00]);
    }

    #[test]
    fn create_entry_populates_placement_pre_writes_and_log() {
        let owners = vec![sample_owner(0x21), sample_owner(0x22)];
        let bytecode = Bytes::from_static(&hex!("60016002"));
        let user_salt = b256!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let mut tx = empty_tx();
        tx.account_changes = vec![AccountChangeEntry::Create(CreateEntry {
            user_salt,
            bytecode: bytecode.clone(),
            initial_owners: owners.clone(),
        })];

        let parts = eip8130_parts(&tx, Address::repeat_byte(0x01));

        assert!(parts.account_changes.has_create_entry);
        assert_eq!(parts.account_changes.code_placements.len(), 1);
        assert_eq!(parts.account_changes.code_placements[0].code, bytecode);

        // pre_writes: one per owner, all targeting the derived account
        // address with the correct slot/value.
        let derived = derive_account_address(ACCOUNT_CONFIG_ADDRESS, user_salt, &bytecode, &owners);
        assert_eq!(parts.account_changes.code_placements[0].address, derived);
        assert_eq!(parts.account_changes.pre_writes.len(), owners.len());
        for (write, owner) in parts.account_changes.pre_writes.iter().zip(&owners) {
            assert_eq!(write.address, ACCOUNT_CONFIG_ADDRESS);
            assert_eq!(
                write.slot,
                U256::from_be_bytes(owner_config_slot(derived, owner.owner_id).0),
            );
            assert_eq!(
                write.value,
                U256::from_be_bytes(encode_owner_config(owner.verifier, owner.scope).0),
            );
        }

        assert_eq!(parts.account_changes.account_creation_logs.len(), 1);
        match &parts.account_changes.account_creation_logs[0] {
            Eip8130ConfigLog::AccountCreated { account, user_salt: us, code_hash } => {
                assert_eq!(*account, derived);
                assert_eq!(*us, user_salt);
                assert_eq!(*code_hash, keccak256(&bytecode));
            }
            other => panic!("unexpected log: {other:?}"),
        }

        // No delegation candidate when has_create_entry == true.
        assert!(parts.account_changes.auto_delegation_code.is_empty());

        // Units: 1 (create) + 2 (owners) = 3.
        assert_eq!(parts.account_changes.account_change_units, 3);
    }

    #[test]
    fn config_change_local_chain_populates_writes_seq_and_log() {
        let sender = Address::repeat_byte(0x01);
        let mut tx = empty_tx();
        tx.account_changes = vec![AccountChangeEntry::ConfigChange(ConfigChangeEntry {
            chain_id: tx.chain_id,
            sequence: 5,
            owner_changes: vec![
                OwnerChange {
                    change_type: OP_AUTHORIZE_OWNER,
                    verifier: Address::repeat_byte(0xAA),
                    owner_id: B256::repeat_byte(0x33),
                    scope: 0x02,
                },
                OwnerChange {
                    change_type: OP_REVOKE_OWNER,
                    verifier: Address::ZERO,
                    owner_id: B256::repeat_byte(0x44),
                    scope: 0,
                },
            ],
            authorizer_auth: Bytes::new(),
        })];

        let parts = eip8130_parts(&tx, sender);

        assert_eq!(parts.account_changes.config_writes.len(), 2);
        assert_eq!(parts.account_changes.config_change_logs.len(), 2);
        // First op: authorize, value = encode_owner_config(verifier, scope).
        assert_eq!(
            parts.account_changes.config_writes[0].value,
            U256::from_be_bytes(encode_owner_config(Address::repeat_byte(0xAA), 0x02).0),
        );
        // Second op: revoke a non-self owner → ZERO sentinel.
        assert_eq!(parts.account_changes.config_writes[1].value, U256::ZERO);

        assert_eq!(parts.account_changes.sequence_updates.len(), 1);
        let upd = &parts.account_changes.sequence_updates[0];
        assert_eq!(upd.slot, U256::from_be_bytes(account_state_slot(sender).0));
        assert!(!upd.is_multichain);
        assert_eq!(upd.new_value, 6);

        assert_eq!(parts.account_changes.authorizer_validations.len(), 1);
        assert_eq!(parts.account_changes.authorizer_validations[0].owner_changes.len(), 2);

        assert_eq!(parts.account_changes.account_change_units, 2);
    }

    #[test]
    fn config_change_multichain_marks_is_multichain() {
        let sender = Address::repeat_byte(0x01);
        let mut tx = empty_tx();
        tx.account_changes = vec![AccountChangeEntry::ConfigChange(ConfigChangeEntry {
            chain_id: 0,
            sequence: 0,
            owner_changes: vec![OwnerChange {
                change_type: OP_AUTHORIZE_OWNER,
                verifier: Address::repeat_byte(0x66),
                owner_id: B256::repeat_byte(0x77),
                scope: 0x04,
            }],
            authorizer_auth: Bytes::new(),
        })];
        let parts = eip8130_parts(&tx, sender);
        assert_eq!(parts.account_changes.sequence_updates.len(), 1);
        assert!(parts.account_changes.sequence_updates[0].is_multichain);
        assert_eq!(parts.account_changes.sequence_updates[0].new_value, 1);
    }

    #[test]
    fn config_change_revoke_self_writes_revoked_sentinel() {
        let sender = Address::repeat_byte(0xAA);
        let mut tx = empty_tx();
        tx.sender = Some(sender);
        tx.account_changes = vec![AccountChangeEntry::ConfigChange(ConfigChangeEntry {
            chain_id: tx.chain_id,
            sequence: 0,
            owner_changes: vec![OwnerChange {
                change_type: OP_REVOKE_OWNER,
                verifier: Address::ZERO,
                // self-owner: bytes32(bytes20(sender)).
                owner_id: address_to_owner_id(sender),
                scope: 0,
            }],
            authorizer_auth: Bytes::new(),
        })];
        let parts = eip8130_parts(&tx, sender);
        assert_eq!(parts.account_changes.config_writes.len(), 1);
        let expected = encode_owner_config(REVOKED_VERIFIER, 0);
        assert_eq!(parts.account_changes.config_writes[0].value, U256::from_be_bytes(expected.0));
    }

    #[test]
    fn config_change_wrong_chain_is_skipped() {
        let sender = Address::repeat_byte(0x01);
        let mut tx = empty_tx();
        tx.account_changes = vec![AccountChangeEntry::ConfigChange(ConfigChangeEntry {
            chain_id: tx.chain_id + 1, // different chain
            sequence: 5,
            owner_changes: vec![OwnerChange {
                change_type: OP_AUTHORIZE_OWNER,
                verifier: Address::repeat_byte(0xAA),
                owner_id: B256::repeat_byte(0x33),
                scope: 0x02,
            }],
            authorizer_auth: Bytes::new(),
        })];
        let parts = eip8130_parts(&tx, sender);
        // Skipped: nothing to apply here. Gas is still billed by the gas
        // path (account_changes_cost has a per-skip slot).
        assert!(parts.account_changes.config_writes.is_empty());
        assert!(parts.account_changes.sequence_updates.is_empty());
        assert!(parts.account_changes.authorizer_validations.is_empty());
        assert!(parts.account_changes.config_change_logs.is_empty());
        // Units still count from the entry (one owner_change).
        assert_eq!(parts.account_changes.account_change_units, 1);
    }

    #[test]
    fn delegation_entry_populates_target_and_clears_auto() {
        let target = Address::repeat_byte(0xCD);
        let mut tx = empty_tx();
        tx.account_changes = vec![AccountChangeEntry::Delegation(DelegationEntry { target })];
        let parts = eip8130_parts(&tx, Address::repeat_byte(0x01));
        assert_eq!(parts.account_changes.delegation_target, Some(target));
        // Auto-delegation candidate suppressed when an explicit Delegation
        // is present.
        assert!(parts.account_changes.auto_delegation_code.is_empty());
        assert_eq!(parts.account_changes.account_change_units, 1);
    }

    #[test]
    fn duplicate_create_entries_yield_two_placements() {
        // Spec invariant: at most one Create entry per tx. The parser must
        // not silently drop a duplicate — the handler's `validate_env`
        // (`code_placements.len() > 1`) is the rejection point.
        let owner = sample_owner(0x21);
        let bytecode = Bytes::from_static(&[0x00]);
        let user_salt = B256::repeat_byte(0xAA);
        let mut tx = empty_tx();
        tx.account_changes = vec![
            AccountChangeEntry::Create(CreateEntry {
                user_salt,
                bytecode: bytecode.clone(),
                initial_owners: vec![owner.clone()],
            }),
            AccountChangeEntry::Create(CreateEntry {
                user_salt: B256::repeat_byte(0xBB),
                bytecode,
                initial_owners: vec![owner],
            }),
        ];
        let parts = eip8130_parts(&tx, Address::repeat_byte(0x01));
        assert_eq!(
            parts.account_changes.code_placements.len(),
            2,
            "must not drop duplicate Create entry",
        );
    }

    #[test]
    fn nonce_free_hash_set_only_when_nonce_max() {
        let mut tx = empty_tx();
        let parts = eip8130_parts(&tx, Address::repeat_byte(0x01));
        assert!(parts.nonce_free_hash.is_none());

        tx.nonce_key = U256::MAX;
        let parts = eip8130_parts(&tx, Address::repeat_byte(0x01));
        assert_eq!(parts.nonce_free_hash, Some(tx.tx_hash()));
    }

    /// Spec invariant: a `Create` entry, if present, must be the first
    /// entry. The parser flags any Create that follows another entry via
    /// `create_at_invalid_position = true`. Validation is the handler's
    /// job — the parser must not short-circuit so the rejection happens
    /// once at the `validate_env` boundary.
    #[test]
    fn create_following_other_entry_flagged_as_invalid_position() {
        let target = Address::repeat_byte(0xCD);
        let owner = sample_owner(0x21);
        let bytecode = Bytes::from_static(&[0x00]);
        let mut tx = empty_tx();
        tx.account_changes = vec![
            AccountChangeEntry::Delegation(DelegationEntry { target }),
            AccountChangeEntry::Create(CreateEntry {
                user_salt: B256::repeat_byte(0xAA),
                bytecode,
                initial_owners: vec![owner],
            }),
        ];
        let parts = eip8130_parts(&tx, Address::repeat_byte(0x01));
        assert!(
            parts.account_changes.create_not_first_entry,
            "Create after Delegation must flag invalid position",
        );
        assert!(parts.account_changes.has_create_entry);
        assert_eq!(parts.account_changes.delegation_entry_count, 1);
    }

    /// First-position Create is the spec-legal layout — flag stays false.
    #[test]
    fn create_first_then_other_entry_position_ok() {
        let owner = sample_owner(0x21);
        let bytecode = Bytes::from_static(&[0x00]);
        let mut tx = empty_tx();
        tx.account_changes = vec![
            AccountChangeEntry::Create(CreateEntry {
                user_salt: B256::repeat_byte(0xAA),
                bytecode,
                initial_owners: vec![owner],
            }),
            AccountChangeEntry::Delegation(DelegationEntry { target: Address::repeat_byte(0xCD) }),
        ];
        let parts = eip8130_parts(&tx, Address::repeat_byte(0x01));
        assert!(!parts.account_changes.create_not_first_entry);
        assert_eq!(parts.account_changes.delegation_entry_count, 1);
    }

    /// Spec invariant: at most one `Delegation` entry per account. The
    /// parser captures the count without rejecting; `validate_env` rejects
    /// when `> 1`.
    #[test]
    fn duplicate_delegation_entries_counted() {
        let mut tx = empty_tx();
        tx.account_changes = vec![
            AccountChangeEntry::Delegation(DelegationEntry { target: Address::repeat_byte(0xAA) }),
            AccountChangeEntry::Delegation(DelegationEntry { target: Address::repeat_byte(0xBB) }),
        ];
        let parts = eip8130_parts(&tx, Address::repeat_byte(0x01));
        assert_eq!(
            parts.account_changes.delegation_entry_count, 2,
            "parser must count duplicates rather than coalesce",
        );
        // Last-write-wins on `delegation_target` — handler still rejects via
        // the count, but the field has a deterministic value.
        assert_eq!(parts.account_changes.delegation_target, Some(Address::repeat_byte(0xBB)));
    }

    #[test]
    fn calls_carry_through_unchanged() {
        let mut tx = empty_tx();
        tx.calls = vec![vec![Eip8130CallEntry {
            to: Address::repeat_byte(0x55),
            data: Bytes::from_static(&[0xDE, 0xAD]),
        }]];
        let parts = eip8130_parts(&tx, Address::repeat_byte(0x01));
        assert_eq!(parts.call_phases.len(), 1);
        assert_eq!(parts.call_phases[0].len(), 1);
        assert_eq!(parts.call_phases[0][0].to, Address::repeat_byte(0x55));
        assert_eq!(parts.call_phases[0][0].data, Bytes::from_static(&[0xDE, 0xAD]));
    }
}
