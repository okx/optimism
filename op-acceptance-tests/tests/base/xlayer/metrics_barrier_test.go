package xlayer

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/stretchr/testify/require"
)

// TestScrapeHealthCheckCount_MetricAbsent verifies that scrapeHealthCheckCount returns an
// error (never silently 0) when op_conductor_healthchecks_count{success="true"} is not
// present in the metrics response.  This satisfies §11-P1: a scrape/parse failure must
// surface as an error, not as a zero baseline that would allow a false "no change" result.
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

// TestScrapeHealthCheckCount_HTTPError verifies that a non-200 response surfaces as an
// error rather than returning 0 silently.
func TestScrapeHealthCheckCount_HTTPError(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
	}))
	defer srv.Close()

	_, err := scrapeHealthCheckCount(context.Background(), srv.URL)
	require.Error(t, err, "a non-200 response must surface as an error")
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
