package xlayer

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
	"github.com/stretchr/testify/require"
)

// stubSequencer stands in for one sequencer's execution client. It exists so the
// error-returning member helpers can be exercised without the XLayer topology:
// what is under test is the helpers' failure contract, not the node.
type stubSequencer struct {
	stopErr  error
	startErr error
	stopped  bool
	started  bool
}

func (s *stubSequencer) StopControlled(context.Context) error {
	if s.stopErr != nil {
		return s.stopErr
	}
	s.stopped = true
	return nil
}

func (s *stubSequencer) StartControlled(context.Context) error {
	if s.startErr != nil {
		return s.startErr
	}
	s.started = true
	return nil
}

// TestDerivePhaseTimeout covers the deadline arithmetic on its own, without any
// topology: a phase gets the shorter of its own budget and what is left of the
// test, minus the safety margin, and never a negative duration.
func TestDerivePhaseTimeout(t *testing.T) {
	tests := []struct {
		name         string
		remaining    time.Duration
		phaseTimeout time.Duration
		want         time.Duration
	}{
		{name: "phase budget is the binding limit", remaining: 300 * time.Second, phaseTimeout: 30 * time.Second, want: 30 * time.Second},
		{name: "remaining test time is the binding limit", remaining: 20 * time.Second, phaseTimeout: 30 * time.Second, want: 19 * time.Second},
		{name: "exactly one margin left leaves nothing usable", remaining: 1 * time.Second, phaseTimeout: 30 * time.Second, want: 0},
		{name: "less than one margin left leaves nothing usable", remaining: 500 * time.Millisecond, phaseTimeout: 30 * time.Second, want: 0},
		{name: "no remaining test time", remaining: 0, phaseTimeout: 30 * time.Second, want: 0},
		{name: "negative remaining test time", remaining: -5 * time.Second, phaseTimeout: 30 * time.Second, want: 0},
		{name: "margin is exactly the subject", remaining: 31 * time.Second, phaseTimeout: 30 * time.Second, want: 30 * time.Second},
		{name: "short phase budget is never widened", remaining: 300 * time.Second, phaseTimeout: time.Second, want: time.Second},
		{name: "zero phase budget stays zero", remaining: 300 * time.Second, phaseTimeout: 0, want: 0},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			require.Equal(t, tc.want, derivePhaseTimeout(tc.remaining, tc.phaseTimeout))
		})
	}
}

// TestAwaitWithinReturnsConditionError verifies that a condition which never
// holds surfaces the CONDITION's own error, not a bare timeout: the caller has
// to be able to say what was still wrong when the window closed.
func TestAwaitWithinReturnsConditionError(t *testing.T) {
	sentinel := errors.New("still not there")
	err := awaitWithin(context.Background(), 50*time.Millisecond, func() error { return sentinel })
	require.Error(t, err)
	require.ErrorIs(t, err, sentinel)
}

// TestAwaitWithinStopsOnSuccess verifies that a condition which holds returns
// without waiting out the window.
func TestAwaitWithinStopsOnSuccess(t *testing.T) {
	calls := 0
	start := time.Now()
	err := awaitWithin(context.Background(), 30*time.Second, func() error {
		calls++
		if calls < 2 {
			return errors.New("not yet")
		}
		return nil
	})
	require.NoError(t, err)
	require.Equal(t, 2, calls)
	require.Less(t, time.Since(start), 30*time.Second, "a satisfied condition must not wait out the window")
}

// TestAwaitWithinHonoursContext verifies that a cancelled context aborts the
// wait instead of running to the timeout.
func TestAwaitWithinHonoursContext(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	go func() {
		time.Sleep(50 * time.Millisecond)
		cancel()
	}()
	start := time.Now()
	err := awaitWithin(ctx, 30*time.Second, func() error { return errors.New("never satisfied") })
	require.Error(t, err)
	require.Less(t, time.Since(start), 10*time.Second, "a cancelled context must end the wait")
}

// TestStopMemberReturnsStopError verifies that a refused stop is reported as an
// error and is not mistaken for a successful fault injection.
func TestStopMemberReturnsStopError(t *testing.T) {
	sentinel := errors.New("stop refused")
	stub := &stubSequencer{stopErr: sentinel}
	h := &haCluster{t: devtest.SerialT(t), els: map[string]controlledSequencer{"conductor2": stub}}

	err := h.stopMember(context.Background(), "conductor2")
	require.ErrorIs(t, err, sentinel)
	require.False(t, stub.stopped, "a refused stop must not be recorded as stopped")
}

// TestStopMemberSucceeds verifies the success path returns nil.
func TestStopMemberSucceeds(t *testing.T) {
	stub := &stubSequencer{}
	h := &haCluster{t: devtest.SerialT(t), els: map[string]controlledSequencer{"conductor2": stub}}

	require.NoError(t, h.stopMember(context.Background(), "conductor2"))
	require.True(t, stub.stopped)
}

// TestStopAndStartMemberUnknownName verifies that naming a member the cluster
// does not hold is an error rather than a silent no-op, so a test cannot inject
// a fault into nothing and then assert on it.
func TestStopAndStartMemberUnknownName(t *testing.T) {
	h := &haCluster{t: devtest.SerialT(t), els: map[string]controlledSequencer{"conductor2": &stubSequencer{}}}

	require.Error(t, h.stopMember(context.Background(), "conductor9"))
	require.Error(t, h.startMember(context.Background(), "conductor9"))
}

// TestStartMemberReturnsStartError verifies the restart side of the same
// contract.
func TestStartMemberReturnsStartError(t *testing.T) {
	sentinel := errors.New("start refused")
	stub := &stubSequencer{startErr: sentinel}
	h := &haCluster{t: devtest.SerialT(t), els: map[string]controlledSequencer{"conductor3": stub}}

	err := h.startMember(context.Background(), "conductor3")
	require.ErrorIs(t, err, sentinel)
	require.False(t, stub.started)
}
