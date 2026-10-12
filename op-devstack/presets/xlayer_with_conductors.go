package presets

import (
	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
	"github.com/ethereum-optimism/optimism/op-devstack/dsl"
	"github.com/ethereum-optimism/optimism/op-devstack/sysgo"
	"github.com/ethereum-optimism/optimism/op-devstack/stack"
	"github.com/ethereum-optimism/optimism/op-service/eth"
)

// XLayerWithConductors is the XLayer high-availability sequencer preset: the full
// XLayer 3-sequencer + 3-conductor + HA-batcher topology exposed as a typed DSL
// target. ConductorSets is keyed by the L2 chain ID and holds all three conductors
// so tests can drive leader election and failover directly.
type XLayerWithConductors struct {
	*Minimal

	// Seq2 and Seq3 are the additional sequencer EL+CL pairs.
	L2ELSeq2 *dsl.L2ELNode
	L2CLSeq2 *dsl.L2CLNode
	L2ELSeq3 *dsl.L2ELNode
	L2CLSeq3 *dsl.L2CLNode

	ConductorSets map[eth.ChainID]dsl.ConductorSet
}

// NewXLayerWithConductors creates a fresh XLayerWithConductors preset target.
func NewXLayerWithConductors(t devtest.T, opts ...Option) *XLayerWithConductors {
	presetCfg, presetOpts := collectSupportedPresetConfig(t, "NewXLayerWithConductors", opts, xlayerWithConductorsPresetSupportedOptionKinds)
	out := xlayerWithConductorsFromRuntime(t, sysgo.NewXLayerWithConductorsRuntimeWithConfig(t, presetCfg))
	presetOpts.applyPreset(out)
	return out
}

func xlayerWithConductorsFromRuntime(t devtest.T, runtime *sysgo.SingleChainRuntime) *XLayerWithConductors {
	minimal := minimalFromRuntime(t, runtime)
	l2ChainID := runtime.L2Network.ChainID()

	t.Require().NotNil(runtime.Conductors, "missing conductor support")

	// Retrieve the secondary and tertiary sequencer node runtimes.
	seq2 := runtime.Nodes["sequencer2"]
	t.Require().NotNil(seq2, "missing sequencer2 node")
	seq3 := runtime.Nodes["sequencer3"]
	t.Require().NotNil(seq3, "missing sequencer3 node")

	// Build DSL frontends for seq2.
	l2ELSeq2 := newL2ELFrontend(
		t,
		"sequencer2",
		l2ChainID,
		seq2.EL.UserRPC(),
		seq2.EL.EngineRPC(),
		seq2.EL.JWTPath(),
		runtime.L2Network.RollupConfig(),
		seq2.EL,
	)
	l2CLSeq2 := newL2CLFrontend(t, "sequencer2", l2ChainID, seq2.CL.UserRPC(), seq2.CL)
	l2CLSeq2.attachEL(l2ELSeq2)

	// Build DSL frontends for seq3.
	l2ELSeq3 := newL2ELFrontend(
		t,
		"sequencer3",
		l2ChainID,
		seq3.EL.UserRPC(),
		seq3.EL.EngineRPC(),
		seq3.EL.JWTPath(),
		runtime.L2Network.RollupConfig(),
		seq3.EL,
	)
	l2CLSeq3 := newL2CLFrontend(t, "sequencer3", l2ChainID, seq3.CL.UserRPC(), seq3.CL)
	l2CLSeq3.attachEL(l2ELSeq3)

	// Register the additional nodes with the preset L2 network so the DSL can
	// route to them.
	l2Net, ok := minimal.L2Chain.Escape().(*presetL2Network)
	t.Require().True(ok, "expected preset L2 network")
	l2Net.AddL2ELNode(l2ELSeq2)
	l2Net.AddL2CLNode(l2CLSeq2)
	l2Net.AddL2ELNode(l2ELSeq3)
	l2Net.AddL2CLNode(l2CLSeq3)

	// Build conductor frontends and register them.
	c1Name := "conductor1"
	c2Name := "conductor2"
	c3Name := "conductor3"
	c1 := newConductorFrontend(t, c1Name, l2ChainID, runtime.Conductors[c1Name].HTTPEndpoint())
	c2 := newConductorFrontend(t, c2Name, l2ChainID, runtime.Conductors[c2Name].HTTPEndpoint())
	c3 := newConductorFrontend(t, c3Name, l2ChainID, runtime.Conductors[c3Name].HTTPEndpoint())
	l2Net.AddConductor(c1)
	l2Net.AddConductor(c2)
	l2Net.AddConductor(c3)

	conductors := []stack.Conductor{c1, c2, c3}
	return &XLayerWithConductors{
		Minimal:  minimal,
		L2ELSeq2: dsl.NewL2ELNode(l2ELSeq2),
		L2CLSeq2: dsl.NewL2CLNode(l2CLSeq2),
		L2ELSeq3: dsl.NewL2ELNode(l2ELSeq3),
		L2CLSeq3: dsl.NewL2CLNode(l2CLSeq3),
		ConductorSets: map[eth.ChainID]dsl.ConductorSet{
			l2ChainID: dsl.NewConductorSet(conductors),
		},
	}
}
