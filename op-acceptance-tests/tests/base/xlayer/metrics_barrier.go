package xlayer

import (
	"context"
	"time"

	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
	"github.com/ethereum-optimism/optimism/op-devstack/dsl"
)

// awaitLeader polls the given ConductorSet until one conductor reports it is
// the Raft leader, then returns that conductor. The caller's context (ctx)
// controls the deadline; the returned conductor is the leader at the time of
// the check.
func awaitLeader(t devtest.T, ctx context.Context, conductors dsl.ConductorSet, timeout time.Duration) *dsl.Conductor {
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		for _, c := range conductors {
			if c.IsLeader() {
				return c
			}
		}
		select {
		case <-ctx.Done():
			t.Require().FailNow("context expired while waiting for a conductor leader")
		case <-time.After(500 * time.Millisecond):
		}
	}
	t.Require().FailNow("timed out waiting for a conductor leader")
	return nil
}

// awaitAllHealthy polls until every conductor in the set reports that its
// sequencer is healthy.
func awaitAllHealthy(t devtest.T, ctx context.Context, conductors dsl.ConductorSet, timeout time.Duration) {
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		allOK := true
		for _, c := range conductors {
			if !c.FetchSequencerHealthy() {
				allOK = false
				break
			}
		}
		if allOK {
			return
		}
		select {
		case <-ctx.Done():
			t.Require().FailNow("context expired while waiting for all conductors to be healthy")
		case <-time.After(500 * time.Millisecond):
		}
	}
	t.Require().FailNow("timed out waiting for all conductors to report healthy sequencers")
}
