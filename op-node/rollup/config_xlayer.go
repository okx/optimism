// X Layer hardcoded fork configurations

package rollup

import (
	"encoding/json"
	"os"

	"github.com/ethereum/go-ethereum/log"
)

// X Layer Chain IDs
const (
	XLayerMainnetChainID = 196  // X Layer mainnet
	XLayerTestnetChainID = 1952 // X Layer testnet (Sepolia)
)

// XLayerForkConfig defines fork time overrides for specific X Layer chains.
// Only fork times that need to be hardcoded are defined here.
// Other configuration fields are read from rollup.json or superchain-registry.
type XLayerForkConfig struct {
	ChainID     uint64
	NetworkName string
	// Only define fork times that need to be hardcoded
	JovianTime *uint64
	// Future forks can be added here (e.g., InteropTime when needed)
}

// XLayerHardcodedForks stores hardcoded fork configurations for all X Layer chains.
var XLayerHardcodedForks = map[uint64]*XLayerForkConfig{
	XLayerMainnetChainID: {
		ChainID:     XLayerMainnetChainID,
		NetworkName: "xlayer-mainnet",
		JovianTime:  newUint64(1764691201), // 2025-12-02 16:00:01 UTC
	},
	XLayerTestnetChainID: {
		ChainID:     XLayerTestnetChainID,
		NetworkName: "xlayer-testnet",
		JovianTime:  newUint64(1764327600), // 2025-11-28 11:00:00 UTC
	},
}

// newUint64 returns a pointer to a uint64 value.
func newUint64(v uint64) *uint64 {
	return &v
}

// ApplyXLayerHardcodedForks applies X Layer hardcoded fork configuration based on ChainID.
// This function only overrides specific fork times, keeping other configuration from the source.
func ApplyXLayerHardcodedForks(cfg *Config) *Config {
	if cfg == nil || cfg.L2ChainID == nil {
		log.Error("X Layer: No rollup config provided, no modifications needed")
		return cfg
	}

	chainID := cfg.L2ChainID.Uint64()
	xlayerForks, exists := XLayerHardcodedForks[chainID]

	if !exists {
		// Not an X Layer chain, return config as-is
		log.Error("X Layer: Not an X Layer chain, no modifications needed", "chainID", chainID)
		return cfg
	}

	log.Info("X Layer: Applying hardcoded fork configuration",
		"chainID", chainID,
		"network", xlayerForks.NetworkName)

	// Apply JovianTime
	if xlayerForks.JovianTime != nil {
		// If config already has a value and it differs, log a warning but still override
		if cfg.JovianTime != nil && *cfg.JovianTime != *xlayerForks.JovianTime {
			log.Warn("X Layer: Overriding rollup config JovianTime with hardcoded value",
				"chainID", chainID,
				"rollupConfig", *cfg.JovianTime,
				"hardcoded", *xlayerForks.JovianTime)
		}
		cfg.JovianTime = xlayerForks.JovianTime
		log.Info("X Layer: Applied JovianTime", "chainID", chainID, "time", *xlayerForks.JovianTime)
	} else {
		// Hardcoded as nil, ensure it's not activated
		if cfg.JovianTime != nil {
			log.Info("X Layer: Disabling JovianTime (hardcoded as nil)",
				"chainID", chainID,
				"previousValue", *cfg.JovianTime)
			cfg.JovianTime = nil
		}
	}

	return cfg
}

func FixXLayerL2Time(cfg *Config, rollupConfigPath string) {
	if cfg == nil || cfg.L2ChainID == nil {
		log.Error("X Layer: No rollup config provided, no modifications needed")
		return
	}

	chainID := cfg.L2ChainID.Uint64()
	if chainID == XLayerMainnetChainID && cfg.Genesis.L2Time != MainnetFixedL2Time {
		log.Warn("X Layer: auto fixed mainnet l2 time")
		cfg.Genesis.L2Time = MainnetFixedL2Time
		saveFixedRollupJSON(cfg, rollupConfigPath)
	} else if chainID == XLayerTestnetChainID && cfg.Genesis.L2Time != TestnetFixedL2Time {
		log.Warn("X Layer: auto fixed mainnet l2 time")
		cfg.Genesis.L2Time = TestnetFixedL2Time
		saveFixedRollupJSON(cfg, rollupConfigPath)
	}
}

func saveFixedRollupJSON(cfg *Config, rollupConfigPath string) {
	data, err := json.MarshalIndent(cfg, "", "  ")
	if err != nil {
		log.Error("X Layer: Failed to marshal JSON", "err", err)
		return
	}

	// Preserve existing file permissions (file is guaranteed to exist at this point
	// as it was opened earlier in NewRollupConfig)
	fileInfo, err := os.Stat(rollupConfigPath)
	if err != nil {
		log.Error("X Layer: Failed to stat rollup config file", "path", rollupConfigPath, "err", err)
		return
	}
	fileMode := fileInfo.Mode().Perm()

	err = os.WriteFile(rollupConfigPath, data, fileMode)
	if err != nil {
		log.Error("X Layer: Failed to save rollup config", "path", rollupConfigPath, "err", err)
		return
	}
	log.Info("X Layer: Successfully saved fixed rollup config", "path", rollupConfigPath)
}
