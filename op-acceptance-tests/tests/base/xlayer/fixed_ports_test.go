package xlayer

import (
	"context"
	"fmt"
	"testing"
	"time"

	gethrpc "github.com/ethereum/go-ethereum/rpc"

	conductorRpc "github.com/ethereum-optimism/optimism/op-conductor/rpc"
	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
	"github.com/ethereum-optimism/optimism/op-devstack/presets"
	"github.com/ethereum-optimism/optimism/op-devstack/sysgo"
	"github.com/ethereum/go-ethereum/ethclient"
	"github.com/stretchr/testify/require"
)

// fixedPlan is the canonical fixed-port configuration used by all three tests
// in this file so each test is fully self-contained and idempotent.
var fixedPlan = sysgo.XLayerFixedPortPlan{
	L1HTTPPort:        8545,
	ELHTTPPorts:       [3]int{8123, 8124, 8125},
	ConductorRPCPorts: [3]int{8547, 8548, 8549},
}

// TestXLayerL1HTTPPortIsFixed verifies that the in-process L1 EL binds to the
// port specified in XLayerFixedPortPlan.L1HTTPPort by dialling it directly.
func TestXLayerL1HTTPPortIsFixed(gt *testing.T) {
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	_ = presets.NewXLayerWithConductors(t, presets.WithXLayerFixedPorts(fixedPlan))

	ctx, cancel := context.WithTimeout(t.Ctx(), 15*time.Second)
	defer cancel()

	endpoint := fmt.Sprintf("http://127.0.0.1:%d", fixedPlan.L1HTTPPort)
	cl, err := ethclient.DialContext(ctx, endpoint)
	require.NoError(t, err, "L1 EL must be reachable at the configured fixed HTTP port")
	defer cl.Close()
	_, err = cl.BlockNumber(ctx)
	require.NoError(t, err, "L1 EL at fixed port must respond to eth_blockNumber")
}

// TestXLayerELHTTPPortsAreFixed verifies that each of the three sequencer EL
// nodes binds to the port specified in XLayerFixedPortPlan.ELHTTPPorts by
// dialling each fixed port directly over HTTP.
func TestXLayerELHTTPPortsAreFixed(gt *testing.T) {
	t := devtest.SerialT(gt)
	sysgo.SkipOnKonaNode(t, "not supported")

	_ = presets.NewXLayerWithConductors(t, presets.WithXLayerFixedPorts(fixedPlan))

	ctx, cancel := context.WithTimeout(t.Ctx(), 15*time.Second)
	defer cancel()

	for i, port := range fixedPlan.ELHTTPPorts {
		endpoint := fmt.Sprintf("http://127.0.0.1:%d", port)
		cl, err := ethclient.DialContext(ctx, endpoint)
		require.NoErrorf(t, err, "L2 EL sequencer%d must be reachable at fixed HTTP port %d", i+1, port)
		defer cl.Close()
		_, err = cl.BlockNumber(ctx)
		require.NoErrorf(t, err, "L2 EL sequencer%d at fixed port %d must respond to eth_blockNumber", i+1, port)
	}
}

// TestXLayerConductorRPCPortsAreFixed verifies that each of the three in-process
// conductors binds to the port specified in XLayerFixedPortPlan.ConductorRPCPorts
// by dialling the expected address and calling Leader().
func TestXLayerConductorRPCPortsAreFixed(gt *testing.T) {
	t := devtest.SerialT(gt)
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
