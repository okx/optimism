package derive

import (
	"embed"
	"encoding/hex"
	"fmt"
	"math/big"
	"strings"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/common/hexutil"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/crypto"
)

// Native AA (EIP-8130) deploys the Account Abstraction system contracts at
// hardfork activation via deposit transactions. Mirrors base/crates/consensus/
// upgrades/src/base_v1.rs:
//
//  1. K1Verifier            (no constructor args)
//  2. P256Verifier          (no constructor args)
//  3. WebAuthnVerifier      (no constructor args)
//  4. AccountConfiguration  (constructor: k1, p256, webauthn, delegate=address(0))
//  5. DelegateVerifier      (constructor: accountConfiguration)
//  6. DefaultAccount        (constructor: accountConfiguration)
//
// Deployer addresses extend the 0x4210...000X series used by prior forks
// (Ecotone/Fjord/Isthmus/Jovian); deployed addresses are deterministic via
// `deployer.create(0)`.

//go:embed native_aa_bytecode/*.hex
var nativeAABytecode embed.FS

var (
	// Deployer addresses (mirrors base Deployers::BASE_V1_*).
	NativeAAK1VerifierDeployer           = common.HexToAddress("0x4210000000000000000000000000000000000008")
	NativeAAP256VerifierDeployer         = common.HexToAddress("0x4210000000000000000000000000000000000009")
	NativeAAWebAuthnVerifierDeployer     = common.HexToAddress("0x421000000000000000000000000000000000000a")
	NativeAAAccountConfigurationDeployer = common.HexToAddress("0x421000000000000000000000000000000000000b")
	NativeAADelegateVerifierDeployer     = common.HexToAddress("0x421000000000000000000000000000000000000c")
	NativeAADefaultAccountDeployer       = common.HexToAddress("0x421000000000000000000000000000000000000d")

	// Deployed contract addresses (deployer.create(0)).
	NativeAAK1VerifierAddr           = crypto.CreateAddress(NativeAAK1VerifierDeployer, 0)
	NativeAAP256VerifierAddr         = crypto.CreateAddress(NativeAAP256VerifierDeployer, 0)
	NativeAAWebAuthnVerifierAddr     = crypto.CreateAddress(NativeAAWebAuthnVerifierDeployer, 0)
	NativeAAAccountConfigurationAddr = crypto.CreateAddress(NativeAAAccountConfigurationDeployer, 0)
	NativeAADelegateVerifierAddr     = crypto.CreateAddress(NativeAADelegateVerifierDeployer, 0)
	NativeAADefaultAccountAddr       = crypto.CreateAddress(NativeAADefaultAccountDeployer, 0)

	// Deposit-tx source hashes.
	deployNativeAAK1VerifierSource           = UpgradeDepositSource{Intent: "Native AA: K1 Verifier Deployment"}
	deployNativeAAP256VerifierSource         = UpgradeDepositSource{Intent: "Native AA: P256 Verifier Deployment"}
	deployNativeAAWebAuthnVerifierSource     = UpgradeDepositSource{Intent: "Native AA: WebAuthn Verifier Deployment"}
	deployNativeAAAccountConfigurationSource = UpgradeDepositSource{Intent: "Native AA: Account Configuration Deployment"}
	deployNativeAADelegateVerifierSource     = UpgradeDepositSource{Intent: "Native AA: Delegate Verifier Deployment"}
	deployNativeAADefaultAccountSource       = UpgradeDepositSource{Intent: "Native AA: Default Account Deployment"}
)

// loadNativeAABytecode reads a deployment bytecode hex file from the embedded
// fs and returns its raw bytes.
func loadNativeAABytecode(name string) []byte {
	raw, err := nativeAABytecode.ReadFile("native_aa_bytecode/" + name)
	if err != nil {
		panic(fmt.Errorf("native_aa: failed to load embedded bytecode %s: %w", name, err))
	}
	cleaned := strings.ReplaceAll(string(raw), "\n", "")
	cleaned = strings.TrimPrefix(cleaned, "0x")
	out, err := hex.DecodeString(cleaned)
	if err != nil {
		panic(fmt.Errorf("native_aa: invalid hex in %s: %w", name, err))
	}
	return out
}

// abiAddress returns a 32-byte left-padded ABI encoding of an address.
func abiAddress(addr common.Address) []byte {
	return common.LeftPadBytes(addr.Bytes(), 32)
}

// nativeAAAccountConfigurationInput returns the deployment input for
// AccountConfiguration: creation bytecode + ABI-encoded constructor args
// (k1, p256, webauthn, delegate=address(0)).
//
// Delegate is intentionally address(0) at deployment to break the circular
// dependency (DelegateVerifier needs AccountConfiguration's address).
func nativeAAAccountConfigurationInput() []byte {
	base := loadNativeAABytecode("base-v1-account-configuration-deployment.hex")
	args := make([]byte, 0, 4*32)
	args = append(args, abiAddress(NativeAAK1VerifierAddr)...)
	args = append(args, abiAddress(NativeAAP256VerifierAddr)...)
	args = append(args, abiAddress(NativeAAWebAuthnVerifierAddr)...)
	args = append(args, abiAddress(common.Address{})...) // delegate = address(0)
	return append(base, args...)
}

// nativeAADelegateVerifierInput: creation bytecode + (accountConfig).
func nativeAADelegateVerifierInput() []byte {
	base := loadNativeAABytecode("base-v1-delegate-verifier-deployment.hex")
	return append(base, abiAddress(NativeAAAccountConfigurationAddr)...)
}

// nativeAADefaultAccountInput: creation bytecode + (accountConfig).
func nativeAADefaultAccountInput() []byte {
	base := loadNativeAABytecode("base-v1-default-account-deployment.hex")
	return append(base, abiAddress(NativeAAAccountConfigurationAddr)...)
}

// NativeAANetworkUpgradeTransactions returns the 6 deposit transactions that
// deploy the EIP-8130 system contracts at NativeAA fork activation.
//
// Gas limits mirror base's `BaseV1::deposits()`:
//   - K1Verifier:           200_000
//   - P256Verifier:         800_000
//   - WebAuthnVerifier:   1_100_000
//   - AccountConfiguration: 2_000_000
//   - DelegateVerifier:     200_000
//   - DefaultAccount:       500_000
func NativeAANetworkUpgradeTransactions() ([]hexutil.Bytes, error) {
	specs := []struct {
		name     string
		source   UpgradeDepositSource
		from     common.Address
		gas      uint64
		input    []byte
		inputErr error
	}{
		{
			name:   "K1Verifier",
			source: deployNativeAAK1VerifierSource,
			from:   NativeAAK1VerifierDeployer,
			gas:    200_000,
			input:  loadNativeAABytecode("base-v1-k1-verifier-deployment.hex"),
		},
		{
			name:   "P256Verifier",
			source: deployNativeAAP256VerifierSource,
			from:   NativeAAP256VerifierDeployer,
			gas:    800_000,
			input:  loadNativeAABytecode("base-v1-p256-verifier-deployment.hex"),
		},
		{
			name:   "WebAuthnVerifier",
			source: deployNativeAAWebAuthnVerifierSource,
			from:   NativeAAWebAuthnVerifierDeployer,
			gas:    1_100_000,
			input:  loadNativeAABytecode("base-v1-web-authn-verifier-deployment.hex"),
		},
		{
			name:   "AccountConfiguration",
			source: deployNativeAAAccountConfigurationSource,
			from:   NativeAAAccountConfigurationDeployer,
			gas:    2_000_000,
			input:  nativeAAAccountConfigurationInput(),
		},
		{
			name:   "DelegateVerifier",
			source: deployNativeAADelegateVerifierSource,
			from:   NativeAADelegateVerifierDeployer,
			gas:    200_000,
			input:  nativeAADelegateVerifierInput(),
		},
		{
			name:   "DefaultAccount",
			source: deployNativeAADefaultAccountSource,
			from:   NativeAADefaultAccountDeployer,
			gas:    500_000,
			input:  nativeAADefaultAccountInput(),
		},
	}

	out := make([]hexutil.Bytes, 0, len(specs))
	for _, s := range specs {
		encoded, err := types.NewTx(&types.DepositTx{
			SourceHash:          s.source.SourceHash(),
			From:                s.from,
			To:                  nil,
			Mint:                big.NewInt(0),
			Value:               big.NewInt(0),
			Gas:                 s.gas,
			IsSystemTransaction: false,
			Data:                s.input,
		}).MarshalBinary()
		if err != nil {
			return nil, fmt.Errorf("build %s deploy tx: %w", s.name, err)
		}
		out = append(out, encoded)
	}
	return out, nil
}

// NativeAANetworkUpgradeGas is the total gas budget reserved for the NativeAA
// upgrade block, matching the sum of per-tx gas limits in
// NativeAANetworkUpgradeTransactions. Exposed so the block builder can extend
// the upgrade-block gas cap to fit all six deposit txs.
const NativeAANetworkUpgradeGas uint64 = 200_000 + 800_000 + 1_100_000 + 2_000_000 + 200_000 + 500_000
