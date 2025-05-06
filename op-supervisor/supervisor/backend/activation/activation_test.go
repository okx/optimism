package activation

import (
	"testing"
	"time"

	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/ethereum-optimism/optimism/op-service/testlog"
	"github.com/ethereum-optimism/optimism/op-supervisor/supervisor/backend/depset"
	"github.com/ethereum-optimism/optimism/op-supervisor/supervisor/types"
	"github.com/ethereum/go-ethereum/log"
	"github.com/stretchr/testify/require"
)

// MockDependencySet implements the DependencySet interface for testing
type mockDependencySet struct {
	chainConfigs   map[eth.ChainID]*depset.StaticConfigDependency
	messageExpiry  uint64
	activationTime uint64
}

func (m *mockDependencySet) AddChain(chainID eth.ChainID, activationTime uint64) {
	if m.chainConfigs == nil {
		m.chainConfigs = make(map[eth.ChainID]*depset.StaticConfigDependency)
	}

	chainValue, ok := chainID.Uint64()
	if !ok {
		panic("chain ID too large")
	}

	m.chainConfigs[chainID] = &depset.StaticConfigDependency{
		ChainIndex:     types.ChainIndex(chainValue),
		ActivationTime: activationTime,
		HistoryMinTime: activationTime - 1,
	}
}

func (m *mockDependencySet) Chains() []eth.ChainID {
	chains := make([]eth.ChainID, 0, len(m.chainConfigs))
	for chain := range m.chainConfigs {
		chains = append(chains, chain)
	}
	return chains
}

func (m *mockDependencySet) CanInitiateAt(chain eth.ChainID, timestamp uint64) (bool, error) {
	cfg, ok := m.chainConfigs[chain]
	if !ok {
		return false, nil
	}
	return timestamp > cfg.ActivationTime, nil
}

func (m *mockDependencySet) CanReceiveAt(chain eth.ChainID, timestamp uint64) (bool, error) {
	return m.CanInitiateAt(chain, timestamp)
}

func (m *mockDependencySet) CanExecuteAt(chain eth.ChainID, timestamp uint64) (bool, error) {
	return m.CanInitiateAt(chain, timestamp)
}

func (m *mockDependencySet) MessageExpiryWindow() uint64 {
	return m.messageExpiry
}

func (m *mockDependencySet) ReverseChainLookup(idx types.ChainIndex) (eth.ChainID, error) {
	for chain, cfg := range m.chainConfigs {
		if cfg.ChainIndex == idx {
			return chain, nil
		}
	}
	return eth.ChainID{}, nil
}

func (m *mockDependencySet) ChainIDFromIndex(idx types.ChainIndex) (eth.ChainID, error) {
	return m.ReverseChainLookup(idx)
}

func (m *mockDependencySet) ChainIndexFromID(id eth.ChainID) (types.ChainIndex, error) {
	cfg, ok := m.chainConfigs[id]
	if !ok {
		return 0, nil
	}
	return cfg.ChainIndex, nil
}

func (m *mockDependencySet) HasChain(id eth.ChainID) bool {
	_, ok := m.chainConfigs[id]
	return ok
}

func (m *mockDependencySet) ValidMessageLifespan(timestamp uint64) (bool, error) {
	now := uint64(time.Now().Unix())
	if timestamp > now {
		return false, nil
	}
	age := now - timestamp
	return age <= m.messageExpiry, nil
}

func TestActivationTimestampChecks(t *testing.T) {
	baseTime := uint64(time.Now().Unix() + 60)

	mockDepSet := &mockDependencySet{
		activationTime: baseTime,
		messageExpiry:  3600,
	}
	chainID := eth.ChainID{1}
	mockDepSet.AddChain(chainID, baseTime)

	logger := testlog.Logger(t, log.LvlInfo)
	am := NewActivationManager(mockDepSet, logger)

	testCases := map[uint64]bool{
		baseTime - 2: false,
		baseTime - 1: false,
		baseTime:     false,
		baseTime + 1: true,
		baseTime + 2: true,
	}

	for ts, expectedVal := range testCases {
		blockRef := eth.BlockRef{Time: ts}
		active := am.IsActiveForChain(chainID, ts)
		shouldProcess := am.ShouldProcessEvent(chainID, blockRef)

		require.Equal(t, expectedVal, active,
			"IsActiveForChain at timestamp %d (activation+%d)", ts, int(ts)-int(baseTime))
		require.Equal(t, expectedVal, shouldProcess,
			"ShouldProcessEvent at timestamp %d (activation+%d)", ts, int(ts)-int(baseTime))
	}
}

func TestActivationTimestampChecksEdgeCases(t *testing.T) {
	activationTime := uint64(1000000)
	mockDepSet := &mockDependencySet{
		activationTime: activationTime,
		messageExpiry:  3600,
	}

	chainID := eth.ChainID{1}
	mockDepSet.AddChain(chainID, activationTime)

	logger := testlog.Logger(t, log.LvlInfo)
	am := NewActivationManager(mockDepSet, logger)

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
			blockRef := eth.BlockRef{Time: tc.timestamp}
			active := am.IsActiveForChain(chainID, tc.timestamp)
			shouldProcess := am.ShouldProcessEvent(chainID, blockRef)

			require.Equal(t, tc.expected, active,
				"IsActiveForChain at timestamp %d", tc.timestamp)
			require.Equal(t, tc.expected, shouldProcess,
				"ShouldProcessEvent at timestamp %d", tc.timestamp)
		})
	}

	unknownChain := eth.ChainID{99}
	active := am.IsActiveForChain(unknownChain, activationTime+1)
	blockRef := eth.BlockRef{Time: activationTime + 1}
	shouldProcess := am.ShouldProcessEvent(unknownChain, blockRef)

	require.False(t, active, "Unknown chain should not be active")
	require.False(t, shouldProcess, "Events for unknown chain should not be processed")
}

func TestActivationManagerIsActive(t *testing.T) {
	baseTime := uint64(time.Now().Unix())
	mockDepSet := &mockDependencySet{
		activationTime: baseTime,
		messageExpiry:  3600,
	}

	chainA := eth.ChainID{1}
	chainB := eth.ChainID{2}
	chainC := eth.ChainID{3}

	mockDepSet.AddChain(chainA, baseTime-5)
	mockDepSet.AddChain(chainB, baseTime+10)
	mockDepSet.AddChain(chainC, baseTime+20)

	logger := testlog.Logger(t, log.LvlInfo)
	am := NewActivationManager(mockDepSet, logger)

	require.True(t, am.IsActive(), "IsActive should return true if any chain is active")

	require.True(t, am.IsActiveAt(baseTime),
		"IsActiveAt(baseTime) should be true because chainA is active")

	require.True(t, am.IsActiveAt(baseTime+15),
		"IsActiveAt(baseTime+15) should be true because chainA and chainB are active")

	require.True(t, am.IsActiveAt(baseTime+25),
		"IsActiveAt(baseTime+25) should be true because all chains are active")

	inactiveMockDepSet := &mockDependencySet{
		activationTime: baseTime + 100,
		messageExpiry:  3600,
	}

	inactiveMockDepSet.AddChain(chainA, baseTime+100)
	inactiveMockDepSet.AddChain(chainB, baseTime+100)

	inactiveAM := NewActivationManager(inactiveMockDepSet, logger)

	require.False(t, inactiveAM.IsActive(),
		"IsActive should return false when no chains are active")

	require.False(t, inactiveAM.IsActiveAt(baseTime),
		"IsActiveAt(baseTime) should be false when no chains are active at that time")

	require.True(t, inactiveAM.IsActiveAt(baseTime+101),
		"IsActiveAt(baseTime+101) should be true when chains activate at baseTime+100")
}

func TestActivationBlockFiltering(t *testing.T) {
	activationTime := uint64(time.Now().Unix() + 3600)

	mockDepSet := &mockDependencySet{
		activationTime: activationTime,
		messageExpiry:  3600,
	}

	chainID := eth.ChainID{1}
	mockDepSet.AddChain(chainID, activationTime)

	logger := testlog.Logger(t, log.LvlInfo)
	am := NewActivationManager(mockDepSet, logger)

	preActivationBlock := eth.BlockRef{
		Time: activationTime - 600,
	}

	postActivationBlock := eth.BlockRef{
		Time: activationTime + 600,
	}

	shouldProcessPre := am.ShouldProcessEvent(chainID, preActivationBlock)
	require.False(t, shouldProcessPre, "Pre-activation blocks should be filtered out")

	shouldProcessPost := am.ShouldProcessEvent(chainID, postActivationBlock)
	require.True(t, shouldProcessPost, "Post-activation blocks should be processed")

	isActiveForPreActivation := am.IsActiveForChain(chainID, preActivationBlock.Time)
	require.False(t, isActiveForPreActivation, "Chain should not be active at pre-activation time")

	isActiveForPostActivation := am.IsActiveForChain(chainID, postActivationBlock.Time)
	require.True(t, isActiveForPostActivation, "Chain should be active at post-activation time")
}

func TestActivationBoundary(t *testing.T) {
	activationTime := uint64(time.Now().Unix())

	mockDepSet := &mockDependencySet{
		activationTime: activationTime,
		messageExpiry:  3600,
	}

	chainA := eth.ChainID{1}
	chainB := eth.ChainID{2}
	mockDepSet.AddChain(chainA, activationTime)
	mockDepSet.AddChain(chainB, activationTime)

	logger := testlog.Logger(t, log.LvlInfo)
	am := NewActivationManager(mockDepSet, logger)

	blockAtActivationA := eth.BlockRef{
		Time: activationTime,
	}

	blockAtActivationB := eth.BlockRef{
		Time: activationTime,
	}

	isActiveA := am.IsActiveForChain(chainA, blockAtActivationA.Time)
	isActiveB := am.IsActiveForChain(chainB, blockAtActivationB.Time)

	require.False(t, isActiveA, "Chain A should not be active at exactly the activation time")
	require.False(t, isActiveB, "Chain B should not be active at exactly the activation time")

	blockJustAfterA := eth.BlockRef{
		Time: activationTime + 1,
	}

	blockJustAfterB := eth.BlockRef{
		Time: activationTime + 1,
	}

	isActiveJustAfterA := am.IsActiveForChain(chainA, blockJustAfterA.Time)
	isActiveJustAfterB := am.IsActiveForChain(chainB, blockJustAfterB.Time)

	require.True(t, isActiveJustAfterA, "Chain A should be active just after the activation time")
	require.True(t, isActiveJustAfterB, "Chain B should be active just after the activation time")

	shouldProcessAtA := am.ShouldProcessEvent(chainA, blockAtActivationA)
	shouldProcessAtB := am.ShouldProcessEvent(chainB, blockAtActivationB)
	shouldProcessAfterA := am.ShouldProcessEvent(chainA, blockJustAfterA)
	shouldProcessAfterB := am.ShouldProcessEvent(chainB, blockJustAfterB)

	require.False(t, shouldProcessAtA, "Blocks exactly at activation should not be processed for Chain A")
	require.False(t, shouldProcessAtB, "Blocks exactly at activation should not be processed for Chain B")
	require.True(t, shouldProcessAfterA, "Blocks just after activation should be processed for Chain A")
	require.True(t, shouldProcessAfterB, "Blocks just after activation should be processed for Chain B")
}

func TestActivationBoundaryMultipleChainsSameActivationTime(t *testing.T) {
	activationTime := uint64(time.Now().Unix() + 10)

	mockDepSet := &mockDependencySet{
		activationTime: activationTime,
		messageExpiry:  3600,
	}

	chainA := eth.ChainID{1}
	chainB := eth.ChainID{2}
	chainC := eth.ChainID{3}
	mockDepSet.AddChain(chainA, activationTime)
	mockDepSet.AddChain(chainB, activationTime)
	mockDepSet.AddChain(chainC, activationTime)

	logger := testlog.Logger(t, log.LvlInfo)
	am := NewActivationManager(mockDepSet, logger)

	beforeActivation := eth.BlockRef{Time: activationTime - 5}
	atActivation := eth.BlockRef{Time: activationTime}
	afterActivation := eth.BlockRef{Time: activationTime + 5}

	require.False(t, am.IsActiveForChain(chainA, beforeActivation.Time))
	require.False(t, am.IsActiveForChain(chainB, beforeActivation.Time))
	require.False(t, am.IsActiveForChain(chainC, beforeActivation.Time))

	require.False(t, am.IsActiveForChain(chainA, atActivation.Time))
	require.False(t, am.IsActiveForChain(chainB, atActivation.Time))
	require.False(t, am.IsActiveForChain(chainC, atActivation.Time))

	require.True(t, am.IsActiveForChain(chainA, afterActivation.Time))
	require.True(t, am.IsActiveForChain(chainB, afterActivation.Time))
	require.True(t, am.IsActiveForChain(chainC, afterActivation.Time))

	require.False(t, am.ShouldProcessEvent(chainA, beforeActivation))
	require.False(t, am.ShouldProcessEvent(chainB, beforeActivation))
	require.False(t, am.ShouldProcessEvent(chainC, beforeActivation))

	require.False(t, am.ShouldProcessEvent(chainA, atActivation))
	require.False(t, am.ShouldProcessEvent(chainB, atActivation))
	require.False(t, am.ShouldProcessEvent(chainC, atActivation))

	require.True(t, am.ShouldProcessEvent(chainA, afterActivation))
	require.True(t, am.ShouldProcessEvent(chainB, afterActivation))
	require.True(t, am.ShouldProcessEvent(chainC, afterActivation))
}

func TestActivationBoundaryMultipleChainsDifferentActivationTimes(t *testing.T) {
	baseTime := uint64(time.Now().Unix())

	mockDepSet := &mockDependencySet{
		activationTime: baseTime,
		messageExpiry:  3600,
	}

	chainA := eth.ChainID{1}
	chainB := eth.ChainID{2}
	chainC := eth.ChainID{3}

	mockDepSet.AddChain(chainA, baseTime)
	mockDepSet.AddChain(chainB, baseTime+10)
	mockDepSet.AddChain(chainC, baseTime+20)

	logger := testlog.Logger(t, log.LvlInfo)
	am := NewActivationManager(mockDepSet, logger)

	t1 := eth.BlockRef{Time: baseTime + 5}
	t2 := eth.BlockRef{Time: baseTime + 15}
	t3 := eth.BlockRef{Time: baseTime + 25}

	require.True(t, am.IsActiveForChain(chainA, t1.Time))
	require.False(t, am.IsActiveForChain(chainB, t1.Time))
	require.False(t, am.IsActiveForChain(chainC, t1.Time))

	require.True(t, am.ShouldProcessEvent(chainA, t1))
	require.False(t, am.ShouldProcessEvent(chainB, t1))
	require.False(t, am.ShouldProcessEvent(chainC, t1))

	require.True(t, am.IsActiveForChain(chainA, t2.Time))
	require.True(t, am.IsActiveForChain(chainB, t2.Time))
	require.False(t, am.IsActiveForChain(chainC, t2.Time))

	require.True(t, am.ShouldProcessEvent(chainA, t2))
	require.True(t, am.ShouldProcessEvent(chainB, t2))
	require.False(t, am.ShouldProcessEvent(chainC, t2))

	require.True(t, am.IsActiveForChain(chainA, t3.Time))
	require.True(t, am.IsActiveForChain(chainB, t3.Time))
	require.True(t, am.IsActiveForChain(chainC, t3.Time))

	require.True(t, am.ShouldProcessEvent(chainA, t3))
	require.True(t, am.ShouldProcessEvent(chainB, t3))
	require.True(t, am.ShouldProcessEvent(chainC, t3))
}
