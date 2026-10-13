package xlayer

import (
	"context"
	"testing"
	"time"

	"github.com/ethereum-optimism/optimism/op-conductor/consensus"
	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
	"github.com/ethereum-optimism/optimism/op-devstack/dsl"
	"github.com/ethereum-optimism/optimism/op-devstack/presets"
	"github.com/ethereum-optimism/optimism/op-devstack/stack"
	"github.com/ethereum-optimism/optimism/op-devstack/sysgo"
	safety "github.com/ethereum-optimism/optimism/op-service/eth/safety"
	"github.com/stretchr/testify/require"
)

// TestXLayerConductorClusterElectsLeader verifies that after the 3-conductor
// cluster starts, exactly one conductor becomes the Raft leader and reports a
// healthy sequencer within a bounded time.
func TestXLayerConductorClusterElectsLeader(gt *testing.T) {
	t := devtest.SerialT(gt)
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

	// Wait for the health monitor to fire at least once (interval: 2s) and
	// report the sequencer healthy. The check is polled rather than instant
	// because SequencerHealthy starts false until the first health check tick.
	require.Eventually(t, func() bool {
		return leader.FetchSequencerHealthy()
	}, 30*time.Second, 500*time.Millisecond, "leader must report a healthy sequencer")
}

// TestXLayerConductorFailoverOnLeaderStop verifies the HA failover path: stopping
// the leader sequencer EL triggers real Raft re-election, all three conductors
// lead in turn, the ex-leader rejoins as a follower after restart, and unsafe
// block production plus batcher safe-head progress are verified after each switch.
func TestXLayerConductorFailoverOnLeaderStop(gt *testing.T) {
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	sys := presets.NewXLayerWithConductors(t)
	l2ChainID := sys.L2Chain.ChainID()
	conductors := sys.ConductorSets[l2ChainID]
	require.Len(t, conductors, 3, "expected exactly 3 conductors")

	ctx, cancel := context.WithTimeout(t.Ctx(), 600*time.Second)
	defer cancel()

	// Map conductor names to their sequencer EL controlled-lifecycle handles so we
	// can stop/restart the EL process rather than calling the conductor RPC, which
	// triggers a real Raft re-election through the conductor health-check path.
	els := map[string]stack.ControlledLifecycle{
		"conductor1": sys.L2EL.Escape().(stack.ControlledLifecycle),
		"conductor2": sys.L2ELSeq2.Escape().(stack.ControlledLifecycle),
		"conductor3": sys.L2ELSeq3.Escape().(stack.ControlledLifecycle),
	}
	cls := map[string]*dsl.L2CLNode{
		"conductor1": sys.L2CL,
		"conductor2": sys.L2CLSeq2,
		"conductor3": sys.L2CLSeq3,
	}

	conductorByName := make(map[string]*dsl.Conductor, len(conductors))
	for _, c := range conductors {
		conductorByName[c.String()] = c
	}

	wasLeader := make(map[string]bool, 3)

	// --- Round 1: stop the current leader's EL, await a real Raft re-election ---
	leader := awaitLeader(t, ctx, conductors, 30*time.Second)
	require.NotNil(t, leader, "initial leader must be elected")
	wasLeader[leader.String()] = true
	stoppedName := leader.String()

	// Ensure all followers have received at least one fresh unsafe block from the
	// leader and their health monitors are passing before we pull the leader's EL
	// out from under them.  Without this, followers whose latest unsafe head is
	// genesis (~60 s old at startup) cannot win the Raft election because the
	// unsafe-interval health check fires immediately.
	awaitAllHealthy(t, ctx, conductors, 60*time.Second)

	require.NoError(t, els[stoppedName].StopControlled(ctx), "stop leader EL %s", stoppedName)

	// Wait for a conductor other than the stopped one to win the election.
	var newLeader *dsl.Conductor
	require.Eventually(t, func() bool {
		for _, c := range conductors {
			if c.String() != stoppedName && c.IsLeader() {
				newLeader = c
				return true
			}
		}
		return false
	}, 60*time.Second, 500*time.Millisecond,
		"a conductor other than %s must become leader after EL stop", stoppedName)
	wasLeader[newLeader.String()] = true

	cls[newLeader.String()].AdvancedUnsafe(3, 60)
	cls[newLeader.String()].Advanced(safety.CrossSafe, 1, 90)

	require.NoError(t, els[stoppedName].StartControlled(ctx), "restart EL %s", stoppedName)

	// Verify the ex-leader rejoins the cluster as a follower.
	require.Eventually(t, func() bool {
		membership := newLeader.FetchClusterMembership()
		for _, srv := range membership.Servers {
			if srv.ID == stoppedName {
				return !conductorByName[stoppedName].IsLeader()
			}
		}
		return false
	}, 30*time.Second, 500*time.Millisecond,
		"ex-leader %s must rejoin as follower after EL restart", stoppedName)

	// --- Round 2: transfer to the third conductor that has not yet led ---
	var thirdName string
	for _, c := range conductors {
		if !wasLeader[c.String()] {
			thirdName = c.String()
			break
		}
	}
	require.NotEmpty(t, thirdName, "expected a conductor that has not yet led")

	membership := newLeader.FetchClusterMembership()
	var thirdInfo *consensus.ServerInfo
	for i := range membership.Servers {
		if membership.Servers[i].ID == thirdName {
			srv := membership.Servers[i]
			thirdInfo = &srv
			break
		}
	}
	require.NotNil(t, thirdInfo, "third conductor %s must appear in membership", thirdName)

	newLeader.TransferLeadershipTo(*thirdInfo)
	require.Eventually(t, func() bool {
		return conductorByName[thirdName].IsLeader()
	}, 30*time.Second, 500*time.Millisecond, "third conductor %s must become leader", thirdName)
	wasLeader[thirdName] = true

	cls[thirdName].AdvancedUnsafe(3, 60)
	cls[thirdName].Advanced(safety.CrossSafe, 1, 90)

	require.Len(t, wasLeader, 3, "all three conductors must have led at some point")
}

// TestXLayerConductorLeadershipTransfer verifies that the leadership can be
// explicitly transferred to each of the three conductors in turn.
func TestXLayerConductorLeadershipTransfer(gt *testing.T) {
	t := devtest.SerialT(gt)
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
// non-leader conductor's sequencer EL does not disrupt the sequencing quorum:
// the remaining two conductors still retain a leader.
func TestXLayerConductorMemberStopPreservesQuorum(gt *testing.T) {
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	sys := presets.NewXLayerWithConductors(t)
	l2ChainID := sys.L2Chain.ChainID()
	conductors := sys.ConductorSets[l2ChainID]
	require.Len(t, conductors, 3, "expected exactly 3 conductors")

	ctx, cancel := context.WithTimeout(t.Ctx(), 90*time.Second)
	defer cancel()

	els := map[string]stack.ControlledLifecycle{
		"conductor1": sys.L2EL.Escape().(stack.ControlledLifecycle),
		"conductor2": sys.L2ELSeq2.Escape().(stack.ControlledLifecycle),
		"conductor3": sys.L2ELSeq3.Escape().(stack.ControlledLifecycle),
	}

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
	require.NoError(t, els[follower.String()].StopControlled(ctx), "stop follower EL %s", follower.String())

	// Leader must remain elected and healthy after the follower EL stops.
	require.Eventually(t, func() bool {
		return leader.IsLeader() && leader.FetchSequencerHealthy()
	}, 30*time.Second, 500*time.Millisecond, "leader must remain healthy after a follower EL stops")
}

// TestXLayerConductorUnsafeBlocksAdvance verifies that the HA cluster produces
// new unsafe L2 blocks after startup.
func TestXLayerConductorUnsafeBlocksAdvance(gt *testing.T) {
	t := devtest.SerialT(gt)
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
	t := devtest.SerialT(gt)
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
