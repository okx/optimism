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
// at <dir>/<relPath>, creating parent dirs. relPath is e.g. "XlayerTxBlacklist.sol/TxBlacklistTestable.json".
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

func TestReadTxBlacklistRuntime(t *testing.T) {
	const relPath = "XlayerTxBlacklist.sol/TxBlacklistTestable.json"

	t.Run("valid artifact returns runtime bytes", func(t *testing.T) {
		dir := t.TempDir()
		writeArtifact(t, dir, relPath, "0x6001600101")
		got, err := readTxBlacklistRuntime(dir)
		require.NoError(t, err)
		require.Equal(t, common.FromHex("0x6001600101"), got)
	})

	t.Run("empty artifactsDir is a hard error", func(t *testing.T) {
		_, err := readTxBlacklistRuntime("")
		require.Error(t, err)
	})

	t.Run("missing file is a hard error", func(t *testing.T) {
		_, err := readTxBlacklistRuntime(t.TempDir())
		require.Error(t, err)
	})

	t.Run("malformed JSON is a hard error", func(t *testing.T) {
		dir := t.TempDir()
		full := filepath.Join(dir, relPath)
		require.NoError(t, os.MkdirAll(filepath.Dir(full), 0o755))
		require.NoError(t, os.WriteFile(full, []byte("{not json"), 0o644))
		_, err := readTxBlacklistRuntime(dir)
		require.Error(t, err)
	})

	t.Run("empty runtime object is a hard error", func(t *testing.T) {
		dir := t.TempDir()
		writeArtifact(t, dir, relPath, "0x")
		_, err := readTxBlacklistRuntime(dir)
		require.Error(t, err)
	})
}

func TestInjectXLayerPredeploysWritesBlacklistAndRepins(t *testing.T) {
	dir := t.TempDir()
	// Both artifacts must be present: the injector reads the gasless artifact first, and a
	// configured-but-missing gasless artifact is itself a hard error.
	writeArtifact(t, dir, "XlayerGaslessWhitelist.sol/GaslessWhitelist.json", "0x60016002")
	writeArtifact(t, dir, "XlayerTxBlacklist.sol/TxBlacklistTestable.json", "0x6001600101")

	genesis := &core.Genesis{
		Config:     &params.ChainConfig{ChainID: big.NewInt(195)},
		Difficulty: big.NewInt(0),
		Alloc:      types.GenesisAlloc{},
	}
	l2 := &L2Network{genesis: genesis, rollupCfg: &rollup.Config{}}

	dt := devtest.SerialT(t)
	injectXLayerGaslessPredeploys(dt, l2, dir)

	acct, ok := l2.genesis.Alloc[XLayerTxBlacklist]
	require.True(t, ok, "blacklist account must be injected at the fixed address")
	require.Equal(t, common.FromHex("0x6001600101"), acct.Code)
	require.Equal(t, big.NewInt(0), acct.Balance)
	require.Equal(t, uint64(1), acct.Nonce)
	require.Empty(t, acct.Storage, "blacklist account must start with empty storage")

	// The fixed address must equal the execution-layer devnet constant.
	require.Equal(t, common.HexToAddress("0xb1ac000000000000000000000000000000000001"), XLayerTxBlacklist)

	// Genesis hash/number must be re-pinned to the mutated genesis block (computed once, after both allocs).
	block := l2.genesis.ToBlock()
	require.Equal(t, block.Hash(), l2.rollupCfg.Genesis.L2.Hash)
	require.Equal(t, block.NumberU64(), l2.rollupCfg.Genesis.L2.Number)
}
