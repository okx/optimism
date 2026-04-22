package conductor

import (
	"math/big"
	"testing"
	"time"

	"github.com/ethereum-optimism/optimism/op-node/rollup"
	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/stretchr/testify/require"
)

// X Layer: TestConfigCheck_HTTPBodyLimit tests the HTTP body limit validation in Config.Check()
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
			name:        "valid: 256MB (maximum)",
			bodyLimitMB: 256,
			expectError: false,
		},
		{
			name:        "valid: 10MB",
			bodyLimitMB: 10,
			expectError: false,
		},
		{
			name:          "invalid: 0MB",
			bodyLimitMB:   0,
			expectError:   true,
			errorContains: "HTTP body limit must be between 5MB and 256MB, got 0MB",
		},
		{
			name:          "invalid: 257MB (above maximum)",
			bodyLimitMB:   257,
			expectError:   true,
			errorContains: "HTTP body limit must be between 5MB and 256MB, got 257MB",
		},
		{
			name:          "invalid: 512MB (too large)",
			bodyLimitMB:   512,
			expectError:   true,
			errorContains: "HTTP body limit must be between 5MB and 256MB, got 512MB",
		},
		{
			name:          "invalid: 1000MB (excessive)",
			bodyLimitMB:   1000,
			expectError:   true,
			errorContains: "HTTP body limit must be between 5MB and 256MB, got 1000MB",
		},
		{
			name:          "invalid: 4MB (below minimum)",
			bodyLimitMB:   4,
			expectError:   true,
			errorContains: "HTTP body limit must be between 5MB and 256MB, got 4MB",
		},
		{
			name:          "invalid: 1MB (too small)",
			bodyLimitMB:   1,
			expectError:   true,
			errorContains: "HTTP body limit must be between 5MB and 256MB, got 1MB",
		},
		{
			name:          "invalid: -1MB (negative)",
			bodyLimitMB:   -1,
			expectError:   true,
			errorContains: "HTTP body limit must be between 5MB and 256MB, got -1MB",
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
			ProtocolVersionsAddress: [20]byte{4, 5},
		},
	}
}
