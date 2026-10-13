package xlayer

import (
	"bufio"
	"context"
	"fmt"
	"net/http"
	"strconv"
	"strings"
	"time"

	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
	"github.com/ethereum-optimism/optimism/op-devstack/dsl"
)

// ConductorWithMetrics is an optional interface for conductor backends that
// expose a Prometheus metrics endpoint.
type ConductorWithMetrics interface {
	MetricsURL() string
}

func metricsURLForConductor(c *dsl.Conductor) string {
	if cw, ok := c.Escape().(ConductorWithMetrics); ok {
		return cw.MetricsURL()
	}
	return ""
}

// scrapeHealthCheckCount fetches op_conductor_healthchecks_count{success="true"}
// from the given Prometheus text endpoint. Returns an error (never 0) when the
// metric line is absent or the scrape fails.
func scrapeHealthCheckCount(ctx context.Context, metricsURL string) (float64, error) {
	req, err := http.NewRequestWithContext(ctx, "GET", metricsURL, nil)
	if err != nil {
		return 0, fmt.Errorf("build metrics request: %w", err)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return 0, fmt.Errorf("scrape %s: %w", metricsURL, err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return 0, fmt.Errorf("metrics server at %s returned %d", metricsURL, resp.StatusCode)
	}
	scanner := bufio.NewScanner(resp.Body)
	for scanner.Scan() {
		line := scanner.Text()
		if strings.HasPrefix(line, "#") {
			continue
		}
		// The metric may have additional labels (e.g. {success="true",error=""}) so
		// we match by metric name prefix AND the success="true" label rather than an
		// exact label-set prefix.
		if strings.HasPrefix(line, "op_conductor_healthchecks_count") && strings.Contains(line, `success="true"`) {
			parts := strings.Fields(line)
			if len(parts) < 2 {
				return 0, fmt.Errorf("unexpected metrics line format: %q", line)
			}
			v, parseErr := strconv.ParseFloat(parts[1], 64)
			if parseErr != nil {
				return 0, fmt.Errorf("parse metric value %q: %w", parts[1], parseErr)
			}
			return v, nil
		}
	}
	if scanErr := scanner.Err(); scanErr != nil {
		return 0, fmt.Errorf("scan metrics response: %w", scanErr)
	}
	return 0, fmt.Errorf("op_conductor_healthchecks_count with success=true not found at %s", metricsURL)
}

// awaitLeader polls the given ConductorSet until one conductor reports it is
// the Raft leader, then returns that conductor.
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

// awaitAllHealthy waits until every conductor in the set has emitted at least
// one new successful health-check sample since this call was made, using the
// Prometheus op_conductor_healthchecks_count{success="true"} counter as the
// barrier. Returns an error (via t.Require) if any scrape fails or the counter
// does not advance within timeout.
func awaitAllHealthy(t devtest.T, ctx context.Context, conductors dsl.ConductorSet, timeout time.Duration) {
	baselines := make(map[string]float64, len(conductors))
	for _, c := range conductors {
		url := metricsURLForConductor(c)
		t.Require().NotEmptyf(url, "conductor %s has no metrics URL; metrics barrier requires real metrics", c.String())
		var baselineCount float64
		var baselineErr error
		for i := 0; i < 10; i++ {
			baselineCount, baselineErr = scrapeHealthCheckCount(ctx, url)
			if baselineErr == nil {
				break
			}
			time.Sleep(300 * time.Millisecond)
		}
		t.Require().NoErrorf(baselineErr, "get baseline health-check count from conductor %s", c.String())
		baselines[c.String()] = baselineCount
	}

	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		allAdvanced := true
		for _, c := range conductors {
			url := metricsURLForConductor(c)
			count, scrapeErr := scrapeHealthCheckCount(ctx, url)
			if scrapeErr != nil {
				t.Require().NoErrorf(scrapeErr, "scrape health-check count from conductor %s", c.String())
			}
			if count <= baselines[c.String()] {
				allAdvanced = false
				break
			}
		}
		if allAdvanced {
			return
		}
		select {
		case <-ctx.Done():
			t.Require().FailNow("context expired while waiting for all conductors to emit new health-check samples")
		case <-time.After(500 * time.Millisecond):
		}
	}
	t.Require().FailNow("timed out waiting for all conductors to emit new health-check samples")
}
