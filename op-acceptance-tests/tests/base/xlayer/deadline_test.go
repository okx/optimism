package xlayer

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
	"github.com/stretchr/testify/require"
)

// delayedMetricsServer serves a real Prometheus endpoint whose every response is
// delayed by delay.
//
// The delay is what makes the assertions below evidence rather than arithmetic:
// a deadline passed to a call that would have returned immediately anyway proves
// nothing, and the failure being reproduced here is exactly a call that outlives
// the context it was given.
func delayedMetricsServer(delay time.Duration) *httptest.Server {
	mux := http.NewServeMux()
	mux.HandleFunc("/metrics", func(w http.ResponseWriter, r *http.Request) {
		time.Sleep(delay)
		w.Header().Set("Content-Type", "text/plain")
		fmt.Fprintf(w, "# TYPE %s counter\n%s{success=\"true\"} 1\n", healthCheckCounterName, healthCheckCounterName)
	})
	return httptest.NewServer(mux)
}

// isDeadlineError reports whether err is, or wraps, a context deadline failure —
// as opposed to some other failure that also happens to be non-nil.
func isDeadlineError(err error) bool {
	if err == nil {
		return false
	}
	if errors.Is(err, context.DeadlineExceeded) {
		return true
	}
	return strings.Contains(strings.ToLower(err.Error()), "deadline")
}

// TestDeadlineEnforcement is the already-expired reproduction: a 10ms deadline
// against a 120ms response must FAIL, not return a value.
//
// The failure mode this guards against is subtle: a deadline that is computed
// and then dropped before the call is indistinguishable from one that works,
// because nothing observes the missing bound until a request is actually slow.
// Here the request is slow on purpose, so a discarded deadline shows up as a
// successful read where a deadline error was required.
func TestDeadlineEnforcement(t *testing.T) {
	server := delayedMetricsServer(120 * time.Millisecond)
	defer server.Close()

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Millisecond)
	defer cancel()

	value, err := scrapeHealthCheckCount(ctx, server.URL+"/metrics")

	require.Error(t, err, "a 10ms deadline against a 120ms response must fail, not return a reading")
	require.Truef(t, isDeadlineError(err), "the failure must be a deadline error, got: %v", err)
	require.Zero(t, value, "no reading may be reported for a request that never completed")
}

// TestDeadlineEnforcementSlowRequest is the same contract at a longer scale: a
// 2s deadline against a 5s response must error rather than silently succeed.
//
// It exists separately from the 10ms case because the two exercise different
// phases of the request: the short one fails during connection or write, the
// long one while waiting on the response, and a bound that only covered the
// first would pass the first test and fail this one.
func TestDeadlineEnforcementSlowRequest(t *testing.T) {
	server := delayedMetricsServer(5 * time.Second)
	defer server.Close()

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	start := time.Now()
	value, err := scrapeHealthCheckCount(ctx, server.URL+"/metrics")
	elapsed := time.Since(start)

	require.Error(t, err, "a 2s deadline against a 5s response must fail, not succeed")
	require.Truef(t, isDeadlineError(err), "the failure must be a deadline error, got: %v", err)
	require.Zero(t, value)
	require.Lessf(t, elapsed, 4*time.Second,
		"the call must return when its deadline expires, not when the server finally answers; took %s", elapsed)
}

// TestDeadlineEnforcementHealthyPath verifies the other half of the contract: a
// deadline that is comfortably longer than the response must not fail the read.
// Without this, a bound that rejected everything would pass both tests above.
func TestDeadlineEnforcementHealthyPath(t *testing.T) {
	server := delayedMetricsServer(20 * time.Millisecond)
	defer server.Close()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	value, err := scrapeHealthCheckCount(ctx, server.URL+"/metrics")

	require.NoError(t, err, "a response that fits inside the deadline must be read normally")
	require.Equal(t, float64(1), value)
}

// TestWaitForHealthIncrementHonoursDeadline verifies that the increment wait
// inherits the caller's bound: against an endpoint that never moves, it must
// return a deadline error instead of waiting forever.
func TestWaitForHealthIncrementHonoursDeadline(t *testing.T) {
	server := delayedMetricsServer(time.Millisecond)
	defer server.Close()

	url := server.URL + "/metrics"
	baselines := map[string]float64{url: 100} // already far above what the endpoint reports

	ctx, cancel := context.WithTimeout(context.Background(), 600*time.Millisecond)
	defer cancel()

	start := time.Now()
	err := waitForHealthIncrement(ctx, baselines, 30*time.Second)

	require.Error(t, err, "an endpoint that never advances must not satisfy the wait")
	require.Truef(t, isDeadlineError(err), "the failure must be a deadline error, got: %v", err)
	require.Lessf(t, time.Since(start), 10*time.Second,
		"the wait must end with its context, not with its own much longer timeout")
}

// TestPhaseDeadlineCapsWithinTestDeadline verifies the per-phase cap arithmetic:
// a phase never gets more than its own cap, and never more than what is left of
// the test.
func TestPhaseDeadlineCapsWithinTestDeadline(t *testing.T) {
	t.Run("cap is the binding limit", func(t *testing.T) {
		deadline := phaseDeadline(devtest.SerialT(t), 5*time.Second)
		require.WithinDuration(t, time.Now().Add(5*time.Second), deadline, time.Second)
	})

	t.Run("test deadline is the binding limit", func(t *testing.T) {
		// A test with no -timeout flag still reports a deadline; assert the cap is
		// never past it by more than the margin.
		if testDeadline, ok := t.Deadline(); ok {
			deadline := phaseDeadline(devtest.SerialT(t), 1000*time.Hour)
			require.Falsef(t, deadline.After(testDeadline),
				"a phase must never outlive its test: phase %s, test %s", deadline, testDeadline)
		}
	})
}

// TestDerivePhaseTimeoutNeverExceedsCap verifies that the derived phase context
// is bounded by BOTH the phase cap and the remaining test time.
func TestDerivePhaseTimeoutNeverExceedsCap(t *testing.T) {
	require.LessOrEqual(t, derivePhaseTimeout(10*time.Minute, 5*time.Second), 5*time.Second)
	require.LessOrEqual(t, derivePhaseTimeout(10*time.Second, time.Minute), 10*time.Second)
}
