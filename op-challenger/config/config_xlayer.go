package config

import (
	"errors"
	"time"

	gameTypes "github.com/ethereum-optimism/optimism/op-challenger/game/types"
)

var (
	ErrMissingTeeProverRpc          = errors.New("missing TEE prover rpc url")
	ErrInvalidTeeProvePollInterval  = errors.New("TEE prove poll interval must be greater than 0")
	ErrInvalidTeeProveTimeout       = errors.New("TEE prove timeout must be greater than 0")
)

const (
	DefaultTeeProvePollInterval = 30 * time.Second
	DefaultTeeProveTimeout      = 1 * time.Hour
)

// xlayerConfigCheckers holds additional config validation functions registered by XLayer extensions.
var xlayerConfigCheckers []func(Config) error

func init() {
	xlayerConfigCheckers = append(xlayerConfigCheckers, checkTeeConfig)
}

func checkTeeConfig(c Config) error {
	if c.GameTypeEnabled(gameTypes.TeeGameType) {
		if c.TeeProverRpc == "" {
			return ErrMissingTeeProverRpc
		}
		if c.TeeProvePollInterval <= 0 {
			return ErrInvalidTeeProvePollInterval
		}
		if c.TeeProveTimeout <= 0 {
			return ErrInvalidTeeProveTimeout
		}
	}
	return nil
}

// onlyTeeGameType returns true if all enabled game types are TEE (no L2/beacon needed).
func (c Config) onlyTeeGameType() bool {
	for _, t := range c.GameTypes {
		if t != gameTypes.TeeGameType {
			return false
		}
	}
	return len(c.GameTypes) > 0
}

// GetL1RPCMaxBatchSize returns the effective batch size, defaulting to 20 (same as op-node).
// When rate limiting is enabled, batch size is capped to int(RateLimit) so that
// a single batch never exceeds the limiter's burst.
func (c Config) GetL1RPCMaxBatchSize() int {
	batchSize := c.L1RPCMaxBatchSize
	if batchSize <= 0 {
		batchSize = 20
	}
	if c.L1RPCRateLimit > 0 && batchSize > int(c.L1RPCRateLimit) {
		batchSize = int(c.L1RPCRateLimit)
	}
	return batchSize
}

// CheckXLayer runs all XLayer-specific config validations.
// Called from the main Check() method via _xlayer integration.
func (c Config) CheckXLayer() error {
	for _, checker := range xlayerConfigCheckers {
		if err := checker(c); err != nil {
			return err
		}
	}
	return nil
}
