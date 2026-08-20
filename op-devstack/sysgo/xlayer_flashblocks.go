package sysgo

import (
	"fmt"
	"net"
	"strconv"

	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
)

// Relay node keys for the XLayer flashblocks topology. There is exactly one
// producer sequencer; rpc1 and rpc2 are relay/RPC followers. No second
// sequencer (seq2) is started.
const (
	XLayerFlashblocksRelay1NodeName = "rpc1"
	XLayerFlashblocksRelay2NodeName = "rpc2"

	xlayerFlashblocksListenAddr = "127.0.0.1"
)

// xlayerFreePort reserves an ephemeral local TCP port for the producer's
// flashblocks WebSocket server. The listener is closed immediately so the reth
// node can bind it; using an OS-assigned port keeps parallel devnets isolated.
func xlayerFreePort(t devtest.T) int {
	ln, err := net.Listen("tcp", xlayerFlashblocksListenAddr+":0")
	t.Require().NoError(err, "allocate a free flashblocks websocket port")
	port := ln.Addr().(*net.TCPAddr).Port
	t.Require().NoError(ln.Close(), "release the reserved flashblocks port")
	return port
}

// xlayerFlashblocksProducerOption enables the sequencer's built-in flashblocks
// producer on an isolated local endpoint.
func xlayerFlashblocksProducerOption(port int) OpRethOption {
	return OpRethWithExtraArgs(
		"--xlayer.sequencer-mode",
		"--flashblocks.enabled",
		"--flashblocks.addr", xlayerFlashblocksListenAddr,
		"--flashblocks.port", strconv.Itoa(port),
		"--flashblocks.block-time", strconv.FormatUint(XLayerFlashblockTimeMS, 10),
		"--rollup.chain-block-time", strconv.FormatUint(XLayerDefaultL2BlockTime*1_000, 10),
	)
}

// NewXLayerFlashblocksRuntime builds the default XLayer flashblocks devnet
// topology.
func NewXLayerFlashblocksRuntime(t devtest.T) *SingleChainRuntime {
	return NewXLayerFlashblocksRuntimeWithConfig(t, PresetConfig{})
}

// NewXLayerFlashblocksRuntimeWithConfig assembles the XLayer flashblocks topology
// using the XLayer reth binary's built-in flashblocks builder — no op-rbuilder or
// rollup-boost. The single producer sequencer runs with the flashblocks builder
// enabled and publishes flashblocks over a WebSocket on an isolated local port;
// two relay followers (rpc1, rpc2) subscribe to that stream to serve pending
// state. It never starts a second sequencer.
func NewXLayerFlashblocksRuntimeWithConfig(t devtest.T, cfg PresetConfig) *SingleChainRuntime {
	// Match the single-chain topology's non-zero genesis height and one-second L2
	// block cadence.
	cfg.DeployerOptions = append([]DeployerOption{
		WithXLayerL2GenesisHeight(XLayerDefaultL2GenesisHeight),
		WithUniformL2BlockTimes(XLayerDefaultL2BlockTime),
		WithXLayerFeeMarketConfig(),
		WithSequencingWindow(XLayerSequencerWindowSize),
	}, cfg.DeployerOptions...)

	producerPort := xlayerFreePort(t)
	producerOpts := xlayerFlashblocksProducerOption(producerPort)

	runtime := newSingleChainRuntimeWithConfig(t, cfg, singleChainRuntimeSpec{
		BuildWorld:      buildXLayerWorld,
		ConfigureL1:     configureXLayerSystemConfig,
		StartPrimary:    xlayerSequencerPrimary(producerOpts),
		StartBatcher:    true,
		StartProposer:   false,
		StartChallenger: false,
	})

	// Each relay subscribes to the producer's flashblock stream and re-publishes
	// it on its OWN isolated WebSocket endpoint. The relay's re-publisher binds
	// --flashblocks.port, which defaults to a fixed port; without a distinct port
	// per relay the second relay fails with "address already in use". Allocating a
	// free port per relay also gives each of producer/rpc1/rpc2 an isolated
	// flashblocks WS endpoint.
	subscribeURL := fmt.Sprintf("ws://%s:%d", xlayerFlashblocksListenAddr, producerPort)
	relayOpts := func(relayPort int) []OpRethOption {
		return append(append([]OpRethOption{}, cfg.OpRethOptions...),
			OpRethWithExtraArgs(
				"--flashblocks-url", subscribeURL,
				"--flashblocks.addr", xlayerFlashblocksListenAddr,
				"--flashblocks.port", strconv.Itoa(relayPort),
			))
	}

	addXLayerFollowerNode(t, runtime, XLayerFlashblocksRelay1NodeName, relayOpts(xlayerFreePort(t)), cfg.GlobalL2CLOptions)
	addXLayerFollowerNode(t, runtime, XLayerFlashblocksRelay2NodeName, relayOpts(xlayerFreePort(t)), cfg.GlobalL2CLOptions)
	return runtime
}
