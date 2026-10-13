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

// TestScrapeHealthCheckCount_MetricAbsent verifies that scrapeHealthCheckCount returns an
// error (never silently 0) when op_conductor_healthchecks_count{success="true"} is not
// present in the metrics response.  A scrape or parse failure must surface as an error,
// not as a zero baseline that would allow a false "no change" result.
func TestScrapeHealthCheckCount_MetricAbsent(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Return a valid Prometheus page that has NO op_conductor_healthchecks_count line.
		w.Header().Set("Content-Type", "text/plain; version=0.0.4")
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte("# HELP some_other_metric A different metric\n# TYPE some_other_metric counter\nsome_other_metric 42\n"))
	}))
	defer srv.Close()

	count, err := scrapeHealthCheckCount(context.Background(), srv.URL)
	require.Error(t, err, "scrapeHealthCheckCount must return an error when metric is absent")
	require.Zero(t, count, "count must be 0 on error")
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
