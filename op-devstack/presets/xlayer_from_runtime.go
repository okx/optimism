package presets

import (
	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
	"github.com/ethereum-optimism/optimism/op-devstack/sysgo"
)

// xLayerSingleChainFromRuntime builds the RPC-client frontends over a started
// XLayer single-chain runtime by reusing the generic minimal wiring, then wraps
// them in the XLayer target. The runtime already ran the XLayer world, so the
// frontends point at XLayer chain configuration.
func xLayerSingleChainFromRuntime(t devtest.T, runtime *sysgo.SingleChainRuntime) *XLayerSingleChain {
	return &XLayerSingleChain{
		Minimal: minimalFromRuntime(t, runtime),
	}
}

// xLayerFlashblocksFromRuntime builds the frontends over a started XLayer
// Flashblocks runtime by reusing the generic flashblocks wiring (sequencer EL/CL,
// op-rbuilder, rollup-boost, test sequencer), then wraps them in the XLayer
// target.
func xLayerFlashblocksFromRuntime(t devtest.T, runtime *sysgo.SingleChainRuntime) *XLayerFlashblocks {
	return &XLayerFlashblocks{
		SingleChainWithFlashblocks: singleChainWithFlashblocksFromRuntime(t, runtime),
	}
}
