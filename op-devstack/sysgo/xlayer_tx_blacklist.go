package sysgo

import (
	"encoding/json"
	"fmt"
	"math/big"
	"os"
	"path/filepath"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core/types"

	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
)

// XLayerTxBlacklist is the fixed devnet address the execution layer performs the
// isBlacklisted(bytes32) Force-Tx system call against. It MUST equal the execution-layer
// constant XLAYER_DEVNET_BLACKLIST_CONTRACT in
// rust/alloy-op-evm/src/block/xlayer_blacklist_contract.rs (devnet chain id 195).
var XLayerTxBlacklist = common.HexToAddress("0xb1ac000000000000000000000000000000000001")

// xlayerTxBlacklistArtifact is the Forge artifact (relative to the local contract artifacts
// directory) whose runtime bytecode is installed at XLayerTxBlacklist.
const xlayerTxBlacklistArtifact = "XlayerTxBlacklist.sol/TxBlacklistTestable.json"

// readTxBlacklistRuntime loads the compiled TxBlacklistTestable runtime bytecode from the local
// Forge artifacts directory. Unlike readGaslessWhitelistRuntime, every failure is a HARD error:
// XLOP-1195 requires the injector to fail loudly and stop startup — never silently skip — when the
// blacklist artifact is missing, unreadable, malformed, or carries empty runtime bytecode. Errors
// include the artifact path so the failing stage is diagnosable.
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

// injectXLayerTxBlacklist installs the Force-Tx blacklist runtime at the fixed devnet address
// XLayerTxBlacklist so the execution client's isBlacklisted(bytes32) system call resolves to real
// code at block 0. It writes ONLY the blacklist account; genesis-hash finalization is the caller's
// responsibility (repinXLayerGenesisL2Hash), invoked once after all predeploys. Strict by Jira
// XLOP-1195: a missing/unreadable/malformed/empty artifact is a hard error that stops startup.
func injectXLayerTxBlacklist(t devtest.T, l2 *L2Network, artifactsDir string) {
	if l2 == nil || l2.genesis == nil {
		return
	}
	if l2.genesis.Alloc == nil {
		l2.genesis.Alloc = make(types.GenesisAlloc)
	}

	blacklistRuntime, err := readTxBlacklistRuntime(artifactsDir)
	t.Require().NoError(err, "read txblacklist runtime bytecode")
	l2.genesis.Alloc[XLayerTxBlacklist] = types.Account{
		Code:    blacklistRuntime,
		Balance: big.NewInt(0),
		Nonce:   1,
	}
}
