package xlayer

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strconv"
	"sync/atomic"
	"testing"
	"time"

	"github.com/stretchr/testify/require"
)

// TestScrapeHealthCheckCount_MetricAbsent verifies the zero-baseline rule: a
// scrape that SUCCEEDS but does not yet contain
// op_conductor_healthchecks_count{success="true"} reads 0 with no error. The
// series is created by its first increment, so its absence is a legitimate
// reading of a conductor that has not completed a health check yet.
func TestScrapeHealthCheckCount_MetricAbsentReadsZero(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// A valid Prometheus page with no op_conductor_healthchecks_count line.
		w.Header().Set("Content-Type", "text/plain; version=0.0.4")
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte("# HELP some_other_metric A different metric\n# TYPE some_other_metric counter\nsome_other_metric 42\n"))
	}))
	defer srv.Close()

	count, err := scrapeHealthCheckCount(context.Background(), srv.URL)
	require.NoError(t, err, "a successful scrape with no series yet is a zero reading, not a failure")
	require.Zero(t, count, "an absent series reads 0")
}

// TestScrapeHealthCheckCount_SuccessFalseOnly verifies that a page carrying only
// the success="false" series still reads 0 rather than picking up a failed check.
func TestScrapeHealthCheckCount_SuccessFalseOnly(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/plain; version=0.0.4")
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte(`op_conductor_healthchecks_count{success="false"} 9` + "\n"))
	}))
	defer srv.Close()

	count, err := scrapeHealthCheckCount(context.Background(), srv.URL)
	require.NoError(t, err)
	require.Zero(t, count, "only the success=true series may be read")
}

// TestScrapeHealthCheckCount_RejectsNearMissName verifies that a metric whose
// name merely starts with the health-check counter is not accepted: matching is
// exact, so a look-alike series cannot stand in for the real one.
func TestScrapeHealthCheckCount_RejectsNearMissName(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/plain; version=0.0.4")
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte(`op_conductor_healthchecks_count_total{success="true"} 7` + "\n"))
	}))
	defer srv.Close()

	count, err := scrapeHealthCheckCount(context.Background(), srv.URL)
	require.NoError(t, err)
	require.Zero(t, count, "a differently named metric must not satisfy the health-check series")
}

// TestScrapeHealthCheckCount_RejectsNearMissLabel verifies that a label whose
// value merely starts with "true" is not accepted: the label is matched as a
// complete name="value" pair.
func TestScrapeHealthCheckCount_RejectsNearMissLabel(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/plain; version=0.0.4")
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte(`op_conductor_healthchecks_count{success="true-ish"} 7` + "\n"))
	}))
	defer srv.Close()

	count, err := scrapeHealthCheckCount(context.Background(), srv.URL)
	require.NoError(t, err)
	require.Zero(t, count, "a look-alike label value must not satisfy the health-check series")
}

// TestScrapeHealthCheckCount_ExtraLabelsStillMatch verifies that a genuine
// sample carrying additional labels is still recognised, since the match is on
// the presence of the exact success="true" pair rather than on the whole label
// set.
func TestScrapeHealthCheckCount_ExtraLabelsStillMatch(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/plain; version=0.0.4")
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte(`op_conductor_healthchecks_count{node="a",success="true",error=""} 11` + "\n"))
	}))
	defer srv.Close()

	count, err := scrapeHealthCheckCount(context.Background(), srv.URL)
	require.NoError(t, err)
	require.InDelta(t, 11.0, count, 0.001)
}

// TestXLayerHealthDeadlineEnforcement verifies that a hung metrics endpoint is
// bounded by the local request deadline rather than by the caller's much longer
// context: with a 300s parent context and a 3s local timeout, the request must
// return well before the parent would have expired.
func TestXLayerHealthDeadlineEnforcement(t *testing.T) {
	blocked := make(chan struct{})
	defer close(blocked)
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		select {
		case <-r.Context().Done():
		case <-blocked:
		}
	}))
	defer srv.Close()

	parent, parentCancel := context.WithTimeout(context.Background(), 300*time.Second)
	defer parentCancel()
	reqCtx, reqCancel := context.WithTimeout(parent, 3*time.Second)
	defer reqCancel()

	start := time.Now()
	_, err := scrapeHealthCheckCount(reqCtx, srv.URL)
	elapsed := time.Since(start)

	require.Error(t, err, "a hung endpoint must fail once the local deadline expires")
	require.Less(t, elapsed, 60*time.Second,
		"the local 3s timeout must bound the request, not the 300s parent context (took %s)", elapsed)
}

// TestScrapeHealthCheckCount_RejectsNaN verifies that a NaN counter is rejected
// as an illegal value. Comparing NaN against a baseline is always false, so
// accepting it would strand a caller in a timeout instead of surfacing the
// malformed endpoint.
func TestScrapeHealthCheckCount_RejectsNaN(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/plain; version=0.0.4")
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte(`op_conductor_healthchecks_count{success="true"} NaN` + "\n"))
	}))
	defer srv.Close()

	_, err := scrapeHealthCheckCount(context.Background(), srv.URL)
	require.Error(t, err, "NaN must be rejected as an illegal counter value")
	require.Contains(t, err.Error(), "illegal counter value")
}

// TestScrapeHealthCheckCount_RejectsInf verifies the same for +Inf.
func TestScrapeHealthCheckCount_RejectsInf(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/plain; version=0.0.4")
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte(`op_conductor_healthchecks_count{success="true"} +Inf` + "\n"))
	}))
	defer srv.Close()

	_, err := scrapeHealthCheckCount(context.Background(), srv.URL)
	require.Error(t, err, "Inf must be rejected as an illegal counter value")
}

// TestScrapeHealthCheckCount_RespectsContextDeadline verifies that an expired
// request context surfaces as an error rather than reading the endpoint anyway:
// the remaining deadline is applied to each individual HTTP request.
func TestScrapeHealthCheckCount_RespectsContextDeadline(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		time.Sleep(2 * time.Second)
		w.WriteHeader(http.StatusOK)
	}))
	defer srv.Close()

	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()

	start := time.Now()
	_, err := scrapeHealthCheckCount(ctx, srv.URL)
	require.Error(t, err, "a request past its deadline must fail")
	require.Less(t, time.Since(start), time.Second, "the deadline must bound the request, not the whole test")
}

// TestScrapeHealthCheckCount_MetricPresent verifies that scrapeHealthCheckCount returns the
// correct counter value when op_conductor_healthchecks_count{success="true"} is present.
func TestScrapeHealthCheckCount_MetricPresent(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/plain; version=0.0.4")
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte(
			"# HELP op_conductor_healthchecks_count Total health checks\n" +
				"# TYPE op_conductor_healthchecks_count counter\n" +
				`op_conductor_healthchecks_count{success="true"} 7` + "\n",
		))
	}))
	defer srv.Close()

	count, err := scrapeHealthCheckCount(context.Background(), srv.URL)
	require.NoError(t, err)
	require.InDelta(t, 7.0, count, 0.001, "scrapeHealthCheckCount must return the metric value")
}

// TestScrapeHealthCheckCount_TwoLabelForm verifies that the two-label form
// op_conductor_healthchecks_count{success="true",error=""} is correctly matched —
// the implementation uses prefix+contains rather than an exact-prefix match.
func TestScrapeHealthCheckCount_TwoLabelForm(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/plain; version=0.0.4")
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte(
			`op_conductor_healthchecks_count{success="true",error=""} 12` + "\n",
		))
	}))
	defer srv.Close()

	count, err := scrapeHealthCheckCount(context.Background(), srv.URL)
	require.NoError(t, err)
	require.InDelta(t, 12.0, count, 0.001, "two-label form must be matched correctly")
}

// newFakeConductorMetrics serves a single op_conductor_healthchecks_count{success="true"}
// counter whose value is read from the supplied getter on every scrape.
func newFakeConductorMetrics(get func() float64) *httptest.Server {
	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/plain; version=0.0.4")
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte(
			"# HELP op_conductor_healthchecks_count Total health checks\n" +
				"# TYPE op_conductor_healthchecks_count counter\n" +
				`op_conductor_healthchecks_count{success="true"} ` + strconv.FormatFloat(get(), 'f', -1, 64) + "\n",
		))
	}))
}

// TestAwaitHealthSampleIncrease_NewSamplePasses verifies that the per-trigger barrier
// returns as soon as every conductor has emitted a strictly newer successful sample.
func TestAwaitHealthSampleIncrease_NewSamplePasses(t *testing.T) {
	var current atomic.Int64
	current.Store(10)
	srv := newFakeConductorMetrics(func() float64 { return float64(current.Load()) })
	defer srv.Close()

	baselines := map[string]float64{srv.URL: 10}
	// A later scrape returns a newer value, so the barrier must pass.
	go func() {
		time.Sleep(400 * time.Millisecond)
		current.Store(11)
	}()

	err := awaitHealthSampleIncrease(context.Background(), []string{srv.URL}, baselines, 10*time.Second)
	require.NoError(t, err, "a newer successful sample must satisfy the barrier")
}

// TestAwaitHealthSampleIncrease_StalledSampleTimesOut verifies that a conductor which
// keeps reporting the same count never satisfies the barrier: a cached value is not
// a new sample.
func TestAwaitHealthSampleIncrease_StalledSampleTimesOut(t *testing.T) {
	var current atomic.Int64
	current.Store(4)
	srv := newFakeConductorMetrics(func() float64 { return float64(current.Load()) })
	defer srv.Close()

	baselines := map[string]float64{srv.URL: 4}

	err := awaitHealthSampleIncrease(context.Background(), []string{srv.URL}, baselines, 1500*time.Millisecond)
	require.Error(t, err, "a counter that never advances must time out rather than pass")
	require.Contains(t, err.Error(), "no new successful health-check sample")
}

// TestAwaitHealthSampleIncrease_ScrapeFailureIsError verifies that a metrics endpoint
// which cannot be scraped surfaces as an error instead of stalling until the timeout
// or silently counting as an advance.
func TestAwaitHealthSampleIncrease_ScrapeFailureIsError(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusServiceUnavailable)
	}))
	defer srv.Close()

	baselines := map[string]float64{srv.URL: 0}

	start := time.Now()
	err := awaitHealthSampleIncrease(context.Background(), []string{srv.URL}, baselines, 10*time.Second)
	require.Error(t, err, "a failing scrape must surface as an error")
	require.Less(t, time.Since(start), 5*time.Second, "a scrape failure must fail fast, not wait for the timeout")
}

// TestScrapeHealthCheckCount_HTTPError verifies that a non-OK HTTP response surfaces as an
// error rather than returning 0 silently.
func TestScrapeHealthCheckCount_HTTPError(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
	}))
	defer srv.Close()

	_, err := scrapeHealthCheckCount(context.Background(), srv.URL)
	require.Error(t, err, "a non-OK HTTP response must surface as an error")
}

// TestScrapeHealthCheckCount_MalformedValue verifies that a malformed metric value
// (non-numeric) surfaces as a parse error rather than returning 0 silently.
func TestScrapeHealthCheckCount_MalformedValue(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/plain; version=0.0.4")
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte(
			`op_conductor_healthchecks_count{success="true"} notanumber` + "\n",
		))
	}))
	defer srv.Close()

	_, err := scrapeHealthCheckCount(context.Background(), srv.URL)
	require.Error(t, err, "a malformed metric value must surface as a parse error")
}
