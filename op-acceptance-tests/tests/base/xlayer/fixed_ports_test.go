package xlayer

import (
	"context"
	"fmt"
	"net/url"
	"strconv"
	"testing"
	"time"

	gethrpc "github.com/ethereum/go-ethereum/rpc"

	conductorRpc "github.com/ethereum-optimism/optimism/op-conductor/rpc"
	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
	"github.com/ethereum-optimism/optimism/op-devstack/presets"
	"github.com/ethereum-optimism/optimism/op-devstack/sysgo"
	"github.com/stretchr/testify/require"
)

// fixedPlan is the canonical fixed-port configuration used by all three tests
// in this file so each test is fully self-contained and idempotent.
var fixedPlan = sysgo.XLayerFixedPortPlan{
	L1HTTPPort:        8545,
	ELHTTPPorts:       [3]int{8123, 8124, 8125},
	ConductorRPCPorts: [3]int{8547, 8548, 8549},
}

// portFromURL parses the port component from an HTTP URL string.
func portFromURL(t devtest.T, rawURL string) int {
	u, err := url.Parse(rawURL)
	require.NoError(t, err, "parse URL %q", rawURL)
	port, err := strconv.Atoi(u.Port())
	require.NoError(t, err, "extract port from URL %q", rawURL)
	return port
}

// TestXLayerL1HTTPPortIsFixed verifies that the in-process L1 EL binds to the
// port specified in XLayerFixedPortPlan.L1HTTPPort.
func TestXLayerL1HTTPPortIsFixed(gt *testing.T) {
	t := devtest.ParallelT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	sys := presets.NewXLayerWithConductors(t, presets.WithXLayerFixedPorts(fixedPlan))

	actualPort := portFromURL(t, sys.L1EL.Escape().UserRPC())
	require.Equal(t, fixedPlan.L1HTTPPort, actualPort,
		"L1 HTTP port must equal the configured fixed port")
}

// TestXLayerELHTTPPortsAreFixed verifies that each of the three sequencer EL
// nodes binds to the port specified in XLayerFixedPortPlan.ELHTTPPorts.
func TestXLayerELHTTPPortsAreFixed(gt *testing.T) {
	t := devtest.ParallelT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	sys := presets.NewXLayerWithConductors(t, presets.WithXLayerFixedPorts(fixedPlan))

	elRPCs := []string{
		sys.L2EL.Escape().UserRPC(),
		sys.L2ELSeq2.Escape().UserRPC(),
		sys.L2ELSeq3.Escape().UserRPC(),
	}
	for i, rpc := range elRPCs {
		actual := portFromURL(t, rpc)
		require.Equalf(t, fixedPlan.ELHTTPPorts[i], actual,
			"L2 EL sequencer%d HTTP port must equal the configured fixed port", i+1)
	}
}

// TestXLayerConductorRPCPortsAreFixed verifies that each of the three in-process
// conductors binds to the port specified in XLayerFixedPortPlan.ConductorRPCPorts
// by dialling the expected address and calling Leader().
func TestXLayerConductorRPCPortsAreFixed(gt *testing.T) {
	t := devtest.ParallelT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	_ = presets.NewXLayerWithConductors(t, presets.WithXLayerFixedPorts(fixedPlan))

	ctx, cancel := context.WithTimeout(t.Ctx(), 15*time.Second)
	defer cancel()

	for i, port := range fixedPlan.ConductorRPCPorts {
		endpoint := fmt.Sprintf("http://127.0.0.1:%d", port)
		rpcCl, err := gethrpc.DialContext(ctx, endpoint)
		require.NoErrorf(t, err, "conductor%d must be reachable at %s", i+1, endpoint)
		defer rpcCl.Close()

		api := conductorRpc.NewAPIClient(rpcCl)
		_, err = api.Leader(ctx)
		require.NoErrorf(t, err, "conductor%d at port %d must respond to RPC", i+1, port)
	}
}
