package xlayer

import (
	"context"
	"testing"
	"time"

	"github.com/ethereum-optimism/optimism/op-conductor/consensus"
	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
	"github.com/ethereum-optimism/optimism/op-devstack/dsl"
	"github.com/ethereum-optimism/optimism/op-devstack/presets"
	"github.com/ethereum-optimism/optimism/op-devstack/sysgo"
	safety "github.com/ethereum-optimism/optimism/op-service/eth/safety"
	"github.com/stretchr/testify/require"
)

// TestXLayerConductorClusterElectsLeader verifies that after the 3-conductor
// cluster starts, exactly one conductor becomes the Raft leader and reports a
// healthy sequencer within a bounded time.
func TestXLayerConductorClusterElectsLeader(gt *testing.T) {
	t := devtest.ParallelT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	sys := presets.NewXLayerWithConductors(t)
	l2ChainID := sys.L2Chain.ChainID()
	conductors := sys.ConductorSets[l2ChainID]
	require.NotEmpty(t, conductors, "expected at least one conductor")

	ctx, cancel := context.WithTimeout(t.Ctx(), 60*time.Second)
	defer cancel()

	leader := awaitLeader(t, ctx, conductors, 30*time.Second)
	require.NotNil(t, leader, "a leader must be elected")
	require.True(t, leader.IsLeader(), "elected conductor must report itself as leader")
	require.True(t, leader.FetchSequencerHealthy(), "leader must report a healthy sequencer")
}

// TestXLayerConductorFailoverOnLeaderStop verifies that when the current leader
// is stopped via its RPC, the remaining conductors elect a new leader.
func TestXLayerConductorFailoverOnLeaderStop(gt *testing.T) {
	t := devtest.ParallelT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	sys := presets.NewXLayerWithConductors(t)
	l2ChainID := sys.L2Chain.ChainID()
	conductors := sys.ConductorSets[l2ChainID]
	require.NotEmpty(t, conductors, "expected at least one conductor")

	ctx, cancel := context.WithTimeout(t.Ctx(), 90*time.Second)
	defer cancel()

	leader := awaitLeader(t, ctx, conductors, 30*time.Second)
	require.NotNil(t, leader, "a leader must be elected before the failover test")
	stoppedLeaderName := leader.String()

	require.NoError(t, leader.Escape().RpcAPI().Stop(ctx), "stopping leader conductor must succeed")

	// After the leader stops, the remaining two must elect a new leader.
	newLeader := awaitLeader(t, ctx, conductors, 60*time.Second)
	require.NotNil(t, newLeader, "a new leader must be elected after the old leader stops")
	require.NotEqual(t, stoppedLeaderName, newLeader.String(), "new leader must be a different conductor")
}

// TestXLayerConductorLeadershipTransfer verifies that the leadership can be
// explicitly transferred to each of the three conductors in turn.
func TestXLayerConductorLeadershipTransfer(gt *testing.T) {
	t := devtest.ParallelT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	sys := presets.NewXLayerWithConductors(t)
	l2ChainID := sys.L2Chain.ChainID()
	conductors := sys.ConductorSets[l2ChainID]
	require.Len(t, conductors, 3, "expected exactly 3 conductors")

	ctx, cancel := context.WithTimeout(t.Ctx(), 120*time.Second)
	defer cancel()

	leader := awaitLeader(t, ctx, conductors, 30*time.Second)
	require.NotNil(t, leader, "an initial leader must be elected")

	// Transfer leadership to each non-leader conductor in turn.
	for _, target := range conductors {
		if target == leader {
			continue
		}
		// Find the target's ServerInfo from the current leader's membership view.
		membership := leader.FetchClusterMembership()
		var targetServerInfo *consensus.ServerInfo
		for i := range membership.Servers {
			if membership.Servers[i].ID == target.String() {
				srv := membership.Servers[i]
				targetServerInfo = &srv
				break
			}
		}
		require.NotNil(t, targetServerInfo, "target conductor %q must appear in cluster membership", target.String())

		leader.TransferLeadershipTo(*targetServerInfo)

		require.Eventually(t, func() bool {
			return target.IsLeader()
		}, 30*time.Second, 500*time.Millisecond, "leadership must transfer to conductor %q", target.String())

		leader = target
	}
}

// TestXLayerConductorMemberStopPreservesQuorum verifies that stopping a
// non-leader conductor does not disrupt the sequencing quorum: the remaining
// two conductors still retain a leader.
func TestXLayerConductorMemberStopPreservesQuorum(gt *testing.T) {
	t := devtest.ParallelT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	sys := presets.NewXLayerWithConductors(t)
	l2ChainID := sys.L2Chain.ChainID()
	conductors := sys.ConductorSets[l2ChainID]
	require.Len(t, conductors, 3, "expected exactly 3 conductors")

	ctx, cancel := context.WithTimeout(t.Ctx(), 90*time.Second)
	defer cancel()

	leader := awaitLeader(t, ctx, conductors, 30*time.Second)
	require.NotNil(t, leader, "a leader must be elected")

	// Find a non-leader follower to stop.
	var follower *dsl.Conductor
	for _, c := range conductors {
		if c != leader {
			follower = c
			break
		}
	}
	require.NotNil(t, follower, "expected to find a non-leader conductor")
	require.NoError(t, follower.Escape().RpcAPI().Stop(ctx), "stopping follower conductor must succeed")

	// Leader must remain elected and healthy.
	require.Eventually(t, func() bool {
		return leader.IsLeader() && leader.FetchSequencerHealthy()
	}, 30*time.Second, 500*time.Millisecond, "leader must remain healthy after a follower stops")
}

// TestXLayerConductorUnsafeBlocksAdvance verifies that the HA cluster produces
// new unsafe L2 blocks after startup.
func TestXLayerConductorUnsafeBlocksAdvance(gt *testing.T) {
	t := devtest.ParallelT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	sys := presets.NewXLayerWithConductors(t)
	l2ChainID := sys.L2Chain.ChainID()
	conductors := sys.ConductorSets[l2ChainID]

	ctx, cancel := context.WithTimeout(t.Ctx(), 60*time.Second)
	defer cancel()

	awaitLeader(t, ctx, conductors, 30*time.Second)
	awaitAllHealthy(t, ctx, conductors, 30*time.Second)

	// Expect the primary CL to advance by at least 3 unsafe blocks.
	sys.L2CL.AdvancedUnsafe(3, 60)
	_ = l2ChainID
}

// TestXLayerBatcherAdvancesSafeHead verifies that the HA batcher submits
// batches and the safe head advances on the primary CL.
func TestXLayerBatcherAdvancesSafeHead(gt *testing.T) {
	t := devtest.ParallelT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	sys := presets.NewXLayerWithConductors(t)
	l2ChainID := sys.L2Chain.ChainID()
	conductors := sys.ConductorSets[l2ChainID]

	ctx, cancel := context.WithTimeout(t.Ctx(), 120*time.Second)
	defer cancel()

	awaitLeader(t, ctx, conductors, 30*time.Second)
	awaitAllHealthy(t, ctx, conductors, 30*time.Second)

	// Unsafe head must advance first so there is something to batch.
	sys.L2CL.AdvancedUnsafe(3, 60)

	// Safe head must advance (batcher submits and L1 confirms within 90 attempts).
	sys.L2CL.Advanced(safety.CrossSafe, 1, 90)
	_ = l2ChainID
}
