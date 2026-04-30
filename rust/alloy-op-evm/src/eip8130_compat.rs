//! EIP-8130 EVM-compat: `TxEip8130` → `Eip8130Parts` converter.
//!
//! Ported 1:1 from base/crates/common/consensus/src/evm_compat.rs (lines 1-500),
//! the `build_eip8130_parts_with_costs` portion. Hosted here in alloy-op-evm
//! rather than op-alloy-consensus because the produced `Eip8130Parts` lives in
//! op-revm; alloy-op-evm already depends on both crates so this avoids a
//! layering inversion. The trailing `FromRecoveredTx` impls (base lines 501+)
//! are not ported here — alloy-op-evm/src/tx.rs has its own dispatch for
//! `OpTxEnvelope`.

use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use op_revm::transaction::eip8130::{
    Eip8130AuthorizerValidation, Eip8130Call, Eip8130CodePlacement, Eip8130ConfigLog,
    Eip8130ConfigOp, Eip8130Parts, Eip8130SequenceUpdate, Eip8130StorageWrite, Eip8130VerifyCall,
};

use op_alloy::consensus::transaction::eip8130::{
    ACCOUNT_CONFIG_ADDRESS, AccountChangeEntry, DELEGATE_VERIFIER_ADDRESS, K1_VERIFIER_ADDRESS,
    NONCE_KEY_MAX, OP_AUTHORIZE_OWNER, OP_REVOKE_OWNER, OwnerScope, TxEip8130, VerifierGasCosts,
    account_change_units, auth_verifier_kind, auto_delegation_code, config_change_digest,
    config_change_sequence, config_change_writes, delegate_inner_verifier, derive_account_address,
    encode_verify_call, intrinsic_gas_with_costs, owner_registration_writes, payer_auth_cost,
    payer_signature_hash, payer_verification_gas, sender_signature_hash, total_verification_gas,
};
#[cfg(feature = "native-verifier")]
use op_alloy::consensus::transaction::eip8130::{
    NativeVerifier, NativeVerifyResult, ParsedSenderAuth, VerifierKind, parse_sender_auth,
    try_native_verify,
};

/// Returns the runtime-configurable cap for aggregate custom verifier STATICCALL gas.
///
/// Mirrors base's `custom_verifier_gas_cap()`. base uses an `AtomicU64` for runtime
/// override; we use the compile-time default for now and add a setter when a use case
/// requires it.
fn custom_verifier_gas_cap() -> u64 {
    op_revm::constants::DEFAULT_CUSTOM_VERIFIER_GAS_CAP
}

#[cfg(feature = "native-verifier")]
fn derive_sender_owner_id(tx: &TxEip8130) -> B256 {
    let parsed = match parse_sender_auth(tx) {
        Ok(parsed) => parsed,
        Err(_) => return B256::ZERO,
    };
    let sig_hash = sender_signature_hash(tx);

    match parsed {
        ParsedSenderAuth::Eoa { signature } => {
            let signature = Bytes::copy_from_slice(&signature);
            match try_native_verify(K1_VERIFIER_ADDRESS, &signature, sig_hash) {
                NativeVerifyResult::Verified(owner_id) => owner_id,
                NativeVerifyResult::Invalid(_) | NativeVerifyResult::Unsupported => B256::ZERO,
            }
        }
        ParsedSenderAuth::Configured { verifier, data } => {
            match NativeVerifier::from_address(verifier) {
                Some(native) => {
                    let mut delegate_owner = [0u8; 32];
                    if native == NativeVerifier::Delegate && data.len() >= 20 {
                        delegate_owner[..20].copy_from_slice(&data[..20]);
                    }
                    match try_native_verify(native.address(), &data, sig_hash) {
                        NativeVerifyResult::Verified(owner_id) => owner_id,
                        NativeVerifyResult::Invalid(_) => B256::ZERO,
                        NativeVerifyResult::Unsupported => {
                            if native == NativeVerifier::Delegate && data.len() >= 20 {
                                B256::from(delegate_owner)
                            } else {
                                B256::ZERO
                            }
                        }
                    }
                }
                None => B256::ZERO,
            }
        }
    }
}

#[cfg(not(feature = "native-verifier"))]
fn derive_sender_owner_id(_tx: &TxEip8130) -> B256 {
    B256::ZERO
}

/// Verifies the payer's signature and returns the payer `owner_id`.
///
/// For self-pay transactions returns `B256::ZERO`. For sponsored transactions,
/// the first 20 bytes of `payer_auth` identify the verifier address; the
/// remaining bytes are passed to the native verifier.
#[cfg(feature = "native-verifier")]
fn derive_payer_owner_id(tx: &TxEip8130) -> B256 {
    if tx.is_self_pay() {
        return B256::ZERO;
    }

    let Some(verifier) = auth_verifier_kind(&tx.payer_auth) else {
        return B256::ZERO;
    };
    let data = Bytes::copy_from_slice(&tx.payer_auth[20..]);
    let sig_hash = payer_signature_hash(tx);

    match verifier {
        VerifierKind::Native(native) => {
            let mut delegate_owner = [0u8; 32];
            if native == NativeVerifier::Delegate && data.len() >= 20 {
                delegate_owner[..20].copy_from_slice(&data[..20]);
            }
            match try_native_verify(native.address(), &data, sig_hash) {
                NativeVerifyResult::Verified(owner_id) => owner_id,
                NativeVerifyResult::Invalid(_) => B256::ZERO,
                NativeVerifyResult::Unsupported => {
                    if native == NativeVerifier::Delegate && data.len() >= 20 {
                        B256::from(delegate_owner)
                    } else {
                        B256::ZERO
                    }
                }
            }
        }
        VerifierKind::Custom(_) => B256::ZERO,
    }
}

#[cfg(not(feature = "native-verifier"))]
fn derive_payer_owner_id(_tx: &TxEip8130) -> B256 {
    B256::ZERO
}

/// Builds a [`Eip8130VerifyCall`] for verifier auth that must run in-EVM.
///
/// Supported cases:
/// - **Custom verifier** auth: `CUSTOM(20) || data...`
/// - **Delegate auth with nested custom verifier**:
///   `DELEGATE(20) || delegate_account(20) || inner_auth(verifier(20) || data...)`
///
/// Returns `None` for native-only paths that do not require runtime STATICCALL.
fn build_verify_call(
    auth: &[u8],
    sig_hash: B256,
    account: Address,
    required_scope: u8,
    direct_nested_custom_delegate: bool,
) -> Option<Eip8130VerifyCall> {
    let verifier_kind = auth_verifier_kind(auth)?;
    let verifier = verifier_kind.address();

    if verifier_kind.is_custom() {
        let data = Bytes::copy_from_slice(&auth[20..]);
        let calldata = encode_verify_call(sig_hash, &data);
        return Some(Eip8130VerifyCall { verifier, calldata, account, required_scope });
    }

    // Delegate envelope:
    // DELEGATE(20) || delegate_account(20) || nested_auth(verifier(20) || ...)
    //
    // Nested native verifiers stay fully native. Nested custom verifiers can
    // either route directly to the nested verifier (native delegate wrapper) or
    // through the Solidity delegate contract, depending on caller policy.
    if verifier == DELEGATE_VERIFIER_ADDRESS && auth.len() >= 60 {
        let inner_verifier = Address::from_slice(&auth[40..60]);
        if inner_verifier != Address::ZERO
            && auth_verifier_kind(&auth[40..]).is_some_and(|inner| inner.is_custom())
        {
            let (call_verifier, call_account, data) = if direct_nested_custom_delegate {
                (
                    inner_verifier,
                    Address::from_slice(&auth[20..40]),
                    Bytes::copy_from_slice(&auth[60..]),
                )
            } else {
                (verifier, account, Bytes::copy_from_slice(&auth[20..]))
            };
            let calldata = encode_verify_call(sig_hash, &data);
            return Some(Eip8130VerifyCall {
                verifier: call_verifier,
                calldata,
                account: call_account,
                required_scope,
            });
        }
    }

    None
}

/// Builds per-config-change authorizer validation data.
///
/// For each `ConfigChangeEntry`, computes the config change digest, then:
/// - **Custom verifier (non-native address):** builds an [`Eip8130VerifyCall`] for runtime
///   STATICCALL.
/// - **Native verifier:** runs `try_native_verify` to obtain the `owner_id`.
///
/// Returns one [`Eip8130AuthorizerValidation`] per config change entry.
#[cfg(feature = "native-verifier")]
fn build_authorizer_validations(
    tx: &TxEip8130,
    sender: Address,
) -> Vec<Eip8130AuthorizerValidation> {
    let mut validations = Vec::new();
    for entry in &tx.account_changes {
        let cc = match entry {
            AccountChangeEntry::ConfigChange(cc) => cc,
            _ => continue,
        };
        if cc.authorizer_auth.len() < 20 {
            validations.push(Eip8130AuthorizerValidation::default());
            continue;
        }

        let digest = config_change_digest(sender, cc);
        let verifier = auth_verifier_kind(&cc.authorizer_auth).expect("len checked above");
        let verifier_address = verifier.address();

        let ops: Vec<Eip8130ConfigOp> = cc
            .owner_changes
            .iter()
            .map(|op| Eip8130ConfigOp {
                change_type: op.change_type,
                verifier: op.verifier,
                owner_id: op.owner_id,
                scope: op.scope,
            })
            .collect();

        if verifier.is_custom() {
            let verify_call =
                build_verify_call(&cc.authorizer_auth, digest, sender, OwnerScope::CONFIG, false);
            validations.push(Eip8130AuthorizerValidation {
                verifier: verifier_address,
                owner_id: B256::ZERO,
                verify_call,
                owner_changes: ops,
            });
        } else {
            let data = Bytes::copy_from_slice(&cc.authorizer_auth[20..]);
            let owner_id = match try_native_verify(verifier_address, &data, digest) {
                NativeVerifyResult::Verified(id) => id,
                NativeVerifyResult::Invalid(_) | NativeVerifyResult::Unsupported => B256::ZERO,
            };
            validations.push(Eip8130AuthorizerValidation {
                verifier: verifier_address,
                owner_id,
                verify_call: None,
                owner_changes: ops,
            });
        }
    }
    validations
}

#[cfg(not(feature = "native-verifier"))]
fn build_authorizer_validations(
    tx: &TxEip8130,
    sender: Address,
) -> Vec<Eip8130AuthorizerValidation> {
    let mut validations = Vec::new();
    for entry in &tx.account_changes {
        let cc = match entry {
            AccountChangeEntry::ConfigChange(cc) => cc,
            _ => continue,
        };
        if cc.authorizer_auth.len() < 20 {
            validations.push(Eip8130AuthorizerValidation::default());
            continue;
        }

        let digest = config_change_digest(sender, cc);
        let verifier = auth_verifier_kind(&cc.authorizer_auth).expect("len checked above");
        let verifier_address = verifier.address();

        let ops: Vec<Eip8130ConfigOp> = cc
            .owner_changes
            .iter()
            .map(|op| Eip8130ConfigOp {
                change_type: op.change_type,
                verifier: op.verifier,
                owner_id: op.owner_id,
                scope: op.scope,
            })
            .collect();

        let verify_call = if verifier.is_custom() {
            build_verify_call(&cc.authorizer_auth, digest, sender, OwnerScope::CONFIG, false)
        } else {
            None
        };

        validations.push(Eip8130AuthorizerValidation {
            verifier: verifier_address,
            owner_id: B256::ZERO,
            verify_call,
            owner_changes: ops,
        });
    }
    validations
}

/// Build [`Eip8130Parts`] from a decoded [`TxEip8130`] for use by the handler.
///
/// `recovered_caller` is the ecrecovered sender address. For EOA mode
/// (`from` empty) this is the address derived from ecrecover;
/// for configured mode it equals `tx.from`.
///
/// Uses [`VerifierGasCosts::BASE_V1`] for verification gas computation.
/// Call [`build_eip8130_parts_with_costs`] to supply custom gas costs.
pub fn build_eip8130_parts(tx: &TxEip8130, recovered_caller: Address) -> Eip8130Parts {
    build_eip8130_parts_with_costs(tx, recovered_caller, &VerifierGasCosts::BASE_V1)
}

/// Build [`Eip8130Parts`] from a decoded [`TxEip8130`] with explicit gas costs.
///
/// `recovered_caller` is the ecrecovered sender address. For EOA mode
/// it overrides an empty `tx.from`. For self-pay
/// transactions, the payer is also set to `recovered_caller`.
pub fn build_eip8130_parts_with_costs(
    tx: &TxEip8130,
    recovered_caller: Address,
    costs: &VerifierGasCosts,
) -> Eip8130Parts {
    let sender = recovered_caller;
    let payer = tx.payer.unwrap_or(recovered_caller);
    let owner_id = derive_sender_owner_id(tx);
    let payer_owner_id = derive_payer_owner_id(tx);

    let sender_inner = delegate_inner_verifier(&tx.sender_auth);
    let payer_inner = delegate_inner_verifier(&tx.payer_auth);
    let verification_gas = total_verification_gas(tx, costs, sender_inner, payer_inner);

    let has_create_entry =
        tx.account_changes.iter().any(|e| matches!(e, AccountChangeEntry::Create(_)));
    let total_account_change_units = account_change_units(tx);

    let mut pre_writes = Vec::new();
    let mut config_writes = Vec::new();
    let mut sequence_updates = Vec::new();
    let mut code_placements = Vec::new();
    let mut account_creation_logs = Vec::new();
    let mut config_change_logs = Vec::new();
    for entry in &tx.account_changes {
        match entry {
            AccountChangeEntry::Create(create) => {
                let account = derive_account_address(
                    ACCOUNT_CONFIG_ADDRESS,
                    create.user_salt,
                    &create.bytecode,
                    &create.initial_owners,
                );
                for w in owner_registration_writes(account, create) {
                    pre_writes.push(Eip8130StorageWrite {
                        address: w.address,
                        slot: w.slot,
                        value: w.value,
                    });
                }
                code_placements
                    .push(Eip8130CodePlacement { address: account, code: create.bytecode.clone() });

                for owner in &create.initial_owners {
                    account_creation_logs.push(Eip8130ConfigLog::OwnerAuthorized {
                        account,
                        owner_id: owner.owner_id,
                        verifier: owner.verifier,
                        scope: owner.scope,
                    });
                }
                account_creation_logs.push(Eip8130ConfigLog::AccountCreated {
                    account,
                    user_salt: create.user_salt,
                    code_hash: keccak256(&create.bytecode),
                });
            }
            AccountChangeEntry::ConfigChange(cc) => {
                for w in config_change_writes(sender, cc) {
                    config_writes.push(Eip8130StorageWrite {
                        address: w.address,
                        slot: w.slot,
                        value: w.value,
                    });
                }
                let seq = config_change_sequence(sender, cc);
                sequence_updates.push(Eip8130SequenceUpdate {
                    slot: seq.slot,
                    is_multichain: seq.is_multichain,
                    new_value: seq.new_value,
                });

                for op in &cc.owner_changes {
                    match op.change_type {
                        OP_AUTHORIZE_OWNER => {
                            config_change_logs.push(Eip8130ConfigLog::OwnerAuthorized {
                                account: sender,
                                owner_id: op.owner_id,
                                verifier: op.verifier,
                                scope: op.scope,
                            });
                        }
                        OP_REVOKE_OWNER => {
                            config_change_logs.push(Eip8130ConfigLog::OwnerRevoked {
                                account: sender,
                                owner_id: op.owner_id,
                            });
                        }
                        _ => {}
                    }
                }
            }
            AccountChangeEntry::Delegation(_) => {
                // delegation_target is extracted below
            }
        }
    }

    let call_phases: Vec<Vec<Eip8130Call>> = tx
        .calls
        .iter()
        .map(|phase| {
            phase
                .iter()
                .map(|call| Eip8130Call { to: call.to, data: call.data.clone(), value: U256::ZERO })
                .collect()
        })
        .collect();

    let sender_verify_call = if !tx.is_eoa() {
        let sig_hash = sender_signature_hash(tx);
        build_verify_call(&tx.sender_auth, sig_hash, sender, OwnerScope::SENDER, true)
    } else {
        None
    };

    let payer_verify_call = if !tx.is_self_pay() && !tx.payer_auth.is_empty() {
        let sig_hash = payer_signature_hash(tx);
        build_verify_call(&tx.payer_auth, sig_hash, payer, OwnerScope::PAYER, true)
    } else {
        None
    };

    let aa_intrinsic_gas = intrinsic_gas_with_costs(
        tx,
        false, /* cold nonce — worst case */
        tx.chain_id,
        costs,
    );

    let sender_auth_empty = !tx.is_eoa() && tx.sender_auth.is_empty();
    let payer_auth_empty = !tx.is_self_pay() && tx.payer_auth.is_empty();

    let payer_intrinsic_gas = payer_auth_cost(tx) + payer_verification_gas(tx, costs, payer_inner);

    let authorizer_validations = build_authorizer_validations(tx, sender);

    let nonce_free_hash =
        if tx.nonce_key == NONCE_KEY_MAX { Some(sender_signature_hash(tx)) } else { None };

    let delegation_target = tx.account_changes.iter().find_map(|e| match e {
        AccountChangeEntry::Delegation(d) => Some(d.target),
        _ => None,
    });

    let has_custom_verifier = tx.has_custom_verifier();
    let custom_verifier_gas_cap = if has_custom_verifier { custom_verifier_gas_cap() } else { 0 };

    let sender_verifier = if tx.is_eoa() {
        K1_VERIFIER_ADDRESS
    } else if tx.sender_auth.is_empty() {
        Address::ZERO
    } else {
        Address::from_slice(&tx.sender_auth[..20])
    };

    let payer_verifier = if tx.is_self_pay() || tx.payer_auth.is_empty() {
        Address::ZERO
    } else {
        Address::from_slice(&tx.payer_auth[..20])
    };

    Eip8130Parts {
        expiry: tx.expiry,
        sender,
        payer,
        owner_id,
        payer_owner_id,
        nonce_key: tx.nonce_key,
        nonce_free_hash,
        has_create_entry,
        delegation_target,
        account_change_units: total_account_change_units,
        verification_gas,
        aa_intrinsic_gas,
        payer_intrinsic_gas,
        custom_verifier_gas_cap,
        sender_verifier,
        payer_verifier,
        auto_delegation_code: auto_delegation_code(),
        pre_writes,
        config_writes,
        sequence_updates,
        code_placements,
        call_phases,
        sender_verify_call,
        payer_verify_call,
        authorizer_validations,
        account_creation_logs,
        config_change_logs,
        sender_auth_empty,
        payer_auth_empty,
    }
}
