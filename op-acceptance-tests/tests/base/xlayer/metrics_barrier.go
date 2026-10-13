package xlayer

import (
	"bufio"
	"context"
	"fmt"
	"math"
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

// healthCheckCounterName is the counter every conductor increments once per
// health-check tick. Only the success="true" series is read.
const healthCheckCounterName = "op_conductor_healthchecks_count"

// scrapeHealthCheckCount fetches op_conductor_healthchecks_count{success="true"}
// from the given Prometheus text endpoint.
//
// Three outcomes are distinguished deliberately:
//
//   - the scrape succeeds and the series is present -> its value;
//   - the scrape succeeds but the series is absent -> 0. The counter is created
//     by its first increment, so "no series yet" is a legitimate reading of a
//     conductor that has not completed a health check, not a failure;
//   - the scrape or the parse fails -> an error, never 0. Zeroing a failed read
//     would let an unreachable conductor satisfy an increase check.
//
// NaN and ±Inf are rejected as illegal counter values: comparing them against a
// baseline silently yields false, which would strand a caller in a timeout
// instead of surfacing the malformed endpoint.
//
// The request inherits ctx, so a caller that passes a deadline gets it applied
// to each individual HTTP request rather than to the whole polling loop only.
func scrapeHealthCheckCount(ctx context.Context, metricsURL string) (float64, error) {
	if err := ctx.Err(); err != nil {
		return 0, fmt.Errorf("scrape %s: %w", metricsURL, err)
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, metricsURL, nil)
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
		line := strings.TrimSpace(scanner.Text())
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		// The metric may carry extra labels (e.g. {success="true",error=""}), so it
		// is matched by metric name plus the success="true" label rather than by an
		// exact label-set prefix.
		if !strings.HasPrefix(line, healthCheckCounterName) {
			continue
		}
		if !strings.Contains(line, `success="true"`) {
			continue
		}
		parts := strings.Fields(line)
		if len(parts) != 2 {
			return 0, fmt.Errorf("unexpected metrics line format for %s: %q", healthCheckCounterName, line)
		}
		value, parseErr := strconv.ParseFloat(parts[1], 64)
		if parseErr != nil {
			return 0, fmt.Errorf("parse metric value %q: %w", parts[1], parseErr)
		}
		if math.IsNaN(value) || math.IsInf(value, 0) {
			return 0, fmt.Errorf("illegal counter value %q for %s", parts[1], healthCheckCounterName)
		}
		return value, nil
	}
	if scanErr := scanner.Err(); scanErr != nil {
		return 0, fmt.Errorf("scan metrics response: %w", scanErr)
	}
	// The endpoint answered but the series does not exist yet.
	return 0, nil
}

// awaitHealthSampleIncrease polls every endpoint until each one reports a value
// strictly greater than the baseline recorded for it, or timeout elapses. A
// successful scrape that has not moved yet is not an error — it is the condition
// being waited for. A failed scrape or an unparsable value is an immediate
// error, never a silent zero, because a silent zero would let a dead conductor
// satisfy the barrier.
func awaitHealthSampleIncrease(ctx context.Context, metricsURLs []string, baselines map[string]float64, timeout time.Duration) error {
	deadline := time.Now().Add(timeout)
	for {
		allAdvanced := true
		for _, url := range metricsURLs {
			count, err := scrapeHealthCheckCount(ctx, url)
			if err != nil {
				return err
			}
			if count <= baselines[url] {
				allAdvanced = false
				break
			}
		}
		if allAdvanced {
			return nil
		}
		if !time.Now().Before(deadline) {
			return fmt.Errorf("no new successful health-check sample within %s from %d conductor(s)", timeout, len(metricsURLs))
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(300 * time.Millisecond):
		}
	}
}

// healthBarrier records one health-check-count reading per conductor at the
// moment it is created, so a test can require a genuinely new sample after a
// trigger (a failover, a leadership transfer) rather than trusting a cached
// boolean that a stopped conductor can also carry.
type healthBarrier struct {
	metricsURLs []string
	baselines   map[string]float64
}

// newHealthBarrier resolves each conductor's metrics endpoint and records its
// current op_conductor_healthchecks_count{success="true"} value, taken at the
// moment of the call so every trigger gets a fresh baseline rather than a stale
// startup reading.
//
// The endpoint is normally reachable by the time a trigger runs; a scrape that
// genuinely fails is retried only briefly before failing the test, because a
// barrier built on an unreachable endpoint cannot measure anything. A conductor
// whose counter series does not exist yet reads 0, which is a valid baseline.
func newHealthBarrier(t devtest.T, ctx context.Context, conductors dsl.ConductorSet) *healthBarrier {
	return newHealthBarrierWithTimeout(t, ctx, conductors, 30*time.Second)
}

// newHealthBarrierWithTimeout is newHealthBarrier with an explicit bound on how
// long a failing scrape is retried before the test fails.
func newHealthBarrierWithTimeout(t devtest.T, ctx context.Context, conductors dsl.ConductorSet, scrapeTimeout time.Duration) *healthBarrier {
	urls := make([]string, 0, len(conductors))
	for _, c := range conductors {
		url := metricsURLForConductor(c)
		t.Require().NotEmptyf(url, "conductor %s has no metrics URL; the health barrier needs real metrics", c.String())
		urls = append(urls, url)
	}

	baselines := make(map[string]float64, len(urls))
	for _, url := range urls {
		deadline := time.Now().Add(scrapeTimeout)
		var count float64
		var err error
		for {
			count, err = scrapeHealthCheckCount(ctx, url)
			if err == nil {
				break
			}
			if !time.Now().Before(deadline) {
				break
			}
			select {
			case <-ctx.Done():
				t.Require().FailNowf("context expired while recording a health-check baseline from %s", url)
			case <-time.After(300 * time.Millisecond):
			}
		}
		t.Require().NoErrorf(err, "record baseline health-check count from %s", url)
		baselines[url] = count
	}
	return &healthBarrier{metricsURLs: urls, baselines: baselines}
}

// awaitNewSample requires at least one new successful health-check sample from
// each of the given conductors within timeout.
func (b *healthBarrier) awaitNewSample(t devtest.T, ctx context.Context, conductors dsl.ConductorSet, timeout time.Duration) {
	urls := make([]string, 0, len(conductors))
	for _, c := range conductors {
		url := metricsURLForConductor(c)
		t.Require().NotEmptyf(url, "conductor %s has no metrics URL; the health barrier needs real metrics", c.String())
		_, recorded := b.baselines[url]
		t.Require().Truef(recorded, "conductor %s has no recorded baseline; build the barrier from a set that includes it", c.String())
		urls = append(urls, url)
	}
	t.Require().NoErrorf(awaitHealthSampleIncrease(ctx, urls, b.baselines, timeout),
		"conductors must emit a new successful health-check sample within %s", timeout)
}

// awaitAllHealthy waits until every conductor in the set has emitted at least
// one new successful health-check sample since this call was made, using the
// Prometheus op_conductor_healthchecks_count{success="true"} counter as the
// barrier. Returns an error (via t.Require) if any scrape fails or the counter
// does not advance within timeout.
func awaitAllHealthy(t devtest.T, ctx context.Context, conductors dsl.ConductorSet, timeout time.Duration) {
	newHealthBarrier(t, ctx, conductors).awaitNewSample(t, ctx, conductors, timeout)
}
