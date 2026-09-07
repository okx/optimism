package sysgo

import (
	"math/big"
	"os"
	"path/filepath"
	"testing"

	"github.com/ethereum/go-ethereum/common"
	"github.com/stretchr/testify/require"

	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
)

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

func TestInjectXLayerTxBlacklist(t *testing.T) {
	dir := t.TempDir()
	writeArtifact(t, dir, "XlayerTxBlacklist.sol/TxBlacklistTestable.json", "0x6001600101")

	l2 := newXLayerTestL2()
	dt := devtest.SerialT(t)
	injectXLayerTxBlacklist(dt, l2, dir)

	acct, ok := l2.genesis.Alloc[XLayerTxBlacklist]
	require.True(t, ok, "blacklist account must be injected at the fixed address")
	require.Equal(t, common.FromHex("0x6001600101"), acct.Code)
	require.Equal(t, big.NewInt(0), acct.Balance)
	require.Equal(t, uint64(1), acct.Nonce)
	require.Empty(t, acct.Storage, "blacklist account must start with empty storage")
	require.Equal(t, common.HexToAddress("0xb1ac000000000000000000000000000000000001"), XLayerTxBlacklist)

	// SRP guard: the blacklist injector alone MUST NOT re-pin the genesis hash (finalization is
	// the caller's job via repinXLayerGenesisL2Hash).
	require.Equal(t, common.Hash{}, l2.rollupCfg.Genesis.L2.Hash,
		"injectXLayerTxBlacklist must not re-pin the genesis hash")
	require.Equal(t, uint64(0), l2.rollupCfg.Genesis.L2.Number)
}
