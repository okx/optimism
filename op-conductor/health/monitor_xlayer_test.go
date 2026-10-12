package health

import (
	"context"
	"testing"

	"github.com/ethereum/go-ethereum/log"
	"github.com/stretchr/testify/require"

	"github.com/ethereum-optimism/optimism/op-conductor/metrics"
	"github.com/ethereum-optimism/optimism/op-node/rollup"
	"github.com/ethereum-optimism/optimism/op-service/testlog"
)

// TestCheckNodePeerCount_ZeroMinDisabled verifies that MinPeerCount=0 skips the
// CL peer count check entirely — no p2p client is called, and nil is returned.
func TestCheckNodePeerCount_ZeroMinDisabled(t *testing.T) {
	logger := testlog.Logger(t, log.LevelDebug)
	hm := &SequencerHealthMonitor{
		log:        logger,
		metrics:    &metrics.NoopMetricsImpl{},
		minPeerCount: 0,
		rollupCfg:  &rollup.Config{BlockTime: 2},
		// p2p intentionally nil — a nil dereference would fail the test
	}
	err := hm.checkNodePeerCount(context.Background())
	require.NoError(t, err, "MinPeerCount=0 must skip the peer count check")
}
