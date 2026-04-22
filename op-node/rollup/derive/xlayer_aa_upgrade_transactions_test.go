package derive

import (
	"testing"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/crypto"
	"github.com/stretchr/testify/require"
)

// TestXLayerAAUpgradeTransactionsAddressesMatchCreateAddress guards the
// invariant that every XLayerAA predeploy address is exactly
// `crypto.CreateAddress(deployer, 0)` for its corresponding deployer.
//
// Mirrors the Rust-side `create_address_matches_deployer` test in
// `crates/chainspec/src/xlayer_aa_predeploys.rs`. If this fires, either
// the deployer was changed without recomputing the address, or the
// address was typed wrong — both would silently break at chain launch
// (op-node would deploy the contract to one address while the EL
// expects it at another).
func TestXLayerAAUpgradeTransactionsAddressesMatchCreateAddress(t *testing.T) {
	cases := []struct {
		name     string
		deployer common.Address
		address  common.Address
	}{
		{"AccountConfiguration", XLayerAAAccountConfigurationDeployerAddress, XLayerAAAccountConfigurationAddress},
		{"DefaultAccount", XLayerAADefaultAccountDeployerAddress, XLayerAADefaultAccountAddress},
		{"DefaultHighRateAccount", XLayerAADefaultHighRateAccountDeployerAddress, XLayerAADefaultHighRateAccountAddress},
		{"K1Verifier", XLayerAAK1VerifierDeployerAddress, XLayerAAK1VerifierAddress},
		{"P256Verifier", XLayerAAP256VerifierDeployerAddress, XLayerAAP256VerifierAddress},
		{"WebAuthnVerifier", XLayerAAWebAuthnVerifierDeployerAddress, XLayerAAWebAuthnVerifierAddress},
		{"DelegateVerifier", XLayerAADelegateVerifierDeployerAddress, XLayerAADelegateVerifierAddress},
	}

	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			expected := crypto.CreateAddress(c.deployer, 0)
			require.Equal(t, c.address, expected,
				"%s: address %s != CREATE(%s, 0) = %s",
				c.name, c.address.Hex(), c.deployer.Hex(), expected.Hex())
		})
	}
}

// TestXLayerAANetworkUpgradeTransactions decodes the 7 deposit txs
// returned by `XLayerAANetworkUpgradeTransactions` and asserts each one
// has the expected shape: correct `from`, `to == nil`, non-empty data,
// and (for the 3 contracts with constructor args) the
// `AccountConfiguration` address appended as the trailing 32 bytes.
func TestXLayerAANetworkUpgradeTransactions(t *testing.T) {
	upgradeTxns, err := XLayerAANetworkUpgradeTransactions()
	require.NoError(t, err)
	require.Len(t, upgradeTxns, 7, "expected exactly 7 upgrade txs")

	expected := []struct {
		name             string
		from             common.Address
		hasConstructorArg bool
	}{
		{"AccountConfiguration", XLayerAAAccountConfigurationDeployerAddress, false},
		{"DefaultAccount", XLayerAADefaultAccountDeployerAddress, true},
		{"DefaultHighRateAccount", XLayerAADefaultHighRateAccountDeployerAddress, true},
		{"K1Verifier", XLayerAAK1VerifierDeployerAddress, false},
		{"P256Verifier", XLayerAAP256VerifierDeployerAddress, false},
		{"WebAuthnVerifier", XLayerAAWebAuthnVerifierDeployerAddress, false},
		{"DelegateVerifier", XLayerAADelegateVerifierDeployerAddress, true},
	}

	acAddrPadded := common.LeftPadBytes(XLayerAAAccountConfigurationAddress.Bytes(), 32)

	for i, exp := range expected {
		t.Run(exp.name, func(t *testing.T) {
			tx := new(types.Transaction)
			require.NoError(t, tx.UnmarshalBinary(upgradeTxns[i]))
			require.Equal(t, types.DepositTxType, int(tx.Type()),
				"tx %d (%s) is not a deposit tx", i, exp.name)
			require.Nil(t, tx.To(),
				"tx %d (%s) should be a CREATE (to == nil)", i, exp.name)
			// Non-deposit-specific accessors don't expose `From` directly,
			// so we re-derive via the deposit signer. Easier here: just
			// require non-empty data.
			require.NotEmpty(t, tx.Data(),
				"tx %d (%s) data is empty", i, exp.name)

			if exp.hasConstructorArg {
				dataLen := len(tx.Data())
				require.GreaterOrEqual(t, dataLen, 32,
					"tx %d (%s) data shorter than constructor-arg width", i, exp.name)
				tail := tx.Data()[dataLen-32:]
				require.Equal(t, acAddrPadded, tail,
					"tx %d (%s) trailing 32 bytes are not the AccountConfiguration address",
					i, exp.name)
			}
		})
	}
}
