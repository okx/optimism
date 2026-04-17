The complete call chain from the actual kona source:

### Step 1 — SequencerActor calls prepare_payload_attributes with unsafe_head (the L2 parent):

```rust
// actors/sequencer/actor.rs:288
self.attributes_builder
.prepare_payload_attributes(unsafe_head, l1_origin.id())
//                              ^^^^^^^^^^^
//                              this is the L2 parent block
```

### Step 2 — prepare_payload_attributes fetches config using l2_parent.block_info.number:

```rust
// derive/src/attributes/stateful.rs:78-82
let mut sys_config = self
.config_fetcher
.system_config_by_number(l2_parent.block_info.number, ...)
//                           ^^^^^^^^^^^^^^^^^^^^^^^^^^
//                           L2 PARENT block number — NOT L1 head
.await?;
```

### Step 3 — system_config_by_number fetches that L2 block and extracts config from it:

// providers-alloy/src/l2_chain_provider.rs:358-364
let block = self.block_by_number(number).await?;   // fetch L2 parent block
let config = to_system_config(&block, &rollup_config)?;  // extract from it

### Step 4 — to_system_config reads config from the L2 block's first deposit tx (L1InfoTx):

```rust
// protocol/src/utils.rs:36-58
let tx = block.body.transactions[0].as_deposit();  // first tx = L1InfoTx
let l1_info = L1BlockInfoTx::decode_calldata(tx.input());  // decode it

let cfg = SystemConfig {
batcher_address: l1_info.batcher_address(),  // embedded when block was built
gas_limit: block.header.gas_limit,           // from block header
scalar: l1_fee_scalar,                       // from L1InfoTx
...
};
```


This is the L2 parent block's already-embedded config. It's historical data, not a live L1 query.

### Step 5 — Config only changes at epoch boundary (l1_origin advances):

```rust
// derive/src/attributes/stateful.rs:88-142
if l2_parent.l1_origin.number == epoch.number {
// SAME EPOCH — 11 of 12 blocks
deposit_transactions = vec![];   // no deposits, no config update
sequence_number = l2_parent.seq_num + 1;
} else {
// EPOCH CHANGE — 1 of 12 blocks
let receipts = self.receipts_fetcher
.receipts_by_hash(epoch.hash).await?;       // fetch L1 receipts
sys_config.update_with_receipts(&receipts, ...); // ← CONFIG CHANGES HERE ONLY
sequence_number = 0;
}
```

### So the staleness concern is a non-issue because:
- Baseline kona reads config from the L2 parent block's embedded L1InfoTx — that's already "stale" by definition (it's the previous block's snapshot)
- Config changes only propagate via update_with_receipts() at epoch boundaries
- Our cache returns the exact same value that to_system_config(l2_parent_block) would return
- Same behaviour in baseline, same behaviour with cache
