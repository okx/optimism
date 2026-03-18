package config

import (
	"errors"
	"time"

	gameTypes "github.com/ethereum-optimism/optimism/op-challenger/game/types"
)

var (
	ErrMissingTeeProverRpc = errors.New("missing TEE prover rpc url")
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
