# XLayerAA (EIP-8130) verification scripts
Manual smoke scripts for validating the XLayerAA tx path.

## Quick start — single EL (--dev mode)
The fastest path: one node, no L1, uses the built-in dev chain.

### build
```
git submodule update --init --recursive

just cannon op-program cannon-prestates
pushd packages/contracts-bedrock && forge build --skip-test && popd
 
# build op-reth
cd rust
just build-op-reth-debug

# build devstack node
cd ../op-up
just

# build contracts

```

### run tests
1. Run the node:
```
# in op-up dir
export DEVSTACK_L2EL_KIND=op-reth
export OP_RETH_EXEC_PATH=../rust/target/debug/op-reth
# Required for EIP-8130 AA txs: activate Karst hardfork at L2 genesis. Without
# this, the validator's fork gate rejects 0x7B txs as "transaction type not
# supported". Equivalent CLI flag: --fork=karst
export OP_UP_FORK=karst
# Optional：export DEVSTACK_L2CL_KIND=kona-node 等
./bin/op-up # start single devnet（default）
```

2. Test sending an AA tx 
```
cd scripts_aa
npm install
npm run send-k1
```