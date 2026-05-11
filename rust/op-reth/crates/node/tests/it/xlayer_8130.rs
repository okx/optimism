//! XLayer EIP-8130 (AA) end-to-end coverage.
//!
//! Capability under test: `eth_sendRawTransaction(0x7B || rlp(...))` → pool admission →
//! builder pulls the tx into a payload → op-revm AA handler executes it → engine_newPayload
//! re-validates the produced block.
//!
//! XLayer-specific ingredients: (1) `xlayer_v1_activated()` on the chain spec to flip the
//! validator's fork gate, and (2) a `NONCE_MANAGER_ADDRESS` allocation in genesis so the
//! on-chain `aa_nonce_slot(sender, 0)` read in `validate_eip8130_transaction` step 5
//! returns 0.
//!
//! Required infrastructure (not test-specific): Ecotone is activated alongside XLayerV1
//! because OP's engine API only knows V3+ payloads — pre-Ecotone payload attributes are
//! malformed, and `OpExecutionPayload::from_block_unchecked` selects V3 on
//! `parent_beacon_block_root: Some(_)` and forces `blob_gas_used = Some(0)` on the way
//! back. Without Ecotone the builder writes the header with `blob_gas_used: None`, so
//! the validator's recomputed hash drifts. See
//! `reth_optimism_payload_builder::validator::tests::v3_payload_drops_blob_gas_when_builder_sets_none`
//! for the unit-level repro of that drift. XLayerV1 itself is still independent (per
//! xlayer-aa.md 2026-04-21); we don't drag in Fjord / Granite / Holocene / Isthmus /
//! Jovian / Karst — Ecotone is the floor for OP engine plumbing, not part of the
//! XLayer feature stack.

use alloy_consensus::{Sealable, transaction::SignerRecoverable};
use alloy_genesis::{Genesis, GenesisAccount};
use alloy_network::eip2718::Encodable2718;
use alloy_primitives::{Address, B256, Bytes, U256, address, bytes};
use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;
use op_alloy_consensus::{
    Eip8130CallEntry, OpTxEnvelope, OpTxType, TxEip8130, sender_signature_hash,
};
use op_alloy_rpc_types_engine::OpPayloadAttributes;
use op_revm::{constants::K1_VERIFIER_ADDRESS, precompiles_xlayer::aa_nonce_slot};
use reth_chainspec::EthChainSpec;
use reth_e2e_test_utils::setup_engine;
use reth_optimism_chainspec::{OpChainSpec, OpChainSpecBuilder};
use reth_optimism_node::OpNode;
use reth_optimism_payload_builder::OpPayloadAttrs;
use reth_provider::{StateProvider, StateProviderFactory};
use std::sync::Arc;

/// `NONCE_MANAGER_ADDRESS` (`address!("0x000000000000000000000000000000000000aa02")`).
/// Hard-coded so this test does not pull `op-revm` just for one constant.
const NONCE_MANAGER_ADDRESS: Address = address!("0x000000000000000000000000000000000000aa02");

/// Ecotone-shape, pre-Jovian payload-attributes generator.
///
/// `parent_beacon_block_root` must be `Some` once Ecotone is active (which it is — see
/// the module docstring on why Ecotone is required infrastructure for OP engine API).
/// `min_base_fee` and `eip_1559_params` are post-Jovian MUSTs; this test does not
/// activate Jovian, so both stay `None` and the engine builder accepts the attributes
/// as-is.
const fn minimal_payload_attributes(timestamp: u64) -> OpPayloadAttrs {
    OpPayloadAttrs(OpPayloadAttributes {
        payload_attributes: alloy_rpc_types_engine::PayloadAttributes {
            timestamp,
            prev_randao: B256::ZERO,
            suggested_fee_recipient: Address::ZERO,
            withdrawals: Some(vec![]),
            parent_beacon_block_root: Some(B256::ZERO),
            // Added upstream as `Option<u64>`; we don't drive a beacon
            // chain in this test so `None` matches the engine builder's
            // pre-PoSC default.
            slot_number: None,
        },
        transactions: None,
        no_tx_pool: None,
        gas_limit: Some(30_000_000),
        eip_1559_params: None,
        min_base_fee: None,
    })
}

/// Builds an Ecotone+XLayerV1 chain spec with `NONCE_MANAGER_ADDRESS` allocated in genesis,
/// so the on-chain `aa_nonce_slot(sender, 0)` read in `validate_eip8130_transaction` step 5
/// returns 0. Returns `(chain_spec, chain_id)`.
///
/// See module docstring for why Ecotone is required infrastructure (OP engine API only
/// knows V3+) even though XLayerV1 itself is independent.
fn xlayer_chain_spec() -> (Arc<OpChainSpec>, u64) {
    let mut genesis: Genesis =
        serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();
    genesis.alloc.insert(
        NONCE_MANAGER_ADDRESS,
        GenesisAccount { balance: U256::ZERO, ..Default::default() },
    );

    let chain_spec = Arc::new(
        OpChainSpecBuilder::base_mainnet()
            .genesis(genesis)
            .ecotone_activated()
            .xlayer_v1_activated()
            .build(),
    );
    let chain_id = chain_spec.chain().id();
    (chain_spec, chain_id)
}

/// Builds an unsigned EIP-8130 tx scaffold (sender_auth left blank for the caller to fill).
///
/// `max_fee_per_gas` is set to 20 gwei so the standard pool's basefee/priority-fee
/// admission does not reject as underpriced; the genesis basefee inherits from the
/// OP base mainnet upgrades and is non-zero.
fn aa_unsigned_tx(chain_id: u64, from: Address, target: Address, nonce_sequence: u64) -> TxEip8130 {
    TxEip8130 {
        chain_id,
        from: Some(from),
        nonce_key: U256::ZERO,
        nonce_sequence,
        expiry: 0,
        max_priority_fee_per_gas: 1_000_000_000,
        max_fee_per_gas: 20_000_000_000,
        gas_limit: 100_000,
        calls: vec![vec![Eip8130CallEntry { to: target, data: bytes!() }]],
        ..Default::default()
    }
}

/// Encodes a `Signature` as the 85-byte explicit-from `sender_auth` blob
/// `[K1_VERIFIER_ADDRESS(20) || r(32) || s(32) || v(1)]`.
///
/// Per EIP-8130, explicit-from sender auth uses the uniform
/// `[verifier_addr || data]` wire format; for the native K1 path the verifier is
/// `K1_VERIFIER_ADDRESS` (`address(1)`) and `data` is the bare 65-byte ECDSA sig.
/// EOA-mode (`tx.from == None`) uses a different shape — bare 65 bytes — but
/// this test only exercises explicit-from mode.
fn signature_to_sender_auth(sig: alloy_primitives::Signature) -> Bytes {
    let mut bytes = Vec::with_capacity(85);
    bytes.extend_from_slice(K1_VERIFIER_ADDRESS.as_slice());
    bytes.extend_from_slice(&sig.r().to_be_bytes::<32>());
    bytes.extend_from_slice(&sig.s().to_be_bytes::<32>());
    bytes.push(if sig.v() { 1 } else { 0 });
    Bytes::from(bytes)
}

/// Builds a 2718-encoded EIP-8130 tx, claiming `from = claimed_from` and signing
/// `sender_auth` with `signing_signer`.
///
/// When `claimed_from == signing_signer.address()`, the recovered address matches `from`
/// and the handler accepts (validator-correct shape). When they differ, ecrecover still
/// succeeds but `build_sender_auth_parts` flags `auth_invalid = true` and the handler
/// rejects — the slice-1 forged-`sender_auth` shape.
fn aa_raw_tx_bytes(
    chain_id: u64,
    claimed_from: Address,
    signing_signer: &PrivateKeySigner,
    target: Address,
    nonce_sequence: u64,
) -> Bytes {
    let unsigned = aa_unsigned_tx(chain_id, claimed_from, target, nonce_sequence);
    let sig_hash = sender_signature_hash(&unsigned);
    let sig = signing_signer.sign_hash_sync(&sig_hash).expect("sign sender_auth");
    let signed = TxEip8130 { sender_auth: signature_to_sender_auth(sig), ..unsigned };
    let envelope = OpTxEnvelope::Eip8130(signed.seal_slow());
    let mut buf = Vec::with_capacity(envelope.encode_2718_len());
    envelope.encode_2718(&mut buf);
    buf.into()
}

/// `eth_sendRawTransaction(0x7B || rlp(...))` → pool admission → builder pulls →
/// op-revm AA handler executes → block contains the AA tx.
/// We deliberately drive the build via `node.advance(...)` rather than `inject_tx` + `new_payload`,
/// so the payload also round-trips through `engine_newPayload` (validator hash recompute) and
/// `update_forkchoice` (FCU). This is the strict end-to-end shape: builder produces a
/// block, the engine API validates it, the FCU commits it.
///
/// Historically this exact form surfaced an `engine::tree: Invalid payload ... block
/// hash mismatch` log: a chain spec activating only XLayerV1 left Ecotone inactive at
/// the test's block timestamp, so the builder wrote a header with
/// `parent_beacon_block_root: Some(_)` but `blob_gas_used: None`. The V3 reverse path
/// in `OpExecutionPayload` then rewrote `blob_gas_used = Some(0)` on the validator
/// side, drifting the hash. The fix is structural: Ecotone is required infrastructure
/// for OP engine plumbing (see the module docstring), so we activate it on the chain
/// spec. The unit-level pin for the underlying alloy V3 asymmetry lives at
/// `reth_optimism_payload_builder::validator::tests::v3_payload_drops_blob_gas_when_builder_sets_none`.
#[tokio::test]
async fn test_xlayer_8130_tx_advance_repro() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (chain_spec, chain_id) = xlayer_chain_spec();
    let (mut nodes, wallet) = setup_engine::<OpNode>(
        1,
        chain_spec,
        false,
        Default::default(),
        minimal_payload_attributes,
    )
    .await?;
    let node = &mut nodes[0];
    let signer = wallet.inner.clone();
    let sender = signer.address();
    let target = Address::repeat_byte(0x22);

    // The form that triggers the engine_newPayload re-validation path. If the block
    // hash drifts during submit_payload, advance() returns an error here.
    let payloads = node
        .advance(1, move |_| {
            let signer = signer.clone();
            Box::pin(async move { aa_raw_tx_bytes(chain_id, sender, &signer, target, 0) })
        })
        .await?;

    assert_eq!(payloads.len(), 1, "advance must produce one payload");
    let block = payloads[0].block();
    let aa_tx = block
        .body()
        .transactions
        .iter()
        .find(|tx| tx.tx_type() == OpTxType::Eip8130)
        .expect("AA tx absent from produced block");
    let recovered = aa_tx.recover_signer().expect("AA sender recovery must succeed");
    assert_eq!(recovered, sender);

    Ok(())
}

///This is the validator-side defense: a tx with a forged `sender_auth` (valid 65-byte K1
/// signature, but signed by a different key than `tx.from` claims) MUST NOT execute.
#[tokio::test]
async fn test_xlayer_8130_tx_corrupted_sender_auth_rejected() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (chain_spec, chain_id) = xlayer_chain_spec();
    let (mut nodes, wallet) = setup_engine::<OpNode>(
        1,
        chain_spec,
        false,
        Default::default(),
        minimal_payload_attributes,
    )
    .await?;
    let node = &mut nodes[0];
    let claimed_from = wallet.inner.address();
    // A different key — its signature recovers to a different address than `claimed_from`.
    let attacker = PrivateKeySigner::from_bytes(&B256::repeat_byte(0xAB)).unwrap();
    let target = Address::repeat_byte(0x22);

    // Filler: a validly-signed AA tx at AA nonce_sequence 0. Without this, the pool
    // would have only the rejected tx and the build would produce an empty payload,
    // which the e2e harness's `wait_for_built_payload` panics on. The filler also
    // sharpens the assertion: the block must contain exactly the valid AA tx and
    // not the corrupted one.
    let valid_signer = wallet.inner.clone();
    let valid_tx = aa_raw_tx_bytes(chain_id, claimed_from, &valid_signer, target, 0);
    node.rpc.inject_tx(valid_tx).await.expect("admit validly-signed AA tx");

    // The forged tx claims `from = claimed_from` (the wallet) but is signed by
    // `attacker`. AA nonce_sequence 1 so it doesn't collide with the filler.
    let corrupted = aa_raw_tx_bytes(chain_id, claimed_from, &attacker, target, 1);
    // Mempool may or may not admit it (slice 1 doesn't touch mempool admission); both
    // outcomes satisfy the capability, the assertion is on what gets executed.
    let _ = node.rpc.inject_tx(corrupted).await;

    let payload = node.new_payload().await?;
    let block = payload.block();
    let aa_txs: Vec<_> =
        block.body().transactions.iter().filter(|tx| tx.tx_type() == OpTxType::Eip8130).collect();
    assert_eq!(
        aa_txs.len(),
        1,
        "expected exactly one AA tx in the block (the validly-signed filler); \
         the forged-sender_auth tx must have been rejected"
    );
    let recovered = aa_txs[0].recover_signer().expect("AA sender recovery");
    assert_eq!(
        recovered, claimed_from,
        "included AA tx must be the validly-signed one (recovered signer == wallet address)"
    );

    Ok(())
}

/// After a successful AA tx at `(nonce_key=0, nonce_sequence=0)` executes, the
/// `NonceManager` storage slot for `(sender, nonce_key=0)` MUST read as `1` —
/// the on-chain nonce was bumped by exactly one. This is the post-execution
/// state-effect check that complements `test_xlayer_8130_tx_advance_repro`
/// (which only verifies the tx made it into a block).
#[tokio::test]
async fn test_xlayer_8130_nonce_incremented() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (chain_spec, chain_id) = xlayer_chain_spec();
    let (mut nodes, wallet) = setup_engine::<OpNode>(
        1,
        chain_spec,
        false,
        Default::default(),
        minimal_payload_attributes,
    )
    .await?;
    let node = &mut nodes[0];
    let signer = wallet.inner.clone();
    let sender = signer.address();
    let target = Address::repeat_byte(0x22);

    let slot = aa_nonce_slot(sender, U256::ZERO);
    let key = B256::from(slot.to_be_bytes());

    let pre = node.inner.provider.latest()?.storage(NONCE_MANAGER_ADDRESS, key)?.unwrap_or_default();
    assert_eq!(pre, U256::ZERO, "pre-state nonce slot must be zero before any AA tx");

    node.advance(1, move |_| {
        let signer = signer.clone();
        Box::pin(async move { aa_raw_tx_bytes(chain_id, sender, &signer, target, 0) })
    })
    .await?;

    let post =
        node.inner.provider.latest()?.storage(NONCE_MANAGER_ADDRESS, key)?.unwrap_or_default();
    assert_eq!(
        post,
        U256::from(1),
        "NonceManager slot for (sender, nonce_key=0) must be 1 after one AA tx executed"
    );

    Ok(())
}
