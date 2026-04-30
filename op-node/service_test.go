package opnode

import (
	"testing"

	"github.com/ethereum-optimism/optimism/op-node/flags"
	"github.com/ethereum/go-ethereum/log"
	"github.com/stretchr/testify/require"
	"github.com/urfave/cli/v2"
)

func syncConfigCliApp() *cli.App {
	syncConfigFlags := append([]cli.Flag{
		flags.SequencerEnabledFlag,
		flags.L2EngineSyncEnabled,
		flags.SyncModeFlag,
		flags.SyncModeReqRespFlag,
		flags.SyncModeOffsetELSafeFlag,
		flags.L2FollowSource,
		flags.L2FollowSourceSkipL1Check,
		flags.L2EngineKind,
		flags.SkipSyncStartCheck,
	}, flags.P2PFlags("")..., // For p2p.sync.req-resp
	)
	return &cli.App{
		Flags: syncConfigFlags,
		Action: func(c *cli.Context) error {
			_, err := NewSyncConfig(c, log.New())
			return err
		},
	}
}

func run(args []string) error {
	return syncConfigCliApp().Run(append([]string{"test"}, args...))
}

func TestNewSyncConfigDefault(t *testing.T) {
	require.NoError(t, run(nil))
}

func TestNewSyncConfig_SkipL1CheckRequiresFollowSource(t *testing.T) {
	// skip-l1-check without follow source should fail
	err := run([]string{"--l2.follow.source.skip-l1-check"})
	require.Error(t, err)
	require.Contains(t, err.Error(), "--l2.follow.source.skip-l1-check requires --l2.follow.source to be set")
}

func TestNewSyncConfig_SkipL1CheckWithFollowSource(t *testing.T) {
	// skip-l1-check with follow source should succeed
	err := run([]string{
		"--l2.follow.source=http://localhost:9545",
		"--l2.follow.source.skip-l1-check",
	})
	require.NoError(t, err)
}

func TestNewSyncConfig_FollowSourceWithoutSkipL1Check(t *testing.T) {
	// follow source without skip-l1-check should succeed (standard Light CL mode)
	err := run([]string{
		"--l2.follow.source=http://localhost:9545",
	})
	require.NoError(t, err)
}
