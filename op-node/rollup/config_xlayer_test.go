package rollup

import (
	"encoding/json"
	"math/big"
	"os"
	"testing"

	"github.com/ethereum/go-ethereum/common"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"

	"github.com/ethereum-optimism/optimism/op-service/eth"
)

func TestXLayerHardcodedForks(t *testing.T) {
	t.Run("XLayerMainnetChainID", func(t *testing.T) {
		cfg, exists := XLayerHardcodedForks[XLayerMainnetChainID]
		require.True(t, exists, "X Layer mainnet config should exist")
		assert.Equal(t, uint64(XLayerMainnetChainID), cfg.ChainID)
		assert.Equal(t, "xlayer-mainnet", cfg.NetworkName)
		assert.NotNil(t, cfg.JovianTime)
		assert.Equal(t, uint64(1764691201), *cfg.JovianTime)
	})

	t.Run("XLayerTestnetChainID", func(t *testing.T) {
		cfg, exists := XLayerHardcodedForks[XLayerTestnetChainID]
		require.True(t, exists, "X Layer testnet config should exist")
		assert.Equal(t, uint64(XLayerTestnetChainID), cfg.ChainID)
		assert.Equal(t, "xlayer-testnet", cfg.NetworkName)
		assert.NotNil(t, cfg.JovianTime)
		assert.Equal(t, uint64(1764327600), *cfg.JovianTime)
	})
}

func TestApplyXLayerHardcodedForks(t *testing.T) {
	t.Run("nil config", func(t *testing.T) {
		result := ApplyXLayerHardcodedForks(nil)
		assert.Nil(t, result)
	})

	t.Run("nil L2ChainID", func(t *testing.T) {
		cfg := &Config{
			L2ChainID: nil,
		}
		result := ApplyXLayerHardcodedForks(cfg)
		assert.Equal(t, cfg, result)
	})

	t.Run("non-XLayer chain", func(t *testing.T) {
		jovianTime := uint64(100)
		cfg := &Config{
			L2ChainID:  big.NewInt(10), // OP Mainnet
			JovianTime: &jovianTime,
		}
		result := ApplyXLayerHardcodedForks(cfg)
		assert.Equal(t, cfg, result)
		assert.Equal(t, uint64(100), *result.JovianTime)
	})

	t.Run("XLayer mainnet - no existing JovianTime", func(t *testing.T) {
		cfg := &Config{
			L2ChainID:  big.NewInt(XLayerMainnetChainID),
			JovianTime: nil,
		}
		result := ApplyXLayerHardcodedForks(cfg)
		require.NotNil(t, result)
		require.NotNil(t, result.JovianTime)
		assert.Equal(t, uint64(1764691201), *result.JovianTime)
	})

	t.Run("XLayer mainnet - override existing JovianTime", func(t *testing.T) {
		jovianTime := uint64(100) // Different value
		cfg := &Config{
			L2ChainID:  big.NewInt(XLayerMainnetChainID),
			JovianTime: &jovianTime,
		}
		result := ApplyXLayerHardcodedForks(cfg)
		require.NotNil(t, result)
		require.NotNil(t, result.JovianTime)
		assert.Equal(t, uint64(1764691201), *result.JovianTime)
	})

	t.Run("XLayer mainnet - same JovianTime", func(t *testing.T) {
		jovianTime := uint64(1764691201) // Same value
		cfg := &Config{
			L2ChainID:  big.NewInt(XLayerMainnetChainID),
			JovianTime: &jovianTime,
		}
		result := ApplyXLayerHardcodedForks(cfg)
		require.NotNil(t, result)
		require.NotNil(t, result.JovianTime)
		assert.Equal(t, uint64(1764691201), *result.JovianTime)
	})

	t.Run("XLayer testnet - no existing JovianTime", func(t *testing.T) {
		cfg := &Config{
			L2ChainID:  big.NewInt(XLayerTestnetChainID),
			JovianTime: nil,
		}
		result := ApplyXLayerHardcodedForks(cfg)
		require.NotNil(t, result)
		require.NotNil(t, result.JovianTime)
		assert.Equal(t, uint64(1764327600), *result.JovianTime)
	})

	t.Run("XLayer testnet - override existing JovianTime", func(t *testing.T) {
		jovianTime := uint64(200) // Different value
		cfg := &Config{
			L2ChainID:  big.NewInt(XLayerTestnetChainID),
			JovianTime: &jovianTime,
		}
		result := ApplyXLayerHardcodedForks(cfg)
		require.NotNil(t, result)
		require.NotNil(t, result.JovianTime)
		assert.Equal(t, uint64(1764327600), *result.JovianTime)
	})

	t.Run("XLayer testnet - same JovianTime", func(t *testing.T) {
		jovianTime := uint64(1764327600) // Same value
		cfg := &Config{
			L2ChainID:  big.NewInt(XLayerTestnetChainID),
			JovianTime: &jovianTime,
		}
		result := ApplyXLayerHardcodedForks(cfg)
		require.NotNil(t, result)
		require.NotNil(t, result.JovianTime)
		assert.Equal(t, uint64(1764327600), *result.JovianTime)
	})
}

func TestApplyXLayerHardcodedForksIntegration(t *testing.T) {
	t.Run("XLayer mainnet with full config", func(t *testing.T) {
		cfg := &Config{
			Genesis: Genesis{
				L1: eth.BlockID{
					Hash:   common.HexToHash("0x1234"),
					Number: 100,
				},
				L2: eth.BlockID{
					Hash:   common.HexToHash("0x5678"),
					Number: 0,
				},
				L2Time: 1000,
			},
			BlockTime:              2,
			MaxSequencerDrift:      600,
			SeqWindowSize:          3600,
			ChannelTimeoutBedrock:  300,
			L1ChainID:              big.NewInt(1),
			L2ChainID:              big.NewInt(XLayerMainnetChainID),
			BatchInboxAddress:      common.HexToAddress("0x1234"),
			DepositContractAddress: common.HexToAddress("0x5678"),
			L1SystemConfigAddress:  common.HexToAddress("0x9abc"),
			JovianTime:             newUint64(999), // Will be overridden
		}

		result := ApplyXLayerHardcodedForks(cfg)
		require.NotNil(t, result)
		assert.Equal(t, cfg.Genesis, result.Genesis)
		assert.Equal(t, cfg.BlockTime, result.BlockTime)
		assert.Equal(t, uint64(1764691201), *result.JovianTime)
	})

	t.Run("XLayer testnet with full config", func(t *testing.T) {
		cfg := &Config{
			Genesis: Genesis{
				L1: eth.BlockID{
					Hash:   common.HexToHash("0xabcd"),
					Number: 200,
				},
				L2: eth.BlockID{
					Hash:   common.HexToHash("0xef01"),
					Number: 0,
				},
				L2Time: 2000,
			},
			BlockTime:              1,
			MaxSequencerDrift:      600,
			SeqWindowSize:          3600,
			ChannelTimeoutBedrock:  300,
			L1ChainID:              big.NewInt(11155111),
			L2ChainID:              big.NewInt(XLayerTestnetChainID),
			BatchInboxAddress:      common.HexToAddress("0xabcd"),
			DepositContractAddress: common.HexToAddress("0xef01"),
			L1SystemConfigAddress:  common.HexToAddress("0x2345"),
			JovianTime:             nil, // Will be set
		}

		result := ApplyXLayerHardcodedForks(cfg)
		require.NotNil(t, result)
		assert.Equal(t, cfg.Genesis, result.Genesis)
		assert.Equal(t, cfg.BlockTime, result.BlockTime)
		assert.Equal(t, uint64(1764327600), *result.JovianTime)
	})
}

func TestApplyXLayerHardcodedForksEdgeCases(t *testing.T) {
	t.Run("XLayer chain with nil JovianTime in config and nil in hardcoded", func(t *testing.T) {
		// Create a temporary config with nil JovianTime
		// This tests the else branch where xlayerForks.JovianTime is nil
		// We need to temporarily modify XLayerHardcodedForks for this test
		originalMainnet := XLayerHardcodedForks[XLayerMainnetChainID]
		defer func() {
			XLayerHardcodedForks[XLayerMainnetChainID] = originalMainnet
		}()

		// Temporarily set JovianTime to nil
		XLayerHardcodedForks[XLayerMainnetChainID] = &XLayerForkConfig{
			ChainID:     XLayerMainnetChainID,
			NetworkName: "xlayer-mainnet",
			JovianTime:  nil, // Hardcoded as nil
		}

		cfg := &Config{
			L2ChainID:  big.NewInt(XLayerMainnetChainID),
			JovianTime: nil, // Config also has nil
		}
		result := ApplyXLayerHardcodedForks(cfg)
		require.NotNil(t, result)
		assert.Nil(t, result.JovianTime)
	})

	t.Run("XLayer chain with existing JovianTime but hardcoded as nil", func(t *testing.T) {
		// Create a temporary config with nil JovianTime
		originalMainnet := XLayerHardcodedForks[XLayerMainnetChainID]
		defer func() {
			XLayerHardcodedForks[XLayerMainnetChainID] = originalMainnet
		}()

		// Temporarily set JovianTime to nil
		XLayerHardcodedForks[XLayerMainnetChainID] = &XLayerForkConfig{
			ChainID:     XLayerMainnetChainID,
			NetworkName: "xlayer-mainnet",
			JovianTime:  nil, // Hardcoded as nil
		}

		jovianTime := uint64(999)
		cfg := &Config{
			L2ChainID:  big.NewInt(XLayerMainnetChainID),
			JovianTime: &jovianTime, // Config has a value
		}
		result := ApplyXLayerHardcodedForks(cfg)
		require.NotNil(t, result)
		assert.Nil(t, result.JovianTime) // Should be disabled
	})
}

func TestNewUint64(t *testing.T) {
	t.Run("returns pointer to value", func(t *testing.T) {
		val := uint64(12345)
		ptr := newUint64(val)
		require.NotNil(t, ptr)
		assert.Equal(t, val, *ptr)
	})

	t.Run("zero value", func(t *testing.T) {
		ptr := newUint64(0)
		require.NotNil(t, ptr)
		assert.Equal(t, uint64(0), *ptr)
	})

	t.Run("max value", func(t *testing.T) {
		ptr := newUint64(^uint64(0))
		require.NotNil(t, ptr)
		assert.Equal(t, ^uint64(0), *ptr)
	})
}

func TestFixXLayerL2Time(t *testing.T) {
	rollupConfigPath := "./rollup-unit-test.json"

	// 1. Read ./rollup-mainnet.json
	cfg, err := readConfig(rollupConfigPath)
	assert.NoError(t, err)

	// 2. Verify L2Time was not fixed
	initL2Time := cfg.Genesis.L2Time
	assert.Equal(t, uint64(MainnetOldL2Time), cfg.Genesis.L2Time)

	// 3. Fix it by calling FixXLayerL2Time
	FixXLayerL2Time(cfg, rollupConfigPath)

	// 4. Verify L2Time was fixed
	assert.Equal(t, uint64(MainnetFixedL2Time), cfg.Genesis.L2Time, "L2Time should be fixed to MainnetFixedL2Time")

	// 5. Verify the file was updated
	savedCfg, err := readConfig(rollupConfigPath)
	assert.NoError(t, err)
	assert.Equal(t, uint64(MainnetFixedL2Time), savedCfg.Genesis.L2Time, "L2Time should be fixed to MainnetFixedL2Time")

	// 6. revert L2 time
	cfg.Genesis.L2Time = initL2Time
	saveFixedRollupJSON(cfg, rollupConfigPath)
}

func readConfig(rollupConfigPath string) (*Config, error) {
	file, err := os.Open(rollupConfigPath)
	if err != nil {
		return nil, err
	}
	defer file.Close()

	var cfg Config
	dec := json.NewDecoder(file)
	dec.DisallowUnknownFields()
	err = dec.Decode(&cfg)
	return &cfg, err
}
