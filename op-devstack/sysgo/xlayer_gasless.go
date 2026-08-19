package sysgo

import (
	"github.com/ethereum/go-ethereum/common"

	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
)

// XLayer gasless-whitelist predeploy coordinates. These match the addresses the
// XLayer gasless deploy script and devnet genesis rely on.
var (
	// XLayerGaslessDeployFactory is the CREATE2 factory that deploys the gasless
	// whitelist implementation and proxy at deterministic addresses.
	XLayerGaslessDeployFactory = common.HexToAddress("0xFaC897544659Fb136C064d5428947f5BC9cC1Fa2")
	// XLayerGaslessWhitelistProxy is the deterministic devnet address the gasless
	// whitelist proxy is reachable at after deployment.
	XLayerGaslessWhitelistProxy = common.HexToAddress("0xA9092BC02e2000a3F8996D1991621E9A03Ef2dfE")
	// XLayerGaslessOwner is the account set as the gasless whitelist owner and
	// ProxyAdmin owner during initialization.
	XLayerGaslessOwner = common.HexToAddress("0x14dC79964da2C08b23698B3D3cc7Ca32193d9955")
)

// XLayerGaslessDeployer coordinates deploying and initializing the XLayer gasless
// whitelist against a running devnet using the contracts-bedrock forge deploy
// script. It owns the deploy inputs so the scenarios can request a whitelist
// without duplicating the address/owner constants.
type XLayerGaslessDeployer struct {
	// ScriptPath is the path to DeployXlayerGaslessWhitelist.s.sol.
	ScriptPath string
	// Owner becomes the gasless whitelist owner and ProxyAdmin owner.
	Owner common.Address
	// L2RPC is the execution RPC the deploy transactions are sent to.
	L2RPC string
}

// NewXLayerGaslessDeployer builds a deployer for the gasless whitelist deploy
// script, sending deploy transactions to the given L2 execution RPC.
func NewXLayerGaslessDeployer(scriptPath, l2RPC string) *XLayerGaslessDeployer {
	return &XLayerGaslessDeployer{
		ScriptPath: scriptPath,
		Owner:      XLayerGaslessOwner,
		L2RPC:      l2RPC,
	}
}

// ProxyAddress reports the deterministic proxy address the whitelist is deployed
// behind.
func (d *XLayerGaslessDeployer) ProxyAddress() common.Address {
	return XLayerGaslessWhitelistProxy
}

// FactoryAddress reports the CREATE2 factory the deploy script routes through.
func (d *XLayerGaslessDeployer) FactoryAddress() common.Address {
	return XLayerGaslessDeployFactory
}

// Deploy runs the gasless whitelist deploy + initialization against the devnet
// and returns the proxy address once the whitelist is live.
func (d *XLayerGaslessDeployer) Deploy(t devtest.T) common.Address {
	// TODO: invoke the forge script at d.ScriptPath (contract
	// DeployGaslessWhitelist) against d.L2RPC with OWNER=d.Owner through the
	// CREATE2 factory, then wait until the proxy at ProxyAddress() holds code.
	// This requires shelling out to `forge script` (or reusing the op-deployer
	// forge runner) with the contracts-bedrock artifacts on disk; no devstack Go
	// helper wires the XLayer gasless deploy today, so this is the net-new
	// integration point the gasless scenarios depend on.
	t.Logger().Warn("XLayer gasless deploy glue is not yet wired to the forge deploy script",
		"script", d.ScriptPath, "factory", d.FactoryAddress(), "proxy", d.ProxyAddress(), "owner", d.Owner)
	return d.ProxyAddress()
}
