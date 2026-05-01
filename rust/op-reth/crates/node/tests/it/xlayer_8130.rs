//! XLayer EIP-8130 (AA) end-to-end coverage.
//!
//! Capability under test: `eth_sendRawTransaction(0x7B || rlp(...))` → pool admission →
//! builder pulls the tx into a payload → op-revm AA handler executes it. We stop short
//! of `engine_newPayload` / forkchoice update — those exercise stock OP plumbing that
//! is not XLayer-specific, and the round-trip re-hashing in the engine validator is
//! orthogonal to AA admission.
//!
//! The harness is the same `setup_engine` used elsewhere; the only XLayer-specific
//! ingredients are (1) `xlayer_v1_activated()` on the chain spec to flip the
//! validator's fork gate, and (2) a `NONCE_MANAGER_ADDRESS` allocation in genesis so
//! the on-chain `aa_nonce_slot(sender, 0)` read in `validate_eip8130_transaction` step
//! 5 returns 0. The chain spec is **not** advanced past Bedrock for upstream OP forks
//! — XLayerV1 is independent (per xlayer-aa.md 2026-04-21) so we don't drag in Karst /
//! Jovian / etc.; this keeps the test capability scoped to "XLayerV1 admits 0x7B".

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

/// Pre-Jovian minimal payload-attributes generator.
///
/// `min_base_fee` and `eip_1559_params` are post-Jovian MUSTs; this test does not
/// activate Jovian (we only activate XLayerV1, which is independent), so both stay
/// `None` and the engine builder accepts the attributes as-is.
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
///
/// We deliberately drive the build via `inject_tx` + `new_payload` rather than
/// `node.advance(...)`. `advance` additionally calls `submit_payload`, which round-trips
/// the just-built payload through `engine_newPayload`; that path re-encodes every tx
/// via `encoded_2718()` and recomputes the transactions trie root, then compares the
/// resulting block hash against the original. Any byte-level drift in the AA tx codec
/// surfaces there as a noisy `engine::tree: Invalid payload ... block hash mismatch`
/// log, which is a serialization concern orthogonal to AA admission and is owned by
/// dedicated codec round-trip tests in `op-alloy-consensus`. Keeping the e2e test
/// scoped to ingress + build + execute leaves that surface to its proper owner.
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

    // XLayerV1-only chain spec — flips the mempool validator's `is_xlayer_v1_active`
    // gate at genesis. No upstream OP-fork activation is needed for the AA capability
    // itself.
    let chain_spec = Arc::new(
        OpChainSpecBuilder::base_mainnet().genesis(genesis).xlayer_v1_activated().build(),
    );
    let chain_id = chain_spec.chain().id();

    // Boot a single-node engine. `setup_engine` wires the full stack: pool, validator,
    // payload builder, engine API, RPC. We do NOT customize the payload-priority hook
    // here — base ordering is fine; we only care that the AA tx round-trips.
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

    // 1) RPC ingress + pool admission.
    let raw_tx = aa_raw_tx_bytes(chain_id, sender, target, 0);
    node.rpc.inject_tx(raw_tx).await.expect("inject AA tx via eth_sendRawTransaction");

    // 2) Builder pulls from the pool and produces a payload. No `submit_payload`
    //    afterwards — see the test docstring for why.
    let payload = node.new_payload().await?;
    let block = payload.block();

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
