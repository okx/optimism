package presets

import (
	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
	"github.com/ethereum-optimism/optimism/op-devstack/dsl"
	"github.com/ethereum-optimism/optimism/op-devstack/sysgo"
)

// XLayerFlashblocks is the XLayer flashblocks preset target: the single-producer
// flashblocks stack (sequencer + op-rbuilder + rollup-boost) plus two relay/RPC
// follower nodes rpc1 and rpc2. It never contains a second sequencer.
type XLayerFlashblocks struct {
	*SingleChainWithFlashblocks

	L2ELRPC1 *dsl.L2ELNode
	L2CLRPC1 *dsl.L2CLNode
	L2ELRPC2 *dsl.L2ELNode
	L2CLRPC2 *dsl.L2CLNode
}

// NewXLayerFlashblocks creates a fresh XLayer flashblocks target for the current
// test from the XLayer flashblocks runtime plus any additional preset options.
func NewXLayerFlashblocks(t devtest.T, opts ...Option) *XLayerFlashblocks {
	presetCfg, _ := collectSupportedPresetConfig(t, "NewXLayerFlashblocks", opts, singleChainWithFlashblocksPresetSupportedOptionKinds)
	runtime := sysgo.NewXLayerFlashblocksRuntimeWithConfig(t, presetCfg)
	base := singleChainWithFlashblocksFromRuntime(t, runtime)
	return xlayerFlashblocksFromRuntime(t, runtime, base)
}

func xlayerFlashblocksFromRuntime(t devtest.T, runtime *sysgo.SingleChainRuntime, base *SingleChainWithFlashblocks) *XLayerFlashblocks {
	l2ChainID := runtime.L2Network.ChainID()
	l2Net, ok := base.L2Chain.Escape().(*presetL2Network)
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
		SingleChainWithFlashblocks: base,
		L2ELRPC1:                   el1,
		L2CLRPC1:                   cl1,
		L2ELRPC2:                   el2,
		L2CLRPC2:                   cl2,
	}
}
