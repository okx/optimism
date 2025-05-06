package activation

import (
	"testing"
	"time"

	"github.com/ethereum/go-ethereum/log"
	"github.com/stretchr/testify/require"

	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/ethereum-optimism/optimism/op-service/testlog"
	"github.com/ethereum-optimism/optimism/op-supervisor/supervisor/backend/depset"
	"github.com/ethereum-optimism/optimism/op-supervisor/supervisor/types"
)

func createDepSet(chainConfigs map[eth.ChainID]uint64, messageExpiry uint64) (*depset.StaticConfigDependencySet, error) {
	deps := make(map[eth.ChainID]*depset.StaticConfigDependency)
	for chainID, activationTime := range chainConfigs {
		chainValue, ok := chainID.Uint64()
		if !ok {
			panic("chain ID too large")
		}
		deps[chainID] = &depset.StaticConfigDependency{
			ChainIndex:     types.ChainIndex(chainValue),
			ActivationTime: activationTime,
			HistoryMinTime: activationTime,
		}
	}
	return depset.NewStaticConfigDependencySetWithMessageExpiryOverride(deps, messageExpiry)
}

func TestActivationTimestampChecks(t *testing.T) {
	baseTime := uint64(time.Now().Unix() + 60)
	chainID := eth.ChainID{1}

	depSet, err := createDepSet(map[eth.ChainID]uint64{
		chainID: baseTime,
	}, 3600)
	require.NoError(t, err)

	logger := testlog.Logger(t, log.LvlInfo)
	activationCheckFn := NewCheckFn(depSet, logger)

	testCases := map[uint64]bool{
		baseTime - 2: false,
		baseTime - 1: false,
		baseTime:     false,
		baseTime + 1: true,
		baseTime + 2: true,
	}

	for ts, expectedVal := range testCases {
		active := activationCheckFn(chainID, ts)
		require.Equal(t, expectedVal, active,
			"IsActiveForChain at timestamp %d (activation+%d)", ts, int(ts)-int(baseTime))
	}
}

func TestActivationTimestampChecksEdgeCases(t *testing.T) {
	activationTime := uint64(1000000)
	chainID := eth.ChainID{1}

	depSet, err := createDepSet(map[eth.ChainID]uint64{
		chainID: activationTime,
	}, 3600)
	require.NoError(t, err)

	logger := testlog.Logger(t, log.LvlInfo)
	activationCheckFn := NewCheckFn(depSet, logger)

	testCases := []struct {
		name      string
		timestamp uint64
		expected  bool
	}{
		{"Zero timestamp", 0, false},
		{"One before activation", activationTime - 1, false},
		{"At activation", activationTime, false},
		{"One after activation", activationTime + 1, true},
		{"Far future", activationTime + 1000000, true},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			active := activationCheckFn(chainID, tc.timestamp)

			require.Equal(t, tc.expected, active,
				"IsActiveForChain at timestamp %d", tc.timestamp)
		})
	}

	unknownChain := eth.ChainID{99}
	active := activationCheckFn(unknownChain, activationTime+1)
	require.False(t, active, "Unknown chain should not be active")
}

func TestActivationBlockFiltering(t *testing.T) {
	activationTime := uint64(time.Now().Unix() + 3600)
	chainID := eth.ChainID{1}

	depSet, err := createDepSet(map[eth.ChainID]uint64{
		chainID: activationTime,
	}, 3600)
	require.NoError(t, err)

	logger := testlog.Logger(t, log.LvlInfo)
	activationCheckFn := NewCheckFn(depSet, logger)

	preActivationBlock := eth.BlockRef{
		Time: activationTime - 600,
	}

	postActivationBlock := eth.BlockRef{
		Time: activationTime + 600,
	}

	isActiveForPreActivation := activationCheckFn(chainID, preActivationBlock.Time)
	require.False(t, isActiveForPreActivation, "Chain should not be active at pre-activation time")

	isActiveForPostActivation := activationCheckFn(chainID, postActivationBlock.Time)
	require.True(t, isActiveForPostActivation, "Chain should be active at post-activation time")
}

func TestActivationBoundary(t *testing.T) {
	activationTime := uint64(time.Now().Unix())
	chainA := eth.ChainID{1}
	chainB := eth.ChainID{2}

	depSet, err := createDepSet(map[eth.ChainID]uint64{
		chainA: activationTime,
		chainB: activationTime,
	}, 3600)
	require.NoError(t, err)

	logger := testlog.Logger(t, log.LvlInfo)
	activationCheckFn := NewCheckFn(depSet, logger)

	blockAtActivationA := eth.BlockRef{
		Time: activationTime,
	}

	blockAtActivationB := eth.BlockRef{
		Time: activationTime,
	}

	isActiveA := activationCheckFn(chainA, blockAtActivationA.Time)
	isActiveB := activationCheckFn(chainB, blockAtActivationB.Time)

	require.False(t, isActiveA, "Chain A should not be active at exactly the activation time")
	require.False(t, isActiveB, "Chain B should not be active at exactly the activation time")

	blockJustAfterA := eth.BlockRef{
		Time: activationTime + 1,
	}

	blockJustAfterB := eth.BlockRef{
		Time: activationTime + 1,
	}

	isActiveJustAfterA := activationCheckFn(chainA, blockJustAfterA.Time)
	isActiveJustAfterB := activationCheckFn(chainB, blockJustAfterB.Time)

	require.True(t, isActiveJustAfterA, "Chain A should be active just after the activation time")
	require.True(t, isActiveJustAfterB, "Chain B should be active just after the activation time")

	require.False(t, activationCheckFn(chainA, blockAtActivationA.Time))
	require.False(t, activationCheckFn(chainB, blockAtActivationB.Time))
	require.True(t, activationCheckFn(chainA, blockJustAfterA.Time))
	require.True(t, activationCheckFn(chainB, blockJustAfterB.Time))
}

func TestActivationBoundaryMultipleChainsSameActivationTime(t *testing.T) {
	activationTime := uint64(time.Now().Unix() + 10)
	chainA := eth.ChainID{1}
	chainB := eth.ChainID{2}
	chainC := eth.ChainID{3}

	depSet, err := createDepSet(map[eth.ChainID]uint64{
		chainA: activationTime,
		chainB: activationTime,
		chainC: activationTime,
	}, 3600)
	require.NoError(t, err)

	logger := testlog.Logger(t, log.LvlInfo)
	activationCheckFn := NewCheckFn(depSet, logger)

	beforeActivation := eth.BlockRef{Time: activationTime - 5}
	atActivation := eth.BlockRef{Time: activationTime}
	afterActivation := eth.BlockRef{Time: activationTime + 5}

	require.False(t, activationCheckFn(chainA, beforeActivation.Time))
	require.False(t, activationCheckFn(chainB, beforeActivation.Time))
	require.False(t, activationCheckFn(chainC, beforeActivation.Time))

	require.False(t, activationCheckFn(chainA, atActivation.Time))
	require.False(t, activationCheckFn(chainB, atActivation.Time))
	require.False(t, activationCheckFn(chainC, atActivation.Time))

	require.True(t, activationCheckFn(chainA, afterActivation.Time))
	require.True(t, activationCheckFn(chainB, afterActivation.Time))
	require.True(t, activationCheckFn(chainC, afterActivation.Time))
}

func TestActivationBoundaryMultipleChainsDifferentActivationTimes(t *testing.T) {
	baseTime := uint64(time.Now().Unix())
	chainA := eth.ChainID{1}
	chainB := eth.ChainID{2}
	chainC := eth.ChainID{3}

	depSet, err := createDepSet(map[eth.ChainID]uint64{
		chainA: baseTime,
		chainB: baseTime + 10,
		chainC: baseTime + 20,
	}, 3600)
	require.NoError(t, err)

	logger := testlog.Logger(t, log.LvlInfo)
	activationCheckFn := NewCheckFn(depSet, logger)

	t1 := eth.BlockRef{Time: baseTime + 5}
	t2 := eth.BlockRef{Time: baseTime + 15}
	t3 := eth.BlockRef{Time: baseTime + 25}

	require.True(t, activationCheckFn(chainA, t1.Time))
	require.False(t, activationCheckFn(chainB, t1.Time))
	require.False(t, activationCheckFn(chainC, t1.Time))

	require.True(t, activationCheckFn(chainA, t2.Time))
	require.True(t, activationCheckFn(chainB, t2.Time))
	require.False(t, activationCheckFn(chainC, t2.Time))

	require.True(t, activationCheckFn(chainA, t3.Time))
	require.True(t, activationCheckFn(chainB, t3.Time))
	require.True(t, activationCheckFn(chainC, t3.Time))
}
