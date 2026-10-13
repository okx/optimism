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
	"github.com/ethereum-optimism/optimism/op-service/eth"
	safety "github.com/ethereum-optimism/optimism/op-service/eth/safety"
	"github.com/stretchr/testify/require"
)

// Phase budgets. A leadership change gets one shared window for everything it
// has to prove, counted from the trigger that caused it, so the phases of one
// change cannot each restart their own clock and hide a slow or stalled
// handover. failoverDeadline bounds the narrow step of noticing a new leader;
// transferDeadline bounds an explicit handover, which Raft completes in one
// round trip.
const (
	failoverDeadline       = 30 * time.Second
	transferDeadline       = 5 * time.Second
	triggerAdvanceDeadline = 60 * time.Second
	progressionWindow      = 30 * time.Second
	// deadlineMargin keeps a derived phase context strictly inside the test's own
	// deadline, so a phase that runs out of its budget reports its own timeout
	// instead of being torn down together with the whole test.
	deadlineMargin = 1 * time.Second
)

// derivePhaseTimeout returns how long one phase may run: the shorter of the
// phase's own budget and what is left of the test, less one margin. A
// non-positive result means there is no room left for the phase at all.
//
// Deriving from the remaining test time rather than from the phase budget alone
// is what keeps a phase from consuming the test's whole deadline: without it the
// last phase of a slow run is killed by the test timeout, which reports as an
// opaque panic rather than as the phase that overran.
func derivePhaseTimeout(remaining, phaseTimeout time.Duration) time.Duration {
	if remaining <= 0 {
		return 0
	}
	usable := remaining - deadlineMargin
	if usable <= 0 {
		return 0
	}
	if phaseTimeout < usable {
		return phaseTimeout
	}
	return usable
}

// phaseContext derives the sub-context for one phase from the TEST's deadline
// and the phase's own budget. The parent context is not consulted for the
// budget: it carries the test deadline already, but a phase also has to finish
// well inside it, and it is the phase budget that decides by how much.
func phaseContext(t devtest.T, phaseTimeout time.Duration) (context.Context, context.CancelFunc) {
	remaining := phaseTimeout
	if deadline, ok := t.Deadline(); ok {
		remaining = derivePhaseTimeout(time.Until(deadline), phaseTimeout)
	}
	if remaining <= 0 {
		t.Require().FailNowf("phaseContext", "no time left inside the test deadline for a %v phase", phaseTimeout)
	}
	return context.WithTimeout(t.Ctx(), remaining)
}

// awaitWithin polls cond until it reports success, the timeout elapses or ctx is
// done. The last error is wrapped into the returned error, so a caller that has
// to fail can say what was still wrong instead of only that time ran out.
func awaitWithin(ctx context.Context, timeout time.Duration, cond func() error) error {
	deadline := time.Now().Add(timeout)
	var last error
	for {
		if last = cond(); last == nil {
			return nil
		}
		if !time.Now().Before(deadline) {
			return fmt.Errorf("condition not met within %s: %w", timeout, last)
		}
		select {
		case <-ctx.Done():
			return fmt.Errorf("condition not met before the context expired: %w", last)
		case <-time.After(250 * time.Millisecond):
		}
	}
}

// controlledSequencer is the lifecycle handle of one sequencer's execution
// client. Both operations report failure as an error rather than as a bare
// boolean: a helper that answered "did it stop?" with true/false would let a
// caller that ignored the answer treat a refused stop as a stopped node, which
// is the opposite of what a failover test is trying to establish.
type controlledSequencer interface {
	StopControlled(ctx context.Context) error
	StartControlled(ctx context.Context) error
}

// haCluster bundles the conductor set of the running topology with the fault
// injection handles a test needs: each conductor's own sequencer EL can be
// stopped and restarted, and each conductor's CL can be queried for its head.
type haCluster struct {
	t          devtest.T
	chainID    eth.ChainID
	conductors dsl.ConductorSet
	byName     map[string]*dsl.Conductor
	els        map[string]controlledSequencer
	dslELs     map[string]*dsl.L2ELNode
	cls        map[string]*dsl.L2CLNode
}

func newHACluster(t devtest.T) *haCluster {
	sys := presets.NewXLayerWithConductors(t)
	l2ChainID := sys.L2Chain.ChainID()
	conductors := sys.ConductorSets[l2ChainID]
	require.Len(t, conductors, 3, "expected exactly 3 conductors")

	cluster := &haCluster{
		t:          t,
		chainID:    l2ChainID,
		conductors: conductors,
		byName:     make(map[string]*dsl.Conductor, len(conductors)),
		els: map[string]controlledSequencer{
			"conductor1": sys.L2EL.Escape().(stack.ControlledLifecycle),
			"conductor2": sys.L2ELSeq2.Escape().(stack.ControlledLifecycle),
			"conductor3": sys.L2ELSeq3.Escape().(stack.ControlledLifecycle),
		},
		dslELs: map[string]*dsl.L2ELNode{
			"conductor1": sys.L2EL,
			"conductor2": sys.L2ELSeq2,
			"conductor3": sys.L2ELSeq3,
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

// stopMember stops the sequencer EL of the named member. It returns the error
// from the stop itself, and reports a missing member as an error too, so a
// caller can never mistake "this node was not the one that stopped" for a
// successful fault injection.
func (h *haCluster) stopMember(ctx context.Context, name string) error {
	el, ok := h.els[name]
	if !ok {
		return fmt.Errorf("no sequencer EL registered for member %q", name)
	}
	return el.StopControlled(ctx)
}

// startMember restarts the sequencer EL of the named member, with the same
// error-returning contract as stopMember.
func (h *haCluster) startMember(ctx context.Context, name string) error {
	el, ok := h.els[name]
	if !ok {
		return fmt.Errorf("no sequencer EL registered for member %q", name)
	}
	return el.StartControlled(ctx)
}

// awaitLeader polls the conductor set until one conductor reports it is the Raft
// leader, then returns that conductor.
func (h *haCluster) awaitLeader(ctx context.Context, timeout time.Duration) *dsl.Conductor {
	var found *dsl.Conductor
	err := awaitWithin(ctx, timeout, func() error {
		for _, c := range h.conductors {
			if c.IsLeader() {
				found = c
				return nil
			}
		}
		return fmt.Errorf("no conductor has become the Raft leader yet")
	})
	h.t.Require().NoError(err, "a conductor must become the Raft leader")
	return found
}

// awaitNewLeader polls until a conductor other than exclude reports leadership.
func (h *haCluster) awaitNewLeader(ctx context.Context, exclude string, timeout time.Duration) *dsl.Conductor {
	var found *dsl.Conductor
	err := awaitWithin(ctx, timeout, func() error {
		for _, c := range h.conductors {
			if c.String() != exclude && c.IsLeader() {
				found = c
				return nil
			}
		}
		return fmt.Errorf("no conductor other than %s has taken leadership", exclude)
	})
	h.t.Require().NoErrorf(err, "a conductor other than %s must become leader", exclude)
	return found
}

// awaitLeaderHealthy requires the leader to still hold leadership and to report
// a healthy sequencer. The health RPC is re-issued on every evaluation: a
// boolean sampled once before a trigger describes the state before it, and a
// cached true would let a conductor that never actually took over satisfy the
// check.
func (h *haCluster) awaitLeaderHealthy(ctx context.Context, leader *dsl.Conductor, timeout time.Duration) {
	err := awaitWithin(ctx, timeout, func() error {
		if !leader.IsLeader() {
			return fmt.Errorf("conductor %s no longer holds leadership", leader.String())
		}
		if !leader.FetchSequencerHealthy() {
			return fmt.Errorf("conductor %s does not report a healthy sequencer", leader.String())
		}
		return nil
	})
	h.t.Require().NoErrorf(err, "conductor %s must lead a healthy sequencer", leader.String())
}

// awaitAdvancement proves the chain is still progressing after a leadership
// change: at least one new unsafe block AND a new safe block on the conductor's
// node. The safe head only moves once the batcher has submitted a batch and L1
// has confirmed it, so a safe-head advance is the observable form of "a batch
// transaction landed". timeout is what is LEFT of the change's single shared
// window, not a fresh budget for this assertion.
func (h *haCluster) awaitAdvancement(ctx context.Context, name string, timeout time.Duration) {
	cl := h.cls[name]
	unsafeBefore := cl.HeadBlockRef(safety.LocalUnsafe).Number
	safeBefore := cl.HeadBlockRef(safety.CrossSafe).Number

	err := awaitWithin(ctx, timeout, func() error {
		unsafeNow := cl.HeadBlockRef(safety.LocalUnsafe).Number
		if unsafeNow <= unsafeBefore {
			return fmt.Errorf("no new unsafe block: %d is not above %d", unsafeNow, unsafeBefore)
		}
		safeNow := cl.HeadBlockRef(safety.CrossSafe).Number
		if safeNow <= safeBefore {
			return fmt.Errorf("no new safe block: %d is not above %d", safeNow, safeBefore)
		}
		return nil
	})
	h.t.Require().NoErrorf(err,
		"sequencer %s must produce a new block and advance its safe head within %s of the trigger", name, timeout)
}

// TestXLayerLeaderElection verifies that the seeded cluster settles on exactly one
// Raft leader with a full three-member membership and that every conductor's
// health monitor really ticks afterwards.
func TestXLayerLeaderElection(gt *testing.T) {
	requireXLayerExecutionBinary(gt)
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	h := newHACluster(t)
	ctx, cancel := phaseContext(t, 120*time.Second)
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

	h.awaitLeaderHealthy(ctx, leader, failoverDeadline)
}

// TestXLayerAutomaticFailover verifies the HA failover path with real Raft
// re-election: the current leader's own sequencer EL is stopped, a different
// conductor takes over, the chain keeps advancing, and all three conductors lead
// at some point in the ring. Each of the two leadership changes gets its own
// single shared window and its own progress proof.
func TestXLayerAutomaticFailover(gt *testing.T) {
	requireXLayerExecutionBinary(gt)
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	h := newHACluster(t)
	ctx, cancel := phaseContext(t, 600*time.Second)
	defer cancel()

	led := make(map[string]bool, 3)

	// --- Round 1: stop the leader's sequencer EL, expect a real re-election ---
	leader := h.awaitLeader(ctx, failoverDeadline)
	led[leader.String()] = true
	stopped := leader.String()

	// Let every conductor complete a health check before the trigger so the
	// followers' monitors are demonstrably live.
	awaitAllHealthy(t, ctx, h.conductors, 60*time.Second)

	// A fresh baseline per trigger, then one shared window for everything this
	// change has to prove. Survivors are asserted on: with its EL down the
	// stopped node's own success counter is expected to stall.
	survivors := make(dsl.ConductorSet, 0, len(h.conductors)-1)
	for _, c := range h.conductors {
		if c.String() != stopped {
			survivors = append(survivors, c)
		}
	}
	barrier := newHealthBarrier(t, ctx, survivors)

	deadline := time.Now().Add(triggerAdvanceDeadline)
	require.NoErrorf(t, h.stopMember(ctx, stopped), "stop the leader's sequencer EL %s", stopped)

	newLeader := h.awaitNewLeader(ctx, stopped, time.Until(deadline))
	led[newLeader.String()] = true

	h.awaitLeaderHealthy(ctx, newLeader, time.Until(deadline))
	barrier.awaitNewSample(t, ctx, survivors, time.Until(deadline))
	h.awaitAdvancement(ctx, newLeader.String(), time.Until(deadline))

	require.NoErrorf(t, h.startMember(ctx, stopped), "restart the stopped EL %s", stopped)

	// The ex-leader must rejoin as a follower, not resume leadership.
	require.NoErrorf(t, awaitWithin(ctx, time.Until(deadline), func() error {
		for _, srv := range newLeader.FetchClusterMembership().Servers {
			if srv.ID == stopped {
				if h.byName[stopped].IsLeader() {
					return fmt.Errorf("the ex-leader %s resumed leadership", stopped)
				}
				return nil
			}
		}
		return fmt.Errorf("conductor %s is not in cluster membership yet", stopped)
	}), "the ex-leader %s must rejoin as a follower", stopped)

	// --- Round 2: hand leadership to the conductor that has not led yet ---
	var third string
	for _, c := range h.conductors {
		if !led[c.String()] {
			third = c.String()
			break
		}
	}
	require.NotEmpty(t, third, "expected a conductor that has not yet led")

	// The second change gets its own baseline and its own shared window, and it
	// has to prove progress exactly like the failover did: a transfer that leaves
	// the cluster unable to produce is a failed transfer.
	secondBarrier := newHealthBarrier(t, ctx, h.conductors)
	secondDeadline := time.Now().Add(triggerAdvanceDeadline)

	newLeader.TransferLeadershipTo(h.serverInfo(newLeader, third))
	require.NoErrorf(t, awaitWithin(ctx, time.Until(secondDeadline), func() error {
		if !h.byName[third].IsLeader() {
			return fmt.Errorf("conductor %s has not taken leadership", third)
		}
		return nil
	}), "the third conductor %s must become leader", third)
	led[third] = true

	secondBarrier.awaitNewSample(t, ctx, h.conductors, time.Until(secondDeadline))
	h.awaitAdvancement(ctx, third, time.Until(secondDeadline))

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
	h.t.Require().FailNowf("serverInfo", "conductor %s must appear in cluster membership", target)
	return consensus.ServerInfo{}
}

// TestXLayerLeadershipTransfer verifies that leadership can be handed explicitly
// to every other conductor, that the outgoing leader steps down, that the
// receiving conductor emits a fresh health sample, and that the chain keeps
// producing and batching after each handover — twice, so the second transfer
// exercises a leader that was itself elected by transfer.
func TestXLayerLeadershipTransfer(gt *testing.T) {
	requireXLayerExecutionBinary(gt)
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	h := newHACluster(t)
	ctx, cancel := phaseContext(t, 300*time.Second)
	defer cancel()

	leader := h.awaitLeader(ctx, failoverDeadline)

	// Let every conductor's health monitor tick once before the first baseline.
	awaitAllHealthy(t, ctx, h.conductors, 60*time.Second)

	for _, target := range h.conductors {
		if target == leader {
			continue
		}
		old := leader

		// A fresh baseline and a fresh shared window per handover: the second
		// transfer must be proved on its own evidence, not on the first one's.
		barrier := newHealthBarrier(t, ctx, h.conductors)
		deadline := time.Now().Add(triggerAdvanceDeadline)

		old.TransferLeadershipTo(h.serverInfo(old, target.String()))

		require.NoErrorf(t, awaitWithin(ctx, time.Until(deadline), func() error {
			if !target.IsLeader() {
				return fmt.Errorf("conductor %q has not taken leadership", target.String())
			}
			if old.IsLeader() {
				return fmt.Errorf("the outgoing leader %q still holds leadership", old.String())
			}
			return nil
		}), "leadership must transfer to conductor %q and %q must step down", target.String(), old.String())

		barrier.awaitNewSample(t, ctx, dsl.ConductorSet{target}, time.Until(deadline))
		h.awaitAdvancement(ctx, target.String(), time.Until(deadline))

		leader = target
	}
}

// TestXLayerFollowerRecovery verifies that a sequencer which lost its EL rejoins
// the cluster as a follower and genuinely catches up: its peers are reconnected
// explicitly, its head is brought back level with the current leader's, the block
// at the height they share is the SAME block on both nodes, its health monitor
// emits a new sample, and the chain keeps producing throughout.
func TestXLayerFollowerRecovery(gt *testing.T) {
	requireXLayerExecutionBinary(gt)
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	h := newHACluster(t)
	ctx, cancel := phaseContext(t, 600*time.Second)
	defer cancel()

	leader := h.awaitLeader(ctx, failoverDeadline)
	stopped := leader.String()

	awaitAllHealthy(t, ctx, h.conductors, 60*time.Second)

	require.NoErrorf(t, h.stopMember(ctx, stopped), "stop the leader's sequencer EL %s", stopped)

	newLeader := h.awaitNewLeader(ctx, stopped, failoverDeadline)
	// The cluster must keep advancing while the failed node is out.
	h.awaitAdvancement(ctx, newLeader.String(), triggerAdvanceDeadline)

	// Baseline before the recovery trigger.
	recoveryBarrier := newHealthBarrier(t, ctx, h.conductors)
	deadline := time.Now().Add(triggerAdvanceDeadline)

	require.NoErrorf(t, h.startMember(ctx, stopped), "restart the stopped EL %s", stopped)

	// A restart drops the node's P2P connections, so both peer sets are rebuilt
	// explicitly. Leaving them to incidental rediscovery would let a node that is
	// present in membership but connected to nobody look recovered.
	recoveredCL := h.cls[stopped]
	leaderCL := h.cls[newLeader.String()]
	recoveredCL.ConnectPeer(leaderCL)
	leaderCL.ConnectPeer(recoveredCL)
	recoveredEL := h.dslELs[stopped]
	leaderEL := h.dslELs[newLeader.String()]
	recoveredEL.PeerWith(leaderEL)
	leaderEL.PeerWith(recoveredEL)

	// Membership must be whole again and the recovered node must be a follower.
	require.NoErrorf(t, awaitWithin(ctx, time.Until(deadline), func() error {
		for _, srv := range newLeader.FetchClusterMembership().Servers {
			if srv.ID == stopped {
				if h.byName[stopped].IsLeader() {
					return fmt.Errorf("the recovered node %s resumed leadership", stopped)
				}
				return nil
			}
		}
		return fmt.Errorf("conductor %s is not in cluster membership yet", stopped)
	}), "the recovered node %s must rejoin as a follower", stopped)

	// Real catch-up: presence in membership is not proof of synchronisation, so
	// the recovered node's own head is compared against the leader's until it is
	// level, and the height the two then share is compared by HASH. Two nodes can
	// agree on a height while holding different blocks, which is a fork, and a
	// number-only check would call that caught up.
	var sharedHeight uint64
	require.NoErrorf(t, awaitWithin(ctx, time.Until(deadline), func() error {
		leaderHead := leaderCL.HeadBlockRef(safety.LocalUnsafe)
		recoveredHead := recoveredCL.HeadBlockRef(safety.LocalUnsafe)
		if recoveredHead.Number+2 < leaderHead.Number {
			return fmt.Errorf("recovered head %d is still behind the leader's %d",
				recoveredHead.Number, leaderHead.Number)
		}
		sharedHeight = min(leaderHead.Number, recoveredHead.Number)
		return nil
	}), "the recovered node %s must catch up to the leader", stopped)

	leaderID, leaderErr := leaderCL.ChainBlockID(h.chainID, sharedHeight)
	require.NoErrorf(t, leaderErr, "read the leader's block at height %d", sharedHeight)
	recoveredID, recoveredErr := recoveredCL.ChainBlockID(h.chainID, sharedHeight)
	require.NoErrorf(t, recoveredErr, "read the recovered node's block at height %d", sharedHeight)
	require.Equalf(t, leaderID.Hash, recoveredID.Hash,
		"the recovered node %s must hold the leader's block %s at height %d, got %s",
		stopped, leaderID.Hash, sharedHeight, recoveredID.Hash)

	// The recovered node emits a new health sample, and the chain still advances.
	recoveryBarrier.awaitNewSample(t, ctx, dsl.ConductorSet{h.byName[stopped]}, time.Until(deadline))
	h.awaitAdvancement(ctx, stopped, time.Until(deadline))
}

// TestXLayerProgression verifies that both the unsafe and the safe head advance
// monotonically over a sustained window, which requires the HA batcher to keep
// following the active sequencer. Every step is compared against the PREVIOUS
// reading, not against the reading taken when the window opened, so a head that
// stalls and then catches up is not hidden by the endpoints agreeing.
func TestXLayerProgression(gt *testing.T) {
	requireXLayerExecutionBinary(gt)
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	h := newHACluster(t)
	ctx, cancel := phaseContext(t, 300*time.Second)
	defer cancel()

	leader := h.awaitLeader(ctx, failoverDeadline)
	awaitAllHealthy(t, ctx, h.conductors, 60*time.Second)

	cl := h.cls[leader.String()]
	prevUnsafe := cl.HeadBlockRef(safety.LocalUnsafe).Number
	prevSafe := cl.HeadBlockRef(safety.CrossSafe).Number
	firstUnsafe, firstSafe := prevUnsafe, prevSafe

	deadline := time.Now().Add(progressionWindow)
	for time.Now().Before(deadline) {
		time.Sleep(1 * time.Second)
		unsafeNow := cl.HeadBlockRef(safety.LocalUnsafe).Number
		safeNow := cl.HeadBlockRef(safety.CrossSafe).Number
		require.GreaterOrEqualf(t, unsafeNow, prevUnsafe,
			"the unsafe head must not regress: %d -> %d", prevUnsafe, unsafeNow)
		require.GreaterOrEqualf(t, safeNow, prevSafe,
			"the safe head must not regress: %d -> %d", prevSafe, safeNow)
		prevUnsafe, prevSafe = unsafeNow, safeNow
	}

	require.Greaterf(t, prevUnsafe, firstUnsafe,
		"the unsafe head must strictly advance over the window: %d -> %d", firstUnsafe, prevUnsafe)
	require.Greaterf(t, prevSafe, firstSafe,
		"the safe head must strictly advance over the window: %d -> %d", firstSafe, prevSafe)
}

// TestXLayerMemberStopPreservesQuorum verifies that losing a non-leader's
// sequencer does not disturb the sequencing quorum.
func TestXLayerMemberStopPreservesQuorum(gt *testing.T) {
	requireXLayerExecutionBinary(gt)
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	h := newHACluster(t)
	ctx, cancel := phaseContext(t, 120*time.Second)
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
	require.NoErrorf(t, h.stopMember(ctx, follower.String()), "stop follower EL %s", follower.String())

	h.awaitLeaderHealthy(ctx, leader, failoverDeadline)
}

// TestXLayerBatcherAdvancesSafeHead verifies that the single HA batcher keeps the
// safe head moving on the active sequencer.
func TestXLayerBatcherAdvancesSafeHead(gt *testing.T) {
	requireXLayerExecutionBinary(gt)
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	h := newHACluster(t)
	ctx, cancel := phaseContext(t, 300*time.Second)
	defer cancel()

	leader := h.awaitLeader(ctx, failoverDeadline)
	awaitAllHealthy(t, ctx, h.conductors, 60*time.Second)

	h.awaitAdvancement(ctx, leader.String(), triggerAdvanceDeadline)
}
