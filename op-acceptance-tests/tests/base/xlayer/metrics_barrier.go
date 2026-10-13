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
		value, matched, lineErr := parseHealthCheckSample(line)
		if lineErr != nil {
			return 0, lineErr
		}
		if matched {
			return value, nil
		}
	}
	if scanErr := scanner.Err(); scanErr != nil {
		return 0, fmt.Errorf("scan metrics response: %w", scanErr)
	}
	// The endpoint answered but the series does not exist yet.
	return 0, nil
}

// parseHealthCheckSample parses one Prometheus exposition line and reports
// whether it is the health-check counter carrying the success="true" label.
//
// The metric name is matched EXACTLY and the label is matched as a complete
// name="value" pair, never by prefix or substring: with substring matching a
// different metric whose name merely starts with this one, or a label such as
// success="true-ish", would be silently accepted as a health sample. The
// returned error covers a malformed line, a non-numeric or illegal value; a
// line that simply is not the series we want reports matched=false and no error.
func parseHealthCheckSample(line string) (value float64, matched bool, err error) {
	var name string
	labelSet := ""
	valueField := ""
	if open := strings.IndexByte(line, '{'); open >= 0 {
		closeIdx := strings.IndexByte(line[open:], '}')
		if closeIdx < 0 {
			return 0, false, fmt.Errorf("unterminated label set in metrics line %q", line)
		}
		closeIdx += open
		name = line[:open]
		labelSet = line[open+1 : closeIdx]
		valueField = strings.TrimSpace(line[closeIdx+1:])
	} else {
		fields := strings.Fields(line)
		if len(fields) != 2 {
			return 0, false, fmt.Errorf("unexpected metrics line format: %q", line)
		}
		name = fields[0]
		valueField = fields[1]
	}
	if name != healthCheckCounterName {
		return 0, false, nil
	}
	wantLabel := `success="true"`
	hasLabel := false
	for _, label := range strings.Split(labelSet, ",") {
		if strings.TrimSpace(label) == wantLabel {
			hasLabel = true
			break
		}
	}
	if !hasLabel {
		return 0, false, nil
	}
	value, parseErr := strconv.ParseFloat(valueField, 64)
	if parseErr != nil {
		return 0, false, fmt.Errorf("parse metric value %q: %w", valueField, parseErr)
	}
	if math.IsNaN(value) || math.IsInf(value, 0) || value < 0 {
		return 0, false, fmt.Errorf("illegal counter value %q for %s", valueField, healthCheckCounterName)
	}
	return value, true, nil
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

// baselineHealthMetrics records one successful health-check reading per
// conductor in the set, taken at the moment of the call.
//
// Calling this immediately before a trigger is what makes the later increment a
// measurement of THAT trigger. A baseline carried over from an earlier phase
// would already be satisfied by samples the earlier phase produced, so a
// conductor whose monitor had since died would still pass.
func baselineHealthMetrics(t devtest.T, ctx context.Context, conductors dsl.ConductorSet, scrapeTimeout time.Duration) map[string]float64 {
	return newHealthBarrierWithTimeout(t, ctx, conductors, scrapeTimeout).baselines
}

// waitForHealthIncrement waits until every endpoint in baselines reports a value
// strictly above the reading recorded for it. A scrape that fails is an
// immediate error, never a silent zero: a zero would let an unreachable
// conductor satisfy the increase.
func waitForHealthIncrement(ctx context.Context, baselines map[string]float64, timeout time.Duration) error {
	urls := make([]string, 0, len(baselines))
	for url := range baselines {
		urls = append(urls, url)
	}
	if len(urls) == 0 {
		return fmt.Errorf("no conductors to wait on: the baseline is empty")
	}
	return awaitHealthSampleIncrease(ctx, urls, baselines, timeout)
}

// memberStopHelper stops the sequencer EL of the named member and returns an
// error — never a bare boolean. It also hands back a health baseline taken
// immediately BEFORE the stop, so the caller can require that the surviving
// conductors emitted a genuinely new sample after the fault rather than
// satisfying the check with a reading from before it.
//
// This helper is the fault-injection surface for the HA lifecycle tests: every
// one of them needs "a node really went down, and here is the evidence that the
// rest of the cluster noticed". A helper that answered with a boolean would let
// a caller that ignored the answer treat a refused stop as a stopped node.
func memberStopHelper(ctx context.Context, t devtest.T, h *haCluster, name string, survivorTimeout time.Duration) (*healthBarrier, error) {
	survivors := make(dsl.ConductorSet, 0, len(h.conductors)-1)
	for _, c := range h.conductors {
		if c.String() != name {
			survivors = append(survivors, c)
		}
	}
	if len(survivors) == 0 {
		return nil, fmt.Errorf("member %q is the only conductor in the set", name)
	}

	// Baseline first, stop second: the other order would race the fault.
	barrier := newHealthBarrierWithTimeout(t, ctx, survivors, survivorTimeout)
	if err := h.stopMember(ctx, name); err != nil {
		return nil, fmt.Errorf("stop member %q: %w", name, err)
	}
	return barrier, nil
}
