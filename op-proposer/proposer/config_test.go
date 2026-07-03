package proposer

import (
	"testing"

	proposerFlags "github.com/ethereum-optimism/optimism/op-proposer/flags"
	oplog "github.com/ethereum-optimism/optimism/op-service/log"
	opmetrics "github.com/ethereum-optimism/optimism/op-service/metrics"
	"github.com/ethereum-optimism/optimism/op-service/oppprof"
	oprpc "github.com/ethereum-optimism/optimism/op-service/rpc"
	"github.com/ethereum-optimism/optimism/op-service/txmgr"
	"github.com/ethereum/go-ethereum/common"
	"github.com/stretchr/testify/require"
	"github.com/urfave/cli/v2"
)

func TestValidConfigIsValid(t *testing.T) {
	cfg := validConfig()
	require.NoError(t, cfg.Check())
}

func TestNewConfigReadsSuperNodeRpcs(t *testing.T) {
	var cfg *CLIConfig
	app := cli.NewApp()
	app.Flags = proposerFlags.Flags
	app.Action = func(ctx *cli.Context) error {
		cfg = NewConfig(ctx)
		return nil
	}
	err := app.Run([]string{
		"op-proposer",
		"--supernode-rpcs", "http://localhost:8882/supernode-a",
		"--supernode-rpcs", "http://localhost:8883/supernode-b",
	})
	require.NoError(t, err)
	require.Equal(t, []string{
		"http://localhost:8882/supernode-a",
		"http://localhost:8883/supernode-b",
	}, cfg.SuperNodeRpcs)
}

func TestRollupRpc(t *testing.T) {
	// For xlayer: only game types 0 and 1 are supported; higher preInterop types are rejected first.
	for _, gameType := range []uint32{0, 1} {
		t.Run("RequiredWithPreInteropGame", func(t *testing.T) {
			cfg := validConfig()
			cfg.DGFAddress = common.Address{0xaa}.Hex()
			cfg.ProposalInterval = 20
			cfg.RollupRpc = ""
			cfg.SuperNodeRpcs = []string{"http://localhost:8882/supernode"}
			cfg.DisputeGameType = gameType
			require.ErrorIs(t, cfg.Check(), ErrMissingRollupRpc)
		})
	}

	t.Run("UnsupportedForOtherGameTypes", func(t *testing.T) {
		cfg := validConfig()
		cfg.DGFAddress = common.Address{0xaa}.Hex()
		cfg.ProposalInterval = 20
		cfg.RollupRpc = ""
		cfg.SuperNodeRpcs = []string{"http://localhost:8882/supernode"}
		cfg.DisputeGameType = 492743
		require.ErrorIs(t, cfg.Check(), ErrUnsupportedDisputeGameType)
	})
}

func TestSuperNodeRpc(t *testing.T) {
	// For xlayer: postInterop game types (>1) are rejected by ErrUnsupportedDisputeGameType
	// before the supernode source check is reached.
	for _, gameType := range postInteropGameTypes {
		t.Run("UnsupportedPostInteropGame", func(t *testing.T) {
			cfg := validConfig()
			cfg.DGFAddress = common.Address{0xaa}.Hex()
			cfg.ProposalInterval = 20
			cfg.RollupRpc = ""
			cfg.SuperNodeRpcs = []string{"http://localhost:8882/supernode"}
			cfg.DisputeGameType = gameType
			require.ErrorIs(t, cfg.Check(), ErrUnsupportedDisputeGameType)
		})
	}

	t.Run("UnsupportedForOtherGameTypes", func(t *testing.T) {
		cfg := validConfig()
		cfg.DGFAddress = common.Address{0xaa}.Hex()
		cfg.ProposalInterval = 20
		cfg.RollupRpc = ""
		cfg.SuperNodeRpcs = []string{"http://localhost:8882/supernode"}
		cfg.DisputeGameType = 492743
		require.ErrorIs(t, cfg.Check(), ErrUnsupportedDisputeGameType)
	})
}

func TestDisallowRollupAndSuperNodeRPC(t *testing.T) {
	cfg := validConfig()
	cfg.ProposalInterval = 20
	cfg.RollupRpc = "http://localhost:8882/rollup"
	cfg.SuperNodeRpcs = []string{"http://localhost:8882/supernode"}
	cfg.DisputeGameType = 492743
	require.ErrorIs(t, cfg.Check(), ErrConflictingSource)
}

func TestRequireSomeRPCSourceForUnknownGameTypes(t *testing.T) {
	cfg := validConfig()
	cfg.RollupRpc = ""
	cfg.SuperNodeRpcs = nil
	cfg.DisputeGameType = 492743
	// For xlayer: unknown game types are rejected before the missing source check.
	require.ErrorIs(t, cfg.Check(), ErrUnsupportedDisputeGameType)
}

func validConfig() *CLIConfig {
	return &CLIConfig{
		L1EthRpc:                     "http://localhost:8888/l1",
		RollupRpc:                    "http://localhost:8888/l2",
		SuperNodeRpcs:                nil,
		PollInterval:                 100,
		AllowNonFinalized:            false,
		TxMgrConfig:                  txmgr.NewCLIConfig("http://localhost:8888/l1", txmgr.DefaultBatcherFlagValues),
		RPCConfig:                    oprpc.DefaultCLIConfig(),
		LogConfig:                    oplog.DefaultCLIConfig(),
		MetricsConfig:                opmetrics.DefaultCLIConfig(),
		PprofConfig:                  oppprof.DefaultCLIConfig(),
		DGFAddress:                   common.Address{0xaa, 0xbb, 0xcc}.Hex(),
		ProposalInterval:             50,
		DisputeGameType:              0,
		ActiveSequencerCheckDuration: 0,
		WaitNodeSync:                 false,
	}
}
