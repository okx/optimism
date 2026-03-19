package config

import (
	"fmt"
	"testing"

	gameTypes "github.com/ethereum-optimism/optimism/op-challenger/game/types"
	"github.com/stretchr/testify/require"
)

var validTeeProverRpc = "http://localhost:8080"

func applyValidConfigForTee(cfg *Config) {
	cfg.TeeProverRpc = validTeeProverRpc
}

func TestTeeProverRpcRequired(t *testing.T) {
	cfg := validConfig(t, gameTypes.TeeGameType)
	applyValidConfigForTee(&cfg)
	cfg.TeeProverRpc = ""
	require.ErrorIs(t, cfg.Check(), ErrMissingTeeProverRpc)
}

func TestTeeProverRpcNotRequiredForOtherTypes(t *testing.T) {
	for _, gameType := range gameTypes.SupportedGameTypes {
		if gameType == gameTypes.TeeGameType {
			continue
		}
		gameType := gameType
		t.Run(fmt.Sprintf("GameType-%v", gameType), func(t *testing.T) {
			cfg := validConfig(t, gameType)
			// TeeProverRpc is not set — should still be valid for non-TEE game types
			require.NoError(t, cfg.Check())
		})
	}
}

func TestTeeOnlyModeNoL1BeaconRequired(t *testing.T) {
	cfg := validConfig(t, gameTypes.TeeGameType)
	applyValidConfigForTee(&cfg)
	cfg.L1Beacon = ""
	require.NoError(t, cfg.Check())
}

func TestTeeOnlyModeNoL2RpcRequired(t *testing.T) {
	cfg := validConfig(t, gameTypes.TeeGameType)
	applyValidConfigForTee(&cfg)
	cfg.L2Rpcs = nil
	require.NoError(t, cfg.Check())
}

func TestTeeMixedModeStillRequiresL1Beacon(t *testing.T) {
	cfg := validConfig(t, gameTypes.CannonGameType)
	// Add TEE game type alongside cannon
	cfg.GameTypes = append(cfg.GameTypes, gameTypes.TeeGameType)
	applyValidConfigForTee(&cfg)
	cfg.L1Beacon = ""
	require.ErrorIs(t, cfg.Check(), ErrMissingL1Beacon)
}
