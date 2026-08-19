package presets

import (
	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
	"github.com/ethereum-optimism/optimism/op-devstack/dsl"
	"github.com/ethereum-optimism/optimism/op-devstack/sysgo"
)

// xlayerPresetSupportedOptionKinds extends the minimal set with the op-reth
// option kind so callers can supply the XLayer execution binary (XLAYER_RETH_BIN)
// and per-node execution args via WithOpRethOption.
const xlayerPresetSupportedOptionKinds = minimalPresetSupportedOptionKinds | optionKindOpReth

// XLayer is the XLayer single-chain preset target: the minimal sequencer stack
// (L1 EL+CL, L2 sequencer, batcher, proposer) plus one follower RPC/validator
// node that tracks the sequencer through L1 derivation.
type XLayer struct {
	Minimal

	L2ELRPC1 *dsl.L2ELNode
	L2CLRPC1 *dsl.L2CLNode
}

// NewXLayer creates a fresh XLayer single-chain target for the current test from
// the XLayer runtime plus any additional preset options.
func NewXLayer(t devtest.T, opts ...Option) *XLayer {
	presetCfg, presetOpts := collectSupportedPresetConfig(t, "NewXLayer", opts, xlayerPresetSupportedOptionKinds)
	out := xlayerFromRuntime(t, sysgo.NewXLayerRuntimeWithConfig(t, presetCfg))
	presetOpts.applyPreset(out)
	return out
}

func xlayerFromRuntime(t devtest.T, runtime *sysgo.SingleChainRuntime) *XLayer {
	minimal := minimalFromRuntime(t, runtime)
	l2ChainID := runtime.L2Network.ChainID()

	rpc1 := runtime.Nodes[sysgo.XLayerValidatorNodeName]
	t.Require().NotNil(rpc1, "missing XLayer validator node %q", sysgo.XLayerValidatorNodeName)

	l2ELRPC1 := newL2ELFrontend(
		t,
		sysgo.XLayerValidatorNodeName,
		l2ChainID,
		rpc1.EL.UserRPC(),
		rpc1.EL.EngineRPC(),
		rpc1.EL.JWTPath(),
		runtime.L2Network.RollupConfig(),
		rpc1.EL,
	)
	l2CLRPC1 := newL2CLFrontend(
		t,
		sysgo.XLayerValidatorNodeName,
		l2ChainID,
		rpc1.CL.UserRPC(),
		rpc1.CL,
	)
	l2CLRPC1.attachEL(l2ELRPC1)

	l2Net, ok := minimal.L2Chain.Escape().(*presetL2Network)
	t.Require().True(ok, "expected preset L2 network")
	l2Net.AddL2ELNode(l2ELRPC1)
	l2Net.AddL2CLNode(l2CLRPC1)

	return &XLayer{
		Minimal:  *minimal,
		L2ELRPC1: dsl.NewL2ELNode(l2ELRPC1),
		L2CLRPC1: dsl.NewL2CLNode(l2CLRPC1),
	}
}
