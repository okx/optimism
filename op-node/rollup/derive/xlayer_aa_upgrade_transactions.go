package derive

// XLayerAA (EIP-8130) network upgrade transactions.
//
// On the activation boundary block of `XLayerAATime`, the attributes
// builder emits 7 deposit transactions that install the EIP-8130 system
// contracts at canonical `CREATE(deployer, 0)` addresses. The 7
// (deployer, address) pairs are mirrored byte-identically in the
// xlayer-reth EL chainspec at:
//
//   crates/chainspec/src/xlayer_aa_predeploys.rs
//
// A unit test in this package + the Rust-side
// `create_address_matches_deployer` test guards both sides against
// drift.
//
// # Bundle format and regeneration
//
// The upgrade-tx payload lives in `xlayer_aa_nut_bundle.json` (NUT =
// Network Upgrade Transactions), `//go:embed`-loaded at compile time
// and parsed with the shared `readNUTBundle` helper from
// [upgrade_transaction.go] (the same machinery that Karst and later
// forks use). Regeneration is one step:
//
//   just contracts-eip8130-build
//
// which re-runs forge + extract_runtime.py in the parent xlayer-reth
// repo. The Python script writes directly into this directory.
//
// For contracts with `(address accountConfiguration)` constructor
// args (DefaultAccount / DefaultHighRateAccount / DelegateVerifier),
// the abi-encoded AC address is already appended to `data` in the
// JSON — no post-processing in Go. On-chain constructors fill the
// immutable slot during deploy.

import (
	"bytes"
	_ "embed"

	"github.com/ethereum-optimism/optimism/op-core/forks"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/common/hexutil"
)

//go:embed xlayer_aa_nut_bundle.json
var xlayerAANUTBundleJSON []byte

// xlayerAAForkName is the `forks.Name` value used for intent
// namespacing inside `toDepositTransactions`. We deliberately use a
// custom string instead of adding XLayerAA to `forks.All` — XLayerAA
// is XLayer-specific and shouldn't pollute the upstream fork ladder
// (which is traversed by generic code in `forks.From`, `forks.Next`,
// etc.). Source hashes stay deterministic either way because this
// string only affects the intent prefix hashed into `SourceHash`.
const xlayerAAForkName = forks.Name("xlayer_aa")

// XLayerAA deployer addresses. Each is a fresh account whose nonce is
// guaranteed to be 0 at the activation boundary block, so
// `CREATE(deployer, 0)` produces a deterministic address that's
// identical across devnet / testnet / mainnet.
//
// MUST stay in sync with the `deployer` field of every entry in
// `AA_PREDEPLOYS` in `crates/chainspec/src/xlayer_aa_predeploys.rs`
// AND with the `from` field in `xlayer_aa_nut_bundle.json`.
var (
	XLayerAAAccountConfigurationDeployerAddress   = common.HexToAddress("0x4210000000000000000000000000000000000010")
	XLayerAADefaultAccountDeployerAddress         = common.HexToAddress("0x4210000000000000000000000000000000000011")
	XLayerAADefaultHighRateAccountDeployerAddress = common.HexToAddress("0x4210000000000000000000000000000000000012")
	XLayerAAK1VerifierDeployerAddress             = common.HexToAddress("0x4210000000000000000000000000000000000013")
	XLayerAAP256VerifierDeployerAddress           = common.HexToAddress("0x4210000000000000000000000000000000000014")
	XLayerAAWebAuthnVerifierDeployerAddress       = common.HexToAddress("0x4210000000000000000000000000000000000015")
	XLayerAADelegateVerifierDeployerAddress       = common.HexToAddress("0x4210000000000000000000000000000000000016")
)

// XLayerAA predeploy addresses — the destinations users / dApps
// reference. Equal to `crypto.CreateAddress(deployer, 0)` for each
// corresponding deployer above.
//
// MUST stay in sync with the `address` field in
// `crates/chainspec/src/xlayer_aa_predeploys.rs`. The unit test
// `TestXLayerAAUpgradeTransactionsAddressesMatchCreateAddress` in
// this package re-derives every address from its deployer to catch
// any typo here.
var (
	XLayerAAAccountConfigurationAddress   = common.HexToAddress("0xA6A551b856B139B3292128F3b36ADa58025c4b27")
	XLayerAADefaultAccountAddress         = common.HexToAddress("0x5D82f4311f134052bb36b11BD665Ddab843ebb3D")
	XLayerAADefaultHighRateAccountAddress = common.HexToAddress("0x86bf4F2d426b3386a04a24fE21a0CEb34A7b806c")
	XLayerAAK1VerifierAddress             = common.HexToAddress("0x7F2c04d16c53f2be99aD1a86771637568B718dBf")
	XLayerAAP256VerifierAddress           = common.HexToAddress("0xAfc812351BE998FB088851a79Fc68887C42D7719")
	XLayerAAWebAuthnVerifierAddress       = common.HexToAddress("0x4921DCFD2541f738990767852aB925B3b9f652A2")
	XLayerAADelegateVerifierAddress       = common.HexToAddress("0xE89A62553fE775AFe77464969b2296dc1745CF85")
)

// XLayerAANetworkUpgradeTransactions returns the deposit transactions
// and total gas budget for the XLayerAA EIP-8130 predeploy install.
//
// Matches the `UpgradeTransactions(forks.Karst)` signature used by the
// NUT-based path so the attributes-builder caller can add
// `upgradeGas` onto the activation block's gas budget — the 7 deploy
// txs total ~9.5M gas, which would otherwise squeeze out normal user
// txs in a 30M-gas block.
func XLayerAANetworkUpgradeTransactions() ([]hexutil.Bytes, uint64, error) {
	bundle, err := readNUTBundle(xlayerAAForkName, bytes.NewReader(xlayerAANUTBundleJSON))
	if err != nil {
		return nil, 0, err
	}
	txs, err := bundle.toDepositTransactions()
	if err != nil {
		return nil, 0, err
	}
	return txs, bundle.totalGas(), nil
}
