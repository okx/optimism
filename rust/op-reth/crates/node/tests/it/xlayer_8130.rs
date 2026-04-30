//! XLayer EIP-8130 (AA) end-to-end coverage.
//!
//! Mirrors the make_tx + add_single_transaction pattern from the txpool unit test, but
//! at the full EL layer: build a self-pay AA tx, push it through `eth_sendRawTransaction`
//! (via `RpcTestContext::inject_tx`), drive a payload build via the engine API, and
//! assert the AA tx lands in the produced block.
//!
//! The harness is the same `setup_engine` used elsewhere; the only XLayer-specific
//! ingredients are (1) `karst_activated()` on the chain spec to flip the validator's
//! fork gate, and (2) a `NONCE_MANAGER_ADDRESS` allocation in genesis so the on-chain
//! `aa_nonce_slot(sender, 0)` read in `validate_eip8130_transaction` step 5 returns 0.

use alloy_consensus::{Sealable, transaction::SignerRecoverable};
use alloy_genesis::{Genesis, GenesisAccount};
use alloy_network::eip2718::Encodable2718;
use alloy_primitives::{Address, B64, B256, Bytes, U256, address, bytes};
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

/// Karst-aware payload attributes generator.
///
/// The Jovian hardfork (which Karst transitively activates) requires `min_base_fee:
/// Some(_)` on every payload attribute set. The generic
/// `reth_optimism_node::utils::optimism_payload_attributes` leaves it `None`, which is
/// fine for pre-Jovian tests but rejected post-Jovian with
/// `MinimumBaseFeeNotSet`/"cannot be None after Jovian".
const fn karst_payload_attributes(timestamp: u64) -> OpPayloadAttrs {
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
        // Post-Jovian: both `eip_1559_params` and `min_base_fee` are MUSTs on every
        // payload-attributes set. `B64::ZERO` is the canonical empty-params placeholder
        // used by the e2e-testsuite; the chainspec's default basefee params are used.
        eip_1559_params: Some(B64::ZERO),
        min_base_fee: Some(1),
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
/// admission does not reject as underpriced; the genesis basefee under
/// `karst_activated()` inherits from the OP base mainnet upgrades and is non-zero.
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

/// End-to-end smoke test: `eth_sendRawTransaction(0x7B || rlp(...))` →
/// pool admission → builder pulls → op-revm AA handler executes → block produced.
///
/// The single closure argument to `node.advance` is the EL equivalent of the txpool
/// unit test's `make_tx`; `advance` itself plays the role of `add_single_transaction`,
/// fanning out into RPC ingress + engine FCU + payload build + forkchoice update.
#[tokio::test]
async fn test_basic_xlayer_8130_tx() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    // Genesis: load the standard fixture and inject the NONCE_MANAGER allocation so
    // the validator's storage read in step 5 hits an existing account.
    let mut genesis: Genesis =
        serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();
    genesis.alloc.insert(
        NONCE_MANAGER_ADDRESS,
        GenesisAccount { balance: U256::ZERO, ..Default::default() },
    );

    // Karst-activated chain spec — mempool validator's fork gate is keyed on Karst.
    let chain_spec =
        Arc::new(OpChainSpecBuilder::base_mainnet().genesis(genesis).karst_activated().build());
    let chain_id = chain_spec.chain().id();

    // Boot a single-node engine. `setup_engine` wires the full stack: pool, validator,
    // payload builder, engine API, RPC. We do NOT customize the payload-priority hook
    // here — base ordering is fine; we only care that the AA tx round-trips.
    let (mut nodes, wallet) = setup_engine::<OpNode>(
        1,
        chain_spec.clone(),
        false,
        Default::default(),
        karst_payload_attributes,
    )
    .await?;
    let node = &mut nodes[0];
    let sender = wallet.inner.address();
    let target = Address::repeat_byte(0x22);

    // Drive the full RPC → pool → builder → execute loop. Per the e2e-test-utils
    // contract, `advance(1, …)` produces exactly one block carrying the injected tx.
    let payloads = node
        .advance(1, |_| Box::pin(async move { aa_raw_tx_bytes(chain_id, sender, target, 0) }))
        .await?;

    assert_eq!(payloads.len(), 1, "advance must produce one payload");
    let block = payloads[0].block();

    // The block must contain our AA tx. We don't assert positional ordering because
    // OP nodes may inject system / deposit txs ahead of mempool-pulled user txs.
    let aa_tx = block
        .body()
        .transactions
        .iter()
        .find(|tx| tx.tx_type() == OpTxType::Eip8130)
        .expect("AA tx absent from produced block");

    // Spot-check: the on-chain recovered sender matches the wallet that submitted it.
    // For explicit-from AA txs `recover_signer` returns `tx.from` directly, so this
    // mainly proves the envelope round-tripped without losing the `from` field.
    let recovered = aa_tx.recover_signer().expect("AA sender recovery must succeed");
    assert_eq!(recovered, sender);

    Ok(())
}
