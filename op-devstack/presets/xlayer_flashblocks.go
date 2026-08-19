package presets

import (
	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
	"github.com/ethereum-optimism/optimism/op-devstack/dsl"
	"github.com/ethereum-optimism/optimism/op-devstack/sysgo"
)

// XLayerFlashblocks is the XLayer flashblocks preset target. The sequencer (the
// embedded Minimal.L2EL) runs the XLayer reth binary with its built-in
// flashblocks builder enabled; rpc1 and rpc2 are relay/RPC followers that
// subscribe to the sequencer's flashblocks stream. There is no second sequencer,
// and no op-rbuilder or rollup-boost process is involved.
type XLayerFlashblocks struct {
	*Minimal

	L2ELRPC1 *dsl.L2ELNode
	L2CLRPC1 *dsl.L2CLNode
	L2ELRPC2 *dsl.L2ELNode
	L2CLRPC2 *dsl.L2CLNode
}

// NewXLayerFlashblocks creates a fresh XLayer flashblocks target for the current
// test from the XLayer flashblocks runtime plus any additional preset options.
func NewXLayerFlashblocks(t devtest.T, opts ...Option) *XLayerFlashblocks {
	presetCfg, presetOpts := collectSupportedPresetConfig(t, "NewXLayerFlashblocks", opts, xlayerPresetSupportedOptionKinds)
	out := xlayerFlashblocksFromRuntime(t, sysgo.NewXLayerFlashblocksRuntimeWithConfig(t, presetCfg))
	presetOpts.applyPreset(out)
	return out
}

func xlayerFlashblocksFromRuntime(t devtest.T, runtime *sysgo.SingleChainRuntime) *XLayerFlashblocks {
	minimal := minimalFromRuntime(t, runtime)
	l2ChainID := runtime.L2Network.ChainID()
	l2Net, ok := minimal.L2Chain.Escape().(*presetL2Network)
	t.Require().True(ok, "expected preset L2 network")

	newRelayFrontends := func(name string) (*dsl.L2ELNode, *dsl.L2CLNode) {
		node := runtime.Nodes[name]
		t.Require().NotNil(node, "missing flashblocks relay node %q", name)
		el := newL2ELFrontend(
			t,
			name,
			l2ChainID,
			node.EL.UserRPC(),
			node.EL.EngineRPC(),
			node.EL.JWTPath(),
			runtime.L2Network.RollupConfig(),
			node.EL,
		)
		cl := newL2CLFrontend(t, name, l2ChainID, node.CL.UserRPC(), node.CL)
		cl.attachEL(el)
		l2Net.AddL2ELNode(el)
		l2Net.AddL2CLNode(cl)
		return dsl.NewL2ELNode(el), dsl.NewL2CLNode(cl)
	}

	el1, cl1 := newRelayFrontends(sysgo.XLayerFlashblocksRelay1NodeName)
	el2, cl2 := newRelayFrontends(sysgo.XLayerFlashblocksRelay2NodeName)

	return &XLayerFlashblocks{
		Minimal:  minimal,
		L2ELRPC1: el1,
		L2CLRPC1: cl1,
		L2ELRPC2: el2,
		L2CLRPC2: cl2,
	}
}
