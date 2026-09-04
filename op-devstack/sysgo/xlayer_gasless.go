package sysgo

import (
	"encoding/json"
	"fmt"
	"math/big"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"

	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
)

// XLayer gasless-whitelist predeploy coordinates. These match the addresses the
// XLayer gasless deploy script and devnet genesis rely on.
var (
	// XLayerGaslessDeployFactory is the CREATE2 factory that deploys the gasless
	// whitelist implementation and proxy at deterministic addresses. It is
	// injected into the L2 genesis (see injectXLayerGaslessFactory) so the deploy
	// script can route CREATE2 through it on a freshly started devnet.
	XLayerGaslessDeployFactory = common.HexToAddress("0xFaC897544659Fb136C064d5428947f5BC9cC1Fa2")
	// XLayerGaslessWhitelistProxy is the deterministic devnet address the gasless
	// whitelist proxy is reachable at after deployment with the canonical compiler.
	// It is the fallback the deployer reports when the actual deployed address
	// could not be parsed from the deploy output.
	XLayerGaslessWhitelistProxy = common.HexToAddress("0xA9092BC02e2000a3F8996D1991621E9A03Ef2dfE")
	// XLayerGaslessOwner is the account set as the gasless whitelist owner and
	// ProxyAdmin owner during initialization.
	XLayerGaslessOwner = common.HexToAddress("0x14dC79964da2C08b23698B3D3cc7Ca32193d9955")
)

// xlayerGaslessFactoryCode is the runtime bytecode of the CREATE2 deploy factory
// installed at XLayerGaslessDeployFactory. A gasless devnet must have this
// contract present at genesis: the deploy script computes the whitelist
// implementation/proxy addresses through the factory's getAddress/deploy entry
// points, so an empty account there makes the script revert with "call to
// non-contract address".
const xlayerGaslessFactoryCode = "0x608060405234801561001057600080fd5b50600436106100365760003560e01c806348aac3921461003b5780634af63f021461006a575b600080fd5b61004e610049366004610121565b61007d565b6040516001600160a01b03909116815260200160405180910390f35b61004e610078366004610121565b61009a565b6000610091828480519060200120306100e1565b90505b92915050565b6000806100a7848461007d565b90506001600160a01b0381163b80156100c257509050610094565b838551602087016000f59250823b6100d957600080fd5b505092915050565b6000604051836040820152846020820152828152600b8101905060ff815360559020949350505050565b634e487b7160e01b600052604160045260246000fd5b6000806040838503121561013457600080fd5b823567ffffffffffffffff8082111561014c57600080fd5b818501915085601f83011261016057600080fd5b8135818111156101725761017261010b565b604051601f8201601f19908116603f0116810190838211818310171561019a5761019a61010b565b816040528281528860208487010111156101b357600080fd5b82602086016020830137600060209382018401529896909101359650505050505056fea26469706673582212207efb8411ab4c4aeb13e72a8aea9f2fe48f1781b601d9e456504a25bc66c61dc964736f6c63430008110033"

// xlayerGaslessProxyLogPattern extracts the deployed proxy address the deploy
// script emits via `console2.log("GaslessWhitelist proxy:", proxy)`. The script
// recomputes the CREATE2 addresses from the actual init code, so the address is
// authoritative even when the compiler version shifts it away from the canonical
// constant.
var xlayerGaslessProxyLogPattern = regexp.MustCompile(`(?i)GaslessWhitelist proxy:\s*(0x[0-9a-fA-F]{40})`)

// xlayerGaslessWhitelistArtifact is the Forge artifact (relative to the local
// contract artifacts directory) whose runtime bytecode is installed at the
// gasless whitelist predeploy address.
const xlayerGaslessWhitelistArtifact = "XlayerGaslessWhitelist.sol/GaslessWhitelist.json"

// XLayerTxBlacklist is the fixed devnet address the execution layer performs the
// isBlacklisted(bytes32) Force-Tx system call against. It MUST equal the execution-layer
// constant XLAYER_DEVNET_BLACKLIST_CONTRACT in
// rust/alloy-op-evm/src/block/xlayer_blacklist_contract.rs (devnet chain id 195).
var XLayerTxBlacklist = common.HexToAddress("0xb1ac000000000000000000000000000000000001")

// xlayerTxBlacklistArtifact is the Forge artifact (relative to the local contract
// artifacts directory) whose runtime bytecode is installed at XLayerTxBlacklist.
const xlayerTxBlacklistArtifact = "XlayerTxBlacklist.sol/TxBlacklistTestable.json"

// readTxBlacklistRuntime loads the compiled TxBlacklistTestable runtime bytecode from the
// local Forge artifacts directory. Unlike readGaslessWhitelistRuntime, every failure is a HARD
// error: XLOP-1195 requires the injector to fail loudly and stop startup — never silently skip —
// when the blacklist artifact is missing, unreadable, malformed, or carries empty runtime
// bytecode. Errors include the artifact path so the failing stage is diagnosable.
func readTxBlacklistRuntime(artifactsDir string) ([]byte, error) {
	if artifactsDir == "" {
		return nil, fmt.Errorf("txblacklist: local contract artifacts dir is required")
	}
	path := filepath.Join(artifactsDir, xlayerTxBlacklistArtifact)
	raw, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("txblacklist: read artifact %s: %w", path, err)
	}
	var artifact struct {
		DeployedBytecode struct {
			Object string `json:"object"`
		} `json:"deployedBytecode"`
	}
	if err := json.Unmarshal(raw, &artifact); err != nil {
		return nil, fmt.Errorf("txblacklist: parse artifact %s: %w", path, err)
	}
	runtime := common.FromHex(artifact.DeployedBytecode.Object)
	if len(runtime) == 0 {
		return nil, fmt.Errorf("txblacklist: empty runtime bytecode in artifact %s", path)
	}
	return runtime, nil
}

// readGaslessWhitelistRuntime loads the compiled GaslessWhitelist runtime
// bytecode from the local Forge artifacts directory. It returns nil (and no
// error) when the artifacts directory is not configured; a configured-but-broken
// artifact is a hard error so a gasless devnet fails loudly rather than starting
// without an enforceable whitelist.
func readGaslessWhitelistRuntime(artifactsDir string) ([]byte, error) {
	if artifactsDir == "" {
		return nil, nil
	}
	raw, err := os.ReadFile(filepath.Join(artifactsDir, xlayerGaslessWhitelistArtifact))
	if err != nil {
		return nil, err
	}
	var artifact struct {
		DeployedBytecode struct {
			Object string `json:"object"`
		} `json:"deployedBytecode"`
	}
	if err := json.Unmarshal(raw, &artifact); err != nil {
		return nil, err
	}
	return common.FromHex(artifact.DeployedBytecode.Object), nil
}

// injectXLayerGaslessPredeploys installs the XLayer system predeploys into the L2
// genesis so the execution client enforces gasless rules, the deploy script has a
// live CREATE2 factory to route through, and the Force-Tx blacklist system call
// resolves to real code at block 0:
//
//   - the CREATE2 deploy factory at XLayerGaslessDeployFactory,
//   - the GaslessWhitelist runtime at XLayerGaslessWhitelistProxy, which is the
//     fixed address the execution client reads gasless rules from. The
//     implementation's constructor disables initializers, but that only runs on a
//     real deployment; a genesis-placed runtime starts with zeroed storage, so a
//     test can call initialize() on it directly, and
//   - the TxBlacklistTestable runtime at XLayerTxBlacklist, the fixed devnet address
//     the execution client performs the isBlacklisted(bytes32) Force-Tx system call
//     against. Unlike the gasless whitelist (which is lenient), a missing/unreadable/
//     malformed/empty blacklist artifact is a hard error that stops startup (Jira
//     XLOP-1195).
//
// Because adding accounts changes the genesis state root (and therefore the
// genesis block hash), it re-derives the rollup config's genesis L2 hash — once,
// after all predeploys are written — so the consensus client still accepts the
// execution client's genesis block. It is safe to call on any XLayer topology; a
// devnet that never uses gasless simply carries extra, unused predeploys.
func injectXLayerGaslessPredeploys(t devtest.T, l2 *L2Network, artifactsDir string) {
	if l2 == nil || l2.genesis == nil {
		return
	}
	if l2.genesis.Alloc == nil {
		l2.genesis.Alloc = make(types.GenesisAlloc)
	}

	l2.genesis.Alloc[XLayerGaslessDeployFactory] = types.Account{
		Code:    common.FromHex(xlayerGaslessFactoryCode),
		Balance: big.NewInt(0),
		Nonce:   1,
	}

	whitelistRuntime, err := readGaslessWhitelistRuntime(artifactsDir)
	t.Require().NoError(err, "read gasless whitelist runtime bytecode")
	if len(whitelistRuntime) > 0 {
		l2.genesis.Alloc[XLayerGaslessWhitelistProxy] = types.Account{
			Code:    whitelistRuntime,
			Balance: big.NewInt(0),
			Nonce:   1,
		}
	} else {
		t.Logger().Warn("gasless whitelist runtime unavailable; gasless enforcement not installed at genesis",
			"artifactsDir", artifactsDir)
	}

	// Install the Force-Tx blacklist runtime at the fixed devnet address so the execution client's
	// isBlacklisted(bytes32) system call resolves to real code at block 0. Strict by Jira XLOP-1195:
	// a missing/unreadable/malformed/empty artifact is a hard error that stops startup.
	blacklistRuntime, err := readTxBlacklistRuntime(artifactsDir)
	t.Require().NoError(err, "read txblacklist runtime bytecode")
	l2.genesis.Alloc[XLayerTxBlacklist] = types.Account{
		Code:    blacklistRuntime,
		Balance: big.NewInt(0),
		Nonce:   1,
	}

	// Re-pin the rollup config's genesis hash to the mutated genesis block so the
	// consensus client's expected L2 genesis hash matches the execution client's.
	if l2.rollupCfg != nil {
		block := l2.genesis.ToBlock()
		l2.rollupCfg.Genesis.L2.Hash = block.Hash()
		l2.rollupCfg.Genesis.L2.Number = block.NumberU64()
	}
}

// XLayerGaslessDeployer coordinates deploying and initializing the XLayer gasless
// whitelist against a running devnet using the contracts-bedrock forge deploy
// script. It owns the deploy inputs so the scenarios can request a whitelist
// without duplicating the address/owner constants.
type XLayerGaslessDeployer struct {
	// ScriptPath is the path to DeployXlayerGaslessWhitelist.s.sol.
	ScriptPath string
	// ContractsRoot is the contracts-bedrock directory the forge script runs from.
	ContractsRoot string
	// Owner becomes the gasless whitelist owner and ProxyAdmin owner.
	Owner common.Address
	// L2RPC is the execution RPC the deploy transactions are broadcast to.
	L2RPC string
	// deployedProxy is the proxy address parsed from the deploy output. It is the
	// authoritative address for the running devnet; it can differ from the
	// canonical constant when the compiler version shifts the CREATE2 result.
	deployedProxy common.Address
}

// NewXLayerGaslessDeployer builds a deployer for the gasless whitelist deploy
// script, broadcasting deploy transactions to the given L2 execution RPC from
// the contracts-bedrock root.
func NewXLayerGaslessDeployer(scriptPath, contractsRoot, l2RPC string) *XLayerGaslessDeployer {
	return &XLayerGaslessDeployer{
		ScriptPath:    scriptPath,
		ContractsRoot: contractsRoot,
		Owner:         XLayerGaslessOwner,
		L2RPC:         l2RPC,
	}
}

// ProxyAddress reports the proxy address the whitelist is deployed behind. After
// a successful Deploy it returns the address parsed from the deploy output;
// before that (or if parsing failed) it falls back to the canonical constant.
func (d *XLayerGaslessDeployer) ProxyAddress() common.Address {
	if d.deployedProxy != (common.Address{}) {
		return d.deployedProxy
	}
	return XLayerGaslessWhitelistProxy
}

// FactoryAddress reports the CREATE2 factory the deploy script routes through.
func (d *XLayerGaslessDeployer) FactoryAddress() common.Address {
	return XLayerGaslessDeployFactory
}

// Deploy runs the gasless whitelist deploy + initialization against the devnet
// by broadcasting the forge deploy script with the given funded broadcaster key,
// setting OWNER to the configured whitelist owner. It parses the deployed proxy
// address from the script output and returns it.
func (d *XLayerGaslessDeployer) Deploy(t devtest.T, broadcasterKeyHex string) common.Address {
	require := t.Require()
	require.NotEmpty(d.ScriptPath, "gasless deploy script path is required")
	require.NotEmpty(d.ContractsRoot, "gasless deploy contracts-bedrock root is required")
	require.NotEmpty(d.L2RPC, "gasless deploy L2 RPC is required")
	require.NotEmpty(broadcasterKeyHex, "gasless deploy broadcaster key is required")

	cmd := exec.CommandContext(t.Ctx(), "forge", "script", d.ScriptPath,
		"--rpc-url", d.L2RPC,
		"--broadcast",
		"--private-key", broadcasterKeyHex,
	)
	cmd.Dir = d.ContractsRoot
	cmd.Env = append(os.Environ(),
		"OWNER="+d.Owner.Hex(),
		"FOUNDRY_DISABLE_NIGHTLY_WARNING=1",
	)
	out, err := cmd.CombinedOutput()
	require.NoErrorf(err, "gasless whitelist forge deploy failed: %s", string(out))

	if match := xlayerGaslessProxyLogPattern.FindSubmatch(out); match != nil {
		d.deployedProxy = common.HexToAddress(string(match[1]))
	}
	t.Logger().Info("Deployed XLayer gasless whitelist", "proxy", d.ProxyAddress(), "owner", d.Owner)
	return d.ProxyAddress()
}
