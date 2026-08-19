package sysgo

import (
	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
)

// Relay node keys for the XLayer flashblocks topology. There is exactly one
// producer sequencer; rpc1 and rpc2 are relay/RPC followers. No second
// sequencer (seq2) is started.
const (
	XLayerFlashblocksRelay1NodeName = "rpc1"
	XLayerFlashblocksRelay2NodeName = "rpc2"
)

// NewXLayerFlashblocksRuntime builds the default XLayer flashblocks devnet
// topology.
func NewXLayerFlashblocksRuntime(t devtest.T) *SingleChainRuntime {
	return NewXLayerFlashblocksRuntimeWithConfig(t, PresetConfig{})
}

// NewXLayerFlashblocksRuntimeWithConfig assembles the XLayer flashblocks
// topology: a single producer sequencer (with its op-rbuilder + rollup-boost
// flashblocks support) plus a batcher, and two follower RPC nodes rpc1 and rpc2.
// It never starts a second sequencer.
func NewXLayerFlashblocksRuntimeWithConfig(t devtest.T, cfg PresetConfig) *SingleChainRuntime {
	runtime := newSingleChainRuntimeWithConfig(t, cfg, singleChainRuntimeSpec{
		BuildWorld:      newDefaultSingleChainWorld,
		StartPrimary:    startFlashblocksSingleChainPrimary,
		StartBatcher:    true,
		StartProposer:   false,
		StartChallenger: false,
	})

	// Two relay RPC followers track the single producer sequencer via L1
	// derivation. Keeping them as followers (not sequencers) guarantees the
	// topology has no seq2.
	addSingleChainOpNode(t, runtime, XLayerFlashblocksRelay1NodeName, false, "", cfg.GlobalL2CLOptions...)
	addSingleChainOpNode(t, runtime, XLayerFlashblocksRelay2NodeName, false, "", cfg.GlobalL2CLOptions...)

	// TODO: give rpc1 and rpc2 their own isolated flashblocks WebSocket
	// endpoints that relay the producer's flashblock stream. The producer side
	// (op-rbuilder + rollup-boost) already exposes a flashblocks WS endpoint;
	// per-relay endpoints require additional per-node op-rbuilder/rollup-boost
	// wiring that has no ready-made helper in this package yet, so rpc1/rpc2
	// currently serve standard RPC only.
	return runtime
}
