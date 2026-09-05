package sysgo

import (
	"encoding/json"
	"math/big"
	"os"
	"path/filepath"
	"testing"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/core"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/params"
	"github.com/stretchr/testify/require"

	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
	"github.com/ethereum-optimism/optimism/op-node/rollup"
)

// writeArtifact writes a minimal Forge artifact JSON ({"deployedBytecode":{"object":<hex>}})
// at <dir>/<relPath>, creating parent dirs. Shared by the gasless-side and blacklist-side tests.
func writeArtifact(t *testing.T, dir, relPath, hexObject string) {
	t.Helper()
	full := filepath.Join(dir, relPath)
	require.NoError(t, os.MkdirAll(filepath.Dir(full), 0o755))
	body, err := json.Marshal(map[string]any{
		"deployedBytecode": map[string]any{"object": hexObject},
	})
	require.NoError(t, err)
	require.NoError(t, os.WriteFile(full, body, 0o644))
}

// newXLayerTestL2 builds a minimal in-memory L2Network (no network) for injector/finalization tests.
func newXLayerTestL2() *L2Network {
	return &L2Network{
		genesis: &core.Genesis{
			Config:     &params.ChainConfig{ChainID: big.NewInt(195)},
			Difficulty: big.NewInt(0),
			Alloc:      types.GenesisAlloc{},
		},
		rollupCfg: &rollup.Config{},
	}
}

func TestRepinXLayerGenesisL2HashAfterPredeploys(t *testing.T) {
	dir := t.TempDir()
	// Both artifacts present: gasless is read first; its configured-but-missing artifact is itself an error.
	writeArtifact(t, dir, "XlayerGaslessWhitelist.sol/GaslessWhitelist.json", "0x60016002")
	writeArtifact(t, dir, "XlayerTxBlacklist.sol/TxBlacklistTestable.json", "0x6001600101")

	l2 := newXLayerTestL2()
	dt := devtest.SerialT(t)

	// Gasless first — and it MUST NOT re-pin (else the final re-pin would MASK an accidental
	// gasless-side re-pin and hide an SRP violation). This is the mandatory intermediate assertion.
	injectXLayerGaslessPredeploys(dt, l2, dir)
	require.Equal(t, common.Hash{}, l2.rollupCfg.Genesis.L2.Hash,
		"injectXLayerGaslessPredeploys must not re-pin the genesis hash")

	// Blacklist next — also must not re-pin.
	injectXLayerTxBlacklist(dt, l2, dir)
	require.Equal(t, common.Hash{}, l2.rollupCfg.Genesis.L2.Hash,
		"injectXLayerTxBlacklist must not re-pin the genesis hash")

	// Both predeploys present before finalization.
	_, gOK := l2.genesis.Alloc[XLayerGaslessWhitelistProxy]
	_, bOK := l2.genesis.Alloc[XLayerTxBlacklist]
	require.True(t, gOK, "gasless whitelist must be present before re-pin")
	require.True(t, bOK, "blacklist must be present before re-pin")

	// Finalize exactly once, after both predeploys.
	repinXLayerGenesisL2Hash(l2)
	block := l2.genesis.ToBlock()
	require.Equal(t, block.Hash(), l2.rollupCfg.Genesis.L2.Hash)
	require.Equal(t, block.NumberU64(), l2.rollupCfg.Genesis.L2.Number)
}
