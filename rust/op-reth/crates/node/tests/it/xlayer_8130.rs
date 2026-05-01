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
use op_alloy_consensus::{Eip8130CallEntry, OpTxEnvelope, OpTxType, TxEip8130};
use op_alloy_rpc_types_engine::OpPayloadAttributes;
use reth_chainspec::EthChainSpec;
use reth_e2e_test_utils::setup_engine;
use reth_optimism_chainspec::OpChainSpecBuilder;
use reth_optimism_node::OpNode;
use reth_optimism_payload_builder::OpPayloadAttrs;
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
        },
        transactions: None,
        no_tx_pool: None,
        gas_limit: Some(30_000_000),
        eip_1559_params: None,
        min_base_fee: None,
    })
}

/// Builds a 2718-encoded self-pay EIP-8130 transaction with a single empty call phase.
///
/// Mirrors `TransactionTestContext::optimism_l1_block_info_tx` in shape: returns the
/// raw bytes ready for `eth_sendRawTransaction`. The tx has no `account_changes`, no
/// sponsored payer, and no EOA recovery — i.e. exactly the shape the mempool
/// validator's MVP path admits today.
///
/// `max_fee_per_gas` is set to 20 gwei so the standard pool's basefee/priority-fee
/// admission does not reject as underpriced; the genesis basefee inherits from the
/// OP base mainnet upgrades and is non-zero.
fn aa_raw_tx_bytes(chain_id: u64, sender: Address, target: Address, nonce_sequence: u64) -> Bytes {
    let tx = TxEip8130 {
        chain_id,
        from: Some(sender),
        nonce_key: U256::ZERO,
        nonce_sequence,
        expiry: 0,
        max_priority_fee_per_gas: 1_000_000_000,
        max_fee_per_gas: 20_000_000_000,
        gas_limit: 100_000,
        calls: vec![vec![Eip8130CallEntry { to: target, data: bytes!() }]],
        ..Default::default()
    };
    let envelope = OpTxEnvelope::Eip8130(tx.seal_slow());
    let mut buf = Vec::with_capacity(envelope.encode_2718_len());
    envelope.encode_2718(&mut buf);
    buf.into()
}

/// `eth_sendRawTransaction(0x7B || rlp(...))` → pool admission → builder pulls →
/// op-revm AA handler executes → block contains the AA tx.
/// We deliberately drive the build via `node.advance(...)` rather than `inject_tx` + `new_payload`, so the payload also round-trips through `engine_newPayload` (validator hash recompute) and
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

    let (mut nodes, wallet) = setup_engine::<OpNode>(
        1,
        chain_spec.clone(),
        false,
        Default::default(),
        minimal_payload_attributes,
    )
    .await?;
    let node = &mut nodes[0];
    let sender = wallet.inner.address();
    let target = Address::repeat_byte(0x22);

    // The form that triggers the engine_newPayload re-validation path. If the block
    // hash drifts during submit_payload, advance() returns an error here.
    let payloads = node
        .advance(1, |_| Box::pin(async move { aa_raw_tx_bytes(chain_id, sender, target, 0) }))
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
