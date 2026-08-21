package sysgo

import (
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

// xlayerFlashblocksProducerOption enables the sequencer's built-in flashblocks
// producer on an OS-assigned local endpoint.
func xlayerFlashblocksProducerOption() OpRethOption {
	return OpRethWithExtraArgs(
		"--xlayer.sequencer-mode",
		"--flashblocks.enabled",
		"--flashblocks.addr", xlayerFlashblocksListenAddr,
		"--flashblocks.port", "0",
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

	producerOpts := xlayerFlashblocksProducerOption()

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
	// per relay the second relay fails with "address already in use". Port 0 lets
	// the OS bind an isolated endpoint without a bind-close-rebind race.
	producer, ok := runtime.L2EL.(*OpReth)
	t.Require().True(ok, "XLayer flashblocks producer must be an op-reth node")
	subscribeURL := producer.FlashblocksWS()
	t.Require().NotEmpty(subscribeURL, "XLayer flashblocks producer URL was not discovered")
	relayOpts := func() []OpRethOption {
		return append(append([]OpRethOption{}, cfg.OpRethOptions...),
			OpRethWithExtraArgs(
				"--flashblocks-url", subscribeURL,
				"--flashblocks.addr", xlayerFlashblocksListenAddr,
				"--flashblocks.port", "0",
				"--xlayer.flashblocks-subscription",
			))
	}

	// A relay needs the producer's canonical unsafe payloads as the base state for
	// the next flashblock sequence. Connect both CL and EL peers: op-node's follow
	// source reports head references but does not transfer the payload bodies that
	// a fresh op-reth database needs to advance from genesis.
	relay1 := addXLayerFollowerNode(t, runtime, XLayerFlashblocksRelay1NodeName, relayOpts(), cfg.GlobalL2CLOptions)
	connectSingleChainNodes(t, runtime.L2EL, runtime.L2CL, relay1)
	relay2 := addXLayerFollowerNode(t, runtime, XLayerFlashblocksRelay2NodeName, relayOpts(), cfg.GlobalL2CLOptions)
	connectSingleChainNodes(t, runtime.L2EL, runtime.L2CL, relay2)
	runtime.P2PEnabled = true
	return runtime
}
