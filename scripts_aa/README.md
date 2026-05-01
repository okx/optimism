# XLayerAA (EIP-8130) verification scripts
Manual smoke scripts for validating the XLayerAA tx path.

## Quick start — single EL (--dev mode)
The fastest path: one node, no L1, uses the built-in dev chain.

### build
```
git submodule update --init --recursive

just cannon op-program cannon-prestates
pushd packages/contracts-bedrock && forge build --skip-test && popd
 
# build op-reth and kona-node
cd rust
just build-op-reth-debug
just build-kona-node-debug

# build devstack node
cd ../op-up
just

# build contracts

```

### run tests
1. Run the node (with CL and EL):
```
# in op-up dir

# Required for EIP-8130 AA txs: activate xlayer_v1 hardfork at L2 genesis. Without
# this, the validator's fork gate rejects 0x7B txs as "transaction type not
# supported". Equivalent CLI flag: --fork=xlayer_v1
export OP_UP_FORK=xlayer_v1
export DEVSTACK_L2CL_KIND=kona-node
./bin/op-up # start single devnet（default）


# default is op-reth + op-node, so the following doesn't need to be configured.
# export DEVSTACK_L2EL_KIND=op-reth
# export RUST_BINARY_PATH_OP_RETH=../rust/target/debug/op-reth # default path
# Optional:
# export RUST_BINARY_PATH_KONA_NODE=../rust/target/debug/kona-node # default path
```

2. Test sending an AA tx 
```
cd scripts_aa
npm install
npm run send-k1
```

- op-reth works: if receipt returns
- kona-node works: if `cast bn` still goes up