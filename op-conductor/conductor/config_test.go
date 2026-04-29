package conductor

import (
	"math/big"
	"testing"
	"time"

	"github.com/ethereum-optimism/optimism/op-node/rollup"
	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/stretchr/testify/require"
)

func TestConfigCheckRollupBoostAndNextMutuallyExclusive(t *testing.T) {
	cfg := &Config{
		ConsensusAddr:                 "127.0.0.1",
		ConsensusPort:                 9000,
		RaftServerID:                  "server-1",
		RaftStorageDir:                "/tmp/op-conductor",
		NodeRPC:                       "http://node.example",
		ExecutionRPC:                  "http://exec.example",
		RollupBoostEnabled:            true,
		RollupBoostNextEnabled:        true,
		RollupBoostNextHealthcheckURL: "http://rollupboost.example",
	}

	err := cfg.Check()
	require.Error(t, err)
	require.Contains(t, err.Error(), "only one of rollup-boost or rollup-boost next healthchecks can be enabled")
}

// TestConfigCheck_HTTPBodyLimit tests the HTTP body limit validation in Config.Check()
func TestConfigCheck_HTTPBodyLimit(t *testing.T) {
	tests := []struct {
		name          string
		bodyLimitMB   int
		expectError   bool
		errorContains string
	}{
		{
			name:        "valid: default 5MB",
			bodyLimitMB: 5,
			expectError: false,
		},
		{
			name:        "valid: 64MB (recommended)",
			bodyLimitMB: 64,
			expectError: false,
		},
		{
			name:        "valid: 128MB (large)",
			bodyLimitMB: 128,
			expectError: false,
		},
		{
			name:        "valid: 10MB",
			bodyLimitMB: 10,
			expectError: false,
		},
		{
			name:        "valid: 4MB",
			bodyLimitMB: 4,
			expectError: false,
		},
		{
			name:        "valid: 1MB",
			bodyLimitMB: 1,
			expectError: false,
		},
		{
			name:          "invalid: 0MB",
			bodyLimitMB:   0,
			expectError:   true,
			errorContains: "HTTP body limit must be greater than 0, got 0MB",
		},
		{
			name:          "invalid: -1MB (negative)",
			bodyLimitMB:   -1,
			expectError:   true,
			errorContains: "HTTP body limit must be greater than 0, got -1MB",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			cfg := newValidConfig(t)
			cfg.HTTPBodyLimitMB = tt.bodyLimitMB

			err := cfg.Check()

			if tt.expectError {
				require.Error(t, err)
				require.Contains(t, err.Error(), tt.errorContains)
			} else {
				require.NoError(t, err)
			}
		})
	}
}

// newValidConfig creates a valid Config for testing
func newValidConfig(t *testing.T) Config {
	now := uint64(time.Now().Unix())
	return Config{
		ConsensusAddr:   "127.0.0.1",
		ConsensusPort:   50050,
		RaftServerID:    "test-node",
		RaftStorageDir:  t.TempDir(),
		RaftBootstrap:   true,
		NodeRPC:         "http://localhost:8545",
		ExecutionRPC:    "http://localhost:8551",
		Paused:          false,
		HTTPBodyLimitMB: 5, // Default valid value
		HealthCheck: HealthCheckConfig{
			Interval:       1,
			UnsafeInterval: 3,
			SafeInterval:   5,
			MinPeerCount:   1,
		},
		RollupCfg: rollup.Config{
			Genesis: rollup.Genesis{
				L1: eth.BlockID{
					Hash:   [32]byte{1},
					Number: 100,
				},
				L2: eth.BlockID{
					Hash:   [32]byte{2},
					Number: 0,
				},
				L2Time: now,
				SystemConfig: eth.SystemConfig{
					BatcherAddr: [20]byte{1},
					Overhead:    [32]byte{1},
					Scalar:      [32]byte{1},
					GasLimit:    30_000_000,
				},
			},
			BlockTime:               2,
			MaxSequencerDrift:       600,
			SeqWindowSize:           3600,
			ChannelTimeoutBedrock:   300,
			L1ChainID:               big.NewInt(1),
			L2ChainID:               big.NewInt(2),
			RegolithTime:            &now,
			CanyonTime:              &now,
			BatchInboxAddress:       [20]byte{1, 2},
			DepositContractAddress:  [20]byte{2, 3},
			L1SystemConfigAddress:   [20]byte{3, 4},
		},
	}
}
