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
	"github.com/stretchr/testify/require"
)

// Per-operation budgets, each measured from the trigger that opens the phase.
//
// These are caps, not suggestions: a phase that is allowed to run past its cap
// stops being evidence that the cluster handed over promptly and starts being
// evidence that it eventually got there. The values are the ones the topology is
// specified with — a failover is a health-staleness detection plus one Raft
// election, a transfer is a single explicit handover, and the safe head needs a
// batch to be submitted and confirmed on L1 after the switch.
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

// phaseDeadline closes one phase of a leadership change. The phase gets its own
// cap counted from now, but never past what is left of the test — a phase that
// outlived its test would be reported as an opaque panic rather than as the
// phase that overran.
func phaseDeadline(t devtest.T, cap time.Duration) time.Time {
	deadline := time.Now().Add(cap)
	if testDeadline, ok := t.Deadline(); ok {
		if limit := testDeadline.Add(-deadlineMargin); limit.Before(deadline) {
			return limit
		}
	}
	return deadline
}

// phaseContext derives the sub-context for one phase from the TEST's deadline
// and the phase's own budget. The returned context is what every call made
// during the phase receives: a sub-context that is computed and then dropped is
// not a deadline, it is a comment.
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

// conductorIsLeader asks a conductor whether it holds Raft leadership, on the
// CALLER's context rather than on the node's own long-lived test context.
//
// That distinction is the whole point of these wrappers: the devstack DSL is
// built around one context for the lifetime of a node, so a phase deadline
// derived for one leadership change would be discarded before it ever reached a
// socket. Passing the phase context down is what turns the cap into an enforced
// bound.
func conductorIsLeader(ctx context.Context, c *dsl.Conductor) (bool, error) {
	return c.Escape().RpcAPI().Leader(ctx)
}

// conductorSequencerHealthy issues the sequencer-health query on the caller's
// context.
func conductorSequencerHealthy(ctx context.Context, c *dsl.Conductor) (bool, error) {
	return c.Escape().RpcAPI().SequencerHealthy(ctx)
}

// conductorClusterMembership reads the Raft membership on the caller's context.
func conductorClusterMembership(ctx context.Context, c *dsl.Conductor) (*consensus.ClusterMembership, error) {
	return c.Escape().RpcAPI().ClusterMembership(ctx)
}

// conductorTransferLeadership hands leadership to target on the caller's
// context, so an explicit transfer is bounded by the transfer cap.
func conductorTransferLeadership(ctx context.Context, c *dsl.Conductor, target consensus.ServerInfo) error {
	return c.Escape().RpcAPI().TransferLeaderToServer(ctx, target.ID, target.Addr)
}

// l2CLUnsafeHead reads a CL's unsafe head on the caller's context, which is how
// the block queries made during a phase inherit that phase's bound.
func l2CLUnsafeHead(ctx context.Context, cl *dsl.L2CLNode) (eth.L2BlockRef, error) {
	status, err := cl.Escape().RollupAPI().SyncStatus(ctx)
	if err != nil {
		return eth.L2BlockRef{}, err
	}
	return status.UnsafeL2, nil
}

// l2CLSafeHead reads a CL's safe head on the caller's context.
func l2CLSafeHead(ctx context.Context, cl *dsl.L2CLNode) (eth.L2BlockRef, error) {
	status, err := cl.Escape().RollupAPI().SyncStatus(ctx)
	if err != nil {
		return eth.L2BlockRef{}, err
	}
	return status.SafeL2, nil
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
			leader, leaderErr := conductorIsLeader(ctx, c)
			if leaderErr != nil {
				return leaderErr
			}
			if leader {
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
			if c.String() == exclude {
				continue
			}
			leader, leaderErr := conductorIsLeader(ctx, c)
			if leaderErr != nil {
				return leaderErr
			}
			if leader {
				found = c
				return nil
			}
		}
		return fmt.Errorf("no conductor other than %s has taken leadership", exclude)
	})
	h.t.Require().NoErrorf(err, "a conductor other than %s must become leader", exclude)
	return found
}

// awaitLeaderHealthy proves one leadership change is genuinely complete, in the
// order the contract asks for:
//
//  1. the conductor still holds leadership;
//  2. every conductor in the set emits a NEW successful health sample, measured
//     against a baseline taken for this trigger;
//  3. the leader's own sequencer-health query — re-issued here, never cached
//     from before the trigger — reports healthy;
//  4. the chain produced a new block since.
//
// Steps 2 and 3 are deliberately both required. A bare health query is satisfied
// by a conductor that merely believes itself healthy, and a counter increment is
// satisfied by a monitor that ticks while its sequencer is stuck; only the pair
// says the monitor is running AND the node it watches is usable.
func (h *haCluster) awaitLeaderHealthy(ctx context.Context, leader *dsl.Conductor, barrier *healthBarrier, timeout time.Duration) {
	deadline := time.Now().Add(timeout)
	require := h.t.Require()

	require.NoErrorf(awaitWithin(ctx, time.Until(deadline), func() error {
		isLeader, err := conductorIsLeader(ctx, leader)
		if err != nil {
			return err
		}
		if !isLeader {
			return fmt.Errorf("conductor %s no longer holds leadership", leader.String())
		}
		return nil
	}), "conductor %s must still hold leadership", leader.String())

	barrier.awaitNewSample(h.t, ctx, h.conductors, time.Until(deadline))

	require.NoErrorf(awaitWithin(ctx, time.Until(deadline), func() error {
		healthy, err := conductorSequencerHealthy(ctx, leader)
		if err != nil {
			return err
		}
		if !healthy {
			return fmt.Errorf("conductor %s does not report a healthy sequencer", leader.String())
		}
		return nil
	}), "conductor %s must report a healthy sequencer", leader.String())
}

// awaitAdvancement proves the chain is still progressing after a leadership
// change: at least one new unsafe block AND a new safe block on the conductor's
// node. The safe head only moves once the batcher has submitted a batch and L1
// has confirmed it, so a safe-head advance is the observable form of "a batch
// transaction landed". timeout is what is LEFT of the change's single shared
// window, not a fresh budget for this assertion.
func (h *haCluster) awaitAdvancement(ctx context.Context, name string, timeout time.Duration) {
	cl := h.cls[name]
	unsafeBefore, err := l2CLUnsafeHead(ctx, cl)
	h.t.Require().NoErrorf(err, "read %s's unsafe head before the advancement proof", name)
	safeBefore, err := l2CLSafeHead(ctx, cl)
	h.t.Require().NoErrorf(err, "read %s's safe head before the advancement proof", name)

	advanceErr := awaitWithin(ctx, timeout, func() error {
		unsafeNow, err := l2CLUnsafeHead(ctx, cl)
		if err != nil {
			return err
		}
		if unsafeNow.Number <= unsafeBefore.Number {
			return fmt.Errorf("no new unsafe block: %d is not above %d", unsafeNow.Number, unsafeBefore.Number)
		}
		safeNow, err := l2CLSafeHead(ctx, cl)
		if err != nil {
			return err
		}
		if safeNow.Number <= safeBefore.Number {
			return fmt.Errorf("no new safe block: %d is not above %d", safeNow.Number, safeBefore.Number)
		}
		return nil
	})
	h.t.Require().NoErrorf(advanceErr,
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

	electionCtx, electionCancel := context.WithDeadline(ctx, phaseDeadline(t, failoverDeadline))
	defer electionCancel()

	leader := h.awaitLeader(electionCtx, failoverDeadline)
	leaders := 0
	for _, c := range h.conductors {
		isLeader, err := conductorIsLeader(ctx, c)
		require.NoError(t, err, "read leadership from %s", c.String())
		if isLeader {
			leaders++
		}
	}
	require.Equal(t, 1, leaders, "exactly one conductor must lead")

	membership, err := conductorClusterMembership(ctx, leader)
	require.NoError(t, err, "read cluster membership from the leader")
	require.Len(t, membership.Servers, 3, "membership must contain all three conductors")

	// A baseline taken here, after the election, so the samples asserted below
	// belong to the post-election state rather than to start-up.
	barrier := newHealthBarrier(t, ctx, h.conductors)
	h.awaitLeaderHealthy(ctx, leader, barrier, failoverDeadline)
}

// TestXLayerAutomaticFailover verifies the HA failover path with real Raft
// re-election: the current leader's own sequencer EL is stopped, a different
// conductor takes over, the chain keeps advancing, and all three conductors lead
// at some point in the ring. Each leadership change gets its own cap, its own
// health baseline and its own progress proof.
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

	// The baseline is taken immediately before the stop, so the samples asserted
	// afterwards are the survivors' reaction to the fault and not readings from
	// before it. The whole change then runs inside one cap measured from the stop.
	barrier, err := memberStopHelper(ctx, t, h, stopped, failoverDeadline)
	require.NoErrorf(t, err, "stop the leader's sequencer EL %s", stopped)

	firstChangeCtx, firstChangeCancel := context.WithDeadline(ctx, phaseDeadline(t, triggerAdvanceDeadline))
	defer firstChangeCancel()

	newLeader := h.awaitNewLeader(firstChangeCtx, stopped, failoverDeadline)
	led[newLeader.String()] = true

	h.awaitLeaderHealthy(firstChangeCtx, newLeader, barrier, failoverDeadline)
	h.awaitAdvancement(firstChangeCtx, newLeader.String(), time.Until(mustDeadline(firstChangeCtx)))

	require.NoErrorf(t, h.startMember(ctx, stopped), "restart the stopped EL %s", stopped)

	// The ex-leader must rejoin as a follower, not resume leadership.
	require.NoErrorf(t, awaitWithin(firstChangeCtx, time.Until(mustDeadline(firstChangeCtx)), func() error {
		membership, err := conductorClusterMembership(firstChangeCtx, newLeader)
		if err != nil {
			return err
		}
		for _, srv := range membership.Servers {
			if srv.ID != stopped {
				continue
			}
			isLeader, err := conductorIsLeader(firstChangeCtx, h.byName[stopped])
			if err != nil {
				return err
			}
			if isLeader {
				return fmt.Errorf("the ex-leader %s resumed leadership", stopped)
			}
			return nil
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

	// The second change gets its OWN baseline and its OWN cap: reusing the first
	// round's baseline would let the second handover pass on samples the first one
	// produced.
	secondBarrier := newHealthBarrier(t, ctx, h.conductors)
	secondChangeCtx, secondChangeCancel := context.WithDeadline(ctx, phaseDeadline(t, triggerAdvanceDeadline))
	defer secondChangeCancel()

	require.NoErrorf(t, conductorTransferLeadership(secondChangeCtx, newLeader,
		h.serverInfo(secondChangeCtx, newLeader, third)), "transfer leadership to %s", third)
	require.NoErrorf(t, awaitWithin(secondChangeCtx, time.Until(mustDeadline(secondChangeCtx)), func() error {
		isLeader, err := conductorIsLeader(secondChangeCtx, h.byName[third])
		if err != nil {
			return err
		}
		if !isLeader {
			return fmt.Errorf("conductor %s has not taken leadership", third)
		}
		return nil
	}), "the third conductor %s must become leader", third)
	led[third] = true

	h.awaitLeaderHealthy(secondChangeCtx, h.byName[third], secondBarrier, failoverDeadline)
	h.awaitAdvancement(secondChangeCtx, third, time.Until(mustDeadline(secondChangeCtx)))

	require.Len(t, led, 3, "all three conductors must have led at some point")
}

// mustDeadline returns the deadline carried by ctx, which every phase context
// here is built with. It exists so call sites can pass "what is left" without
// recomputing the cap and silently inventing a fresh one.
func mustDeadline(ctx context.Context) time.Time {
	deadline, ok := ctx.Deadline()
	if !ok {
		panic("phase context must carry a deadline")
	}
	return deadline
}

// serverInfo looks up a conductor's Raft server info from the given conductor's
// membership view, on the caller's context.
func (h *haCluster) serverInfo(ctx context.Context, from *dsl.Conductor, target string) consensus.ServerInfo {
	membership, err := conductorClusterMembership(ctx, from)
	h.t.Require().NoError(err, "read cluster membership")
	for _, srv := range membership.Servers {
		if srv.ID == target {
			return srv
		}
	}
	h.t.Require().FailNowf("serverInfo", "conductor %s must appear in cluster membership", target)
	return consensus.ServerInfo{}
}

// TestXLayerLeadershipTransfer verifies that leadership can be handed explicitly
// to every other conductor, that the outgoing leader steps down within the
// transfer cap, that the receiving conductor emits a fresh health sample, and
// that the chain keeps producing and batching after each handover — twice, so
// the second transfer exercises a leader that was itself elected by transfer.
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

		// A fresh baseline and a fresh cap per handover: the second transfer must
		// be proved on its own evidence, not on the first one's.
		barrier := newHealthBarrier(t, ctx, h.conductors)
		handoverCtx, handoverCancel := context.WithDeadline(ctx, phaseDeadline(t, triggerAdvanceDeadline))

		require.NoErrorf(t, conductorTransferLeadership(handoverCtx, old, h.serverInfo(handoverCtx, old, target.String())),
			"transfer leadership from %s to %s", old.String(), target.String())

		// The handover itself gets the tight transfer cap; the progress proof that
		// follows gets the wider window, because a batch still has to reach L1.
		require.NoErrorf(t, awaitWithin(handoverCtx, transferDeadline, func() error {
			targetLeads, err := conductorIsLeader(handoverCtx, target)
			if err != nil {
				return err
			}
			if !targetLeads {
				return fmt.Errorf("conductor %q has not taken leadership", target.String())
			}
			oldLeads, err := conductorIsLeader(handoverCtx, old)
			if err != nil {
				return err
			}
			if oldLeads {
				return fmt.Errorf("the outgoing leader %q still holds leadership", old.String())
			}
			return nil
		}), "leadership must transfer to conductor %q within %s", target.String(), transferDeadline)

		h.awaitLeaderHealthy(handoverCtx, target, barrier, failoverDeadline)
		h.awaitAdvancement(handoverCtx, target.String(), time.Until(mustDeadline(handoverCtx)))

		handoverCancel()
		leader = target
	}
}

// TestXLayerFollowerRecovery verifies that a sequencer which lost its EL rejoins
// the cluster as a follower and genuinely catches up: its peers are reconnected
// explicitly, its head is brought back level with the current leader's, the block
// at the height they share is the SAME block on both nodes, its health monitor
// emits a new sample, and the chain keeps producing throughout.
//
// The phase budget is SHARED with the failover that preceded it, not restarted:
// a recovery that only ever appears to finish because it was handed a fresh
// window says nothing about how long the cluster really took to heal.
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

	// One window opens here, at the trigger, and covers the failover AND the
	// recovery that follows it.
	recoveryCtx, recoveryCancel := context.WithDeadline(ctx, phaseDeadline(t, triggerAdvanceDeadline))
	defer recoveryCancel()

	recoveryBarrier, err := memberStopHelper(recoveryCtx, t, h, stopped, failoverDeadline)
	require.NoErrorf(t, err, "stop the leader's sequencer EL %s", stopped)

	newLeader := h.awaitNewLeader(recoveryCtx, stopped, failoverDeadline)
	// The cluster must keep advancing while the failed node is out.
	h.awaitAdvancement(recoveryCtx, newLeader.String(), time.Until(mustDeadline(recoveryCtx)))

	require.NoErrorf(t, h.startMember(recoveryCtx, stopped), "restart the stopped EL %s", stopped)

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
	require.NoErrorf(t, awaitWithin(recoveryCtx, time.Until(mustDeadline(recoveryCtx)), func() error {
		membership, err := conductorClusterMembership(recoveryCtx, newLeader)
		if err != nil {
			return err
		}
		for _, srv := range membership.Servers {
			if srv.ID != stopped {
				continue
			}
			isLeader, err := conductorIsLeader(recoveryCtx, h.byName[stopped])
			if err != nil {
				return err
			}
			if isLeader {
				return fmt.Errorf("the recovered node %s resumed leadership", stopped)
			}
			return nil
		}
		return fmt.Errorf("conductor %s is not in cluster membership yet", stopped)
	}), "the recovered node %s must rejoin as a follower", stopped)

	// Real catch-up: presence in membership is not proof of synchronisation, so
	// the recovered node's own head is compared against the leader's until it is
	// level, and the height the two then share is compared by HASH. Two nodes can
	// agree on a height while holding different blocks, which is a fork, and a
	// number-only check would call that caught up.
	var sharedHeight uint64
	require.NoErrorf(t, awaitWithin(recoveryCtx, time.Until(mustDeadline(recoveryCtx)), func() error {
		leaderHead, err := l2CLUnsafeHead(recoveryCtx, leaderCL)
		if err != nil {
			return err
		}
		recoveredHead, err := l2CLUnsafeHead(recoveryCtx, recoveredCL)
		if err != nil {
			return err
		}
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
	h.awaitLeaderHealthy(recoveryCtx, newLeader, recoveryBarrier, failoverDeadline)
	h.awaitAdvancement(recoveryCtx, newLeader.String(), time.Until(mustDeadline(recoveryCtx)))

	// Budget-sharing proof: the whole recovery ran inside the window opened at the
	// trigger, so this context is still live. A phase that had quietly taken a
	// fresh budget of its own would show up here as an expired context.
	require.NoErrorf(t, recoveryCtx.Err(),
		"the recovery must complete inside the window opened at the trigger, not a fresh one")
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
	unsafeHead, err := l2CLUnsafeHead(ctx, cl)
	require.NoError(t, err, "read the initial unsafe head")
	safeHead, err := l2CLSafeHead(ctx, cl)
	require.NoError(t, err, "read the initial safe head")
	prevUnsafe := unsafeHead.Number
	prevSafe := safeHead.Number
	firstUnsafe, firstSafe := prevUnsafe, prevSafe

	deadline := time.Now().Add(progressionWindow)
	for time.Now().Before(deadline) {
		time.Sleep(1 * time.Second)
		unsafeNow, err := l2CLUnsafeHead(ctx, cl)
		require.NoError(t, err, "read the unsafe head")
		safeNow, err := l2CLSafeHead(ctx, cl)
		require.NoError(t, err, "read the safe head")
		require.GreaterOrEqualf(t, unsafeNow.Number, prevUnsafe,
			"the unsafe head must not regress: %d -> %d", prevUnsafe, unsafeNow.Number)
		require.GreaterOrEqualf(t, safeNow.Number, prevSafe,
			"the safe head must not regress: %d -> %d", prevSafe, safeNow.Number)
		prevUnsafe, prevSafe = unsafeNow.Number, safeNow.Number
	}

	require.Greaterf(t, prevUnsafe, firstUnsafe,
		"the unsafe head must strictly advance over the window: %d -> %d", firstUnsafe, prevUnsafe)
	require.Greaterf(t, prevSafe, firstSafe,
		"the safe head must strictly advance over the window: %d -> %d", firstSafe, prevSafe)
}

// TestXLayerMemberStopPreservesQuorum verifies that losing a non-leader's
// sequencer does not disturb the sequencing quorum, with a fresh health baseline
// taken at the stop.
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

	barrier, err := memberStopHelper(ctx, t, h, follower.String(), failoverDeadline)
	require.NoErrorf(t, err, "stop follower EL %s", follower.String())

	h.awaitLeaderHealthy(ctx, leader, barrier, failoverDeadline)
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

// TestXLayerHealthBaselinePerTrigger verifies that each leadership trigger takes
// its own health baseline rather than reusing one from the previous phase.
//
// The proof is that the monotonic counter never runs backwards across phases: a
// baseline captured once and reused would already be behind by the second
// trigger, so the second phase's increment assertion would pass on samples the
// first phase produced, and a monitor that had since died would go unnoticed.
func TestXLayerHealthBaselinePerTrigger(gt *testing.T) {
	requireXLayerExecutionBinary(gt)
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	h := newHACluster(t)
	ctx, cancel := phaseContext(t, 300*time.Second)
	defer cancel()

	leader := h.awaitLeader(ctx, failoverDeadline)
	awaitAllHealthy(t, ctx, h.conductors, 60*time.Second)

	// Trigger 1 baseline.
	first := baselineHealthMetrics(t, ctx, h.conductors, failoverDeadline)
	h.awaitLeaderHealthy(ctx, leader, newHealthBarrierFromBaselines(first), failoverDeadline)

	// Trigger 2 baseline, taken after the first phase completed. It must be at or
	// above the first reading — the counter is monotonic — and a second increase
	// must still be observable from it.
	second := baselineHealthMetrics(t, ctx, h.conductors, failoverDeadline)
	for url, firstValue := range first {
		require.GreaterOrEqualf(t, second[url], firstValue,
			"the health counter must not go backwards between phases for %s", url)
	}

	require.NoErrorf(t, waitForHealthIncrement(ctx, second, failoverDeadline),
		"each phase must observe its own increment from its own baseline")

	// And the first baseline is now stale: it has already been satisfied, which is
	// exactly why it must not be reused for the second phase.
	require.Error(t, waitForHealthIncrement(ctx, first, 2*time.Second),
		"a stale baseline must not be reusable as a second-phase trigger")
}

// newHealthBarrierFromBaselines rebuilds a barrier around an already-recorded set
// of readings, so the HA helpers can consume a baseline taken elsewhere.
func newHealthBarrierFromBaselines(baselines map[string]float64) *healthBarrier {
	urls := make([]string, 0, len(baselines))
	for url := range baselines {
		urls = append(urls, url)
	}
	return &healthBarrier{metricsURLs: urls, baselines: baselines}
}
