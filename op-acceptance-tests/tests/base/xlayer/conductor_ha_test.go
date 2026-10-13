package xlayer

import (
	"context"
	"fmt"
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

// One shared per-trigger deadline for the whole suite: a leadership change is
// allowed this long after the trigger that caused it, whether that trigger is a
// stopped sequencer or an explicit transfer.
const (
	failoverDeadline = 30 * time.Second
	transferDeadline = 5 * time.Second
)

// haCluster bundles the conductor set of the running topology with the fault
// injection handles a test needs: each conductor's own sequencer EL can be
// stopped and restarted, and each conductor's CL can be queried for its head.
type haCluster struct {
	t          devtest.T
	conductors dsl.ConductorSet
	byName     map[string]*dsl.Conductor
	els        map[string]stack.ControlledLifecycle
	cls        map[string]*dsl.L2CLNode
}

func newHACluster(t devtest.T) *haCluster {
	sys := presets.NewXLayerWithConductors(t)
	l2ChainID := sys.L2Chain.ChainID()
	conductors := sys.ConductorSets[l2ChainID]
	require.Len(t, conductors, 3, "expected exactly 3 conductors")

	cluster := &haCluster{
		t:          t,
		conductors: conductors,
		byName:     make(map[string]*dsl.Conductor, len(conductors)),
		els: map[string]stack.ControlledLifecycle{
			"conductor1": sys.L2EL.Escape().(stack.ControlledLifecycle),
			"conductor2": sys.L2ELSeq2.Escape().(stack.ControlledLifecycle),
			"conductor3": sys.L2ELSeq3.Escape().(stack.ControlledLifecycle),
		},
		cls: map[string]*dsl.L2CLNode{
			"conductor1": sys.L2CL,
			"conductor2": sys.L2CLSeq2,
			"conductor3": sys.L2CLSeq3,
		},
	}
	for _, c := range conductors {
		cluster.byName[c.String()] = c
	}
	return cluster
}

// awaitLeader polls the conductor set until one conductor reports it is the Raft
// leader, then returns that conductor.
func (h *haCluster) awaitLeader(ctx context.Context, timeout time.Duration) *dsl.Conductor {
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		for _, c := range h.conductors {
			if c.IsLeader() {
				return c
			}
		}
		select {
		case <-ctx.Done():
			h.t.Require().FailNow("context expired while waiting for a conductor leader")
		case <-time.After(500 * time.Millisecond):
		}
	}
	h.t.Require().FailNow("timed out waiting for a conductor leader")
	return nil
}

// awaitNewLeader polls until a conductor other than exclude reports leadership.
func (h *haCluster) awaitNewLeader(ctx context.Context, exclude string, timeout time.Duration) *dsl.Conductor {
	var found *dsl.Conductor
	h.t.Require().Eventually(func() bool {
		for _, c := range h.conductors {
			if c.String() != exclude && c.IsLeader() {
				found = c
				return true
			}
		}
		return false
	}, timeout, 500*time.Millisecond,
		fmt.Sprintf("a conductor other than %s must become leader", exclude))
	return found
}

// assertAdvancement proves the chain is still progressing after a leadership
// change: at least three new unsafe blocks and a new safe block, which requires
// the batcher to have submitted and had confirmed a new batch on L1.
func (h *haCluster) assertAdvancement(name string) {
	cl := h.cls[name]
	cl.AdvancedUnsafe(3, 60)
	cl.Advanced(safety.CrossSafe, 1, 90)
}

// TestXLayerLeaderElection verifies that the seeded cluster settles on exactly one
// Raft leader with a full three-member membership and that every conductor's
// health monitor really ticks afterwards.
func TestXLayerLeaderElection(gt *testing.T) {
	requireXLayerExecutionBinary(gt)
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	h := newHACluster(t)
	ctx, cancel := context.WithTimeout(t.Ctx(), 120*time.Second)
	defer cancel()

	leader := h.awaitLeader(ctx, failoverDeadline)
	require.True(t, leader.IsLeader(), "the elected conductor must report itself as leader")

	leaders := 0
	for _, c := range h.conductors {
		if c.IsLeader() {
			leaders++
		}
	}
	require.Equal(t, 1, leaders, "exactly one conductor must lead")

	membership := leader.FetchClusterMembership()
	require.Len(t, membership.Servers, 3, "membership must contain all three conductors")

	// Every conductor must emit a genuinely new successful health-check sample,
	// then report a healthy sequencer.
	barrier := newHealthBarrier(t, ctx, h.conductors)
	barrier.awaitNewSample(t, ctx, h.conductors, failoverDeadline)

	require.Eventually(t, func() bool {
		return leader.FetchSequencerHealthy()
	}, failoverDeadline, 500*time.Millisecond, "the leader must report a healthy sequencer")
}

// TestXLayerAutomaticFailover verifies the HA failover path with real Raft
// re-election: the current leader's own sequencer EL is stopped, a different
// conductor takes over, the chain keeps advancing, and all three conductors lead
// at some point in the ring.
func TestXLayerAutomaticFailover(gt *testing.T) {
	requireXLayerExecutionBinary(gt)
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	h := newHACluster(t)
	ctx, cancel := context.WithTimeout(t.Ctx(), 600*time.Second)
	defer cancel()

	led := make(map[string]bool, 3)

	// --- Round 1: stop the leader's sequencer EL, expect a real re-election ---
	leader := h.awaitLeader(ctx, failoverDeadline)
	led[leader.String()] = true
	stopped := leader.String()

	// Let every conductor complete a health check before the trigger so the
	// followers' monitors are demonstrably live.
	awaitAllHealthy(t, ctx, h.conductors, 60*time.Second)

	// A fresh baseline per trigger: the survivors must show a genuinely new
	// sample after the failover, not a value cached from before it.
	barrier := newHealthBarrier(t, ctx, h.conductors)

	require.NoError(t, h.els[stopped].StopControlled(ctx), "stop the leader's sequencer EL %s", stopped)

	newLeader := h.awaitNewLeader(ctx, stopped, failoverDeadline)
	led[newLeader.String()] = true

	survivors := make(dsl.ConductorSet, 0, len(h.conductors)-1)
	for _, c := range h.conductors {
		if c.String() != stopped {
			survivors = append(survivors, c)
		}
	}
	// The stopped node is excluded: with its EL down its own success counter is
	// expected to stall, and asserting on it would test the wrong thing.
	barrier.awaitNewSample(t, ctx, survivors, failoverDeadline)

	h.assertAdvancement(newLeader.String())

	require.NoError(t, h.els[stopped].StartControlled(ctx), "restart the stopped EL %s", stopped)

	// The ex-leader must rejoin as a follower, not resume leadership.
	require.Eventually(t, func() bool {
		membership := newLeader.FetchClusterMembership()
		for _, srv := range membership.Servers {
			if srv.ID == stopped {
				return !h.byName[stopped].IsLeader()
			}
		}
		return false
	}, failoverDeadline, 500*time.Millisecond,
		fmt.Sprintf("the ex-leader %s must rejoin as a follower", stopped))

	// --- Round 2: hand leadership to the conductor that has not led yet ---
	var third string
	for _, c := range h.conductors {
		if !led[c.String()] {
			third = c.String()
			break
		}
	}
	require.NotEmpty(t, third, "expected a conductor that has not yet led")

	newLeader.TransferLeadershipTo(h.serverInfo(newLeader, third))
	require.Eventually(t, func() bool {
		return h.byName[third].IsLeader()
	}, failoverDeadline, 500*time.Millisecond, "the third conductor %s must become leader", third)
	led[third] = true

	require.Len(t, led, 3, "all three conductors must have led at some point")
}

// serverInfo looks up a conductor's Raft server info from the current leader's
// membership view.
func (h *haCluster) serverInfo(from *dsl.Conductor, target string) consensus.ServerInfo {
	membership := from.FetchClusterMembership()
	for _, srv := range membership.Servers {
		if srv.ID == target {
			return srv
		}
	}
	h.t.Require().FailNowf("conductor %s must appear in cluster membership", target)
	return consensus.ServerInfo{}
}

// TestXLayerLeadershipTransfer verifies that leadership can be handed explicitly
// to every other conductor, that the outgoing leader steps down, and that the
// receiving conductor emits a new health sample — twice, so the second transfer
// exercises a leader that was itself elected by transfer.
func TestXLayerLeadershipTransfer(gt *testing.T) {
	requireXLayerExecutionBinary(gt)
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	h := newHACluster(t)
	ctx, cancel := context.WithTimeout(t.Ctx(), 300*time.Second)
	defer cancel()

	leader := h.awaitLeader(ctx, failoverDeadline)

	// Let every conductor's health monitor tick once before the first baseline.
	awaitAllHealthy(t, ctx, h.conductors, 60*time.Second)

	for _, target := range h.conductors {
		if target == leader {
			continue
		}
		old := leader

		// A fresh baseline per trigger, then the assertion that a new sample
		// really arrived on the receiving conductor.
		barrier := newHealthBarrier(t, ctx, h.conductors)

		old.TransferLeadershipTo(h.serverInfo(old, target.String()))

		require.Eventually(t, func() bool {
			return target.IsLeader()
		}, transferDeadline, 100*time.Millisecond,
			"leadership must transfer to conductor %q within %s", target.String(), transferDeadline)

		require.Eventually(t, func() bool {
			return !old.IsLeader()
		}, transferDeadline, 100*time.Millisecond,
			"the outgoing leader %q must step down", old.String())

		barrier.awaitNewSample(t, ctx, dsl.ConductorSet{target}, failoverDeadline)

		leader = target
	}
}

// TestXLayerFollowerRecovery verifies that a sequencer which lost its EL rejoins
// the cluster as a follower and genuinely catches up: its unsafe head is brought
// back level with the current leader's, its health monitor emits a new sample,
// and the chain keeps producing throughout.
func TestXLayerFollowerRecovery(gt *testing.T) {
	requireXLayerExecutionBinary(gt)
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	h := newHACluster(t)
	ctx, cancel := context.WithTimeout(t.Ctx(), 600*time.Second)
	defer cancel()

	leader := h.awaitLeader(ctx, failoverDeadline)
	stopped := leader.String()

	awaitAllHealthy(t, ctx, h.conductors, 60*time.Second)

	require.NoError(t, h.els[stopped].StopControlled(ctx), "stop the leader's sequencer EL %s", stopped)

	newLeader := h.awaitNewLeader(ctx, stopped, failoverDeadline)
	// The cluster must keep advancing while the failed node is out.
	h.assertAdvancement(newLeader.String())

	// Baseline before the recovery trigger.
	barrier := newHealthBarrier(t, ctx, h.conductors)

	require.NoError(t, h.els[stopped].StartControlled(ctx), "restart the stopped EL %s", stopped)

	// Membership must be whole again and the recovered node must be a follower.
	require.Eventually(t, func() bool {
		membership := newLeader.FetchClusterMembership()
		for _, srv := range membership.Servers {
			if srv.ID == stopped {
				return !h.byName[stopped].IsLeader()
			}
		}
		return false
	}, failoverDeadline, 500*time.Millisecond,
		fmt.Sprintf("the recovered node %s must rejoin as a follower", stopped))

	// Real catch-up: presence in membership is not proof of synchronisation, so
	// the recovered node's own unsafe head is compared against the leader's.
	recovered := h.cls[stopped]
	require.Eventually(t, func() bool {
		leaderHead := h.cls[newLeader.String()].HeadBlockRef(safety.LocalUnsafe)
		recoveredHead := recovered.HeadBlockRef(safety.LocalUnsafe)
		return leaderHead.Number <= recoveredHead.Number+2
	}, failoverDeadline, 500*time.Millisecond,
		fmt.Sprintf("the recovered node %s must catch up to within 2 blocks of the leader", stopped))

	// The recovered node emits a new health sample, and the chain still advances.
	barrier.awaitNewSample(t, ctx, dsl.ConductorSet{h.byName[stopped]}, failoverDeadline)
	h.assertAdvancement(stopped)
}

// TestXLayerProgression verifies that both the unsafe and the safe head advance
// monotonically over a sustained window, which requires the HA batcher to keep
// following the active sequencer.
func TestXLayerProgression(gt *testing.T) {
	requireXLayerExecutionBinary(gt)
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	h := newHACluster(t)
	ctx, cancel := context.WithTimeout(t.Ctx(), 300*time.Second)
	defer cancel()

	leader := h.awaitLeader(ctx, failoverDeadline)
	awaitAllHealthy(t, ctx, h.conductors, 60*time.Second)

	cl := h.cls[leader.String()]
	unsafeBefore := cl.HeadBlockRef(safety.LocalUnsafe).Number
	safeBefore := cl.HeadBlockRef(safety.CrossSafe).Number

	// Over a sustained window both heads must move forward, and neither may go
	// backwards.
	deadline := time.Now().Add(30 * time.Second)
	for time.Now().Before(deadline) {
		unsafeNow := cl.HeadBlockRef(safety.LocalUnsafe).Number
		safeNow := cl.HeadBlockRef(safety.CrossSafe).Number
		require.GreaterOrEqual(t, unsafeNow, unsafeBefore, "the unsafe head must not regress")
		require.GreaterOrEqual(t, safeNow, safeBefore, "the safe head must not regress")
		time.Sleep(1 * time.Second)
	}

	require.Greater(t, cl.HeadBlockRef(safety.LocalUnsafe).Number, unsafeBefore,
		"the unsafe head must advance over the window")
	require.Greater(t, cl.HeadBlockRef(safety.CrossSafe).Number, safeBefore,
		"the safe head must advance over the window")
}

// TestXLayerMemberStopPreservesQuorum verifies that losing a non-leader's
// sequencer does not disturb the sequencing quorum.
func TestXLayerMemberStopPreservesQuorum(gt *testing.T) {
	requireXLayerExecutionBinary(gt)
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	h := newHACluster(t)
	ctx, cancel := context.WithTimeout(t.Ctx(), 120*time.Second)
	defer cancel()

	leader := h.awaitLeader(ctx, failoverDeadline)

	var follower *dsl.Conductor
	for _, c := range h.conductors {
		if c != leader {
			follower = c
			break
		}
	}
	require.NotNil(t, follower, "expected a non-leader conductor")
	require.NoError(t, h.els[follower.String()].StopControlled(ctx), "stop follower EL %s", follower.String())

	require.Eventually(t, func() bool {
		return leader.IsLeader() && leader.FetchSequencerHealthy()
	}, failoverDeadline, 500*time.Millisecond, "the leader must stay healthy after a follower EL stops")
}

// TestXLayerBatcherAdvancesSafeHead verifies that the single HA batcher keeps the
// safe head moving on the active sequencer.
func TestXLayerBatcherAdvancesSafeHead(gt *testing.T) {
	requireXLayerExecutionBinary(gt)
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	h := newHACluster(t)
	ctx, cancel := context.WithTimeout(t.Ctx(), 300*time.Second)
	defer cancel()

	leader := h.awaitLeader(ctx, failoverDeadline)
	awaitAllHealthy(t, ctx, h.conductors, 60*time.Second)

	h.cls[leader.String()].AdvancedUnsafe(3, 60)
	h.cls[leader.String()].Advanced(safety.CrossSafe, 1, 90)
}
