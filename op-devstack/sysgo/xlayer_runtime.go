package sysgo

import (
	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
	ps "github.com/ethereum-optimism/optimism/op-proposer/proposer"
)

// XLayerDefaultL2GenesisHeight is the non-zero L2 genesis block height an XLayer
// devnet starts from. It mirrors the production XLayer devnet, whose L2 chain
// does not begin at block 0.
const XLayerDefaultL2GenesisHeight uint64 = 8_593_921

// XLayerValidatorNodeName is the runtime key of the single follower RPC node in
// the XLayer single-chain topology. It follows the sequencer via L1 derivation.
const XLayerValidatorNodeName = "rpc1"

// WithProposerGenesisHeight returns a ProposerOption that informs op-proposer of
// the L2 genesis block height, so it does not try to propose an output for a
// non-existent pre-genesis block on chains whose genesis height is not zero.
func WithProposerGenesisHeight(height uint64) ProposerOption {
	return func(_ ComponentTarget, cfg *ps.CLIConfig) {
		cfg.GenesisHeight = height
	}
}

// NewXLayerRuntime builds the default XLayer single-chain devnet topology.
func NewXLayerRuntime(t devtest.T) *SingleChainRuntime {
	return NewXLayerRuntimeWithConfig(t, PresetConfig{})
}

// NewXLayerRuntimeWithConfig assembles the XLayer single-chain topology: an L1
// execution + beacon pair, an L2 sequencer, one L2 RPC/validator node (keyed
// XLayerValidatorNodeName), a batcher and a proposer. It deliberately omits the
// challenger, op-conductor and any fault-proof dependency.
//
// The validator node is started with discovery disabled and no P2P link to the
// sequencer, so it can only advance its safe head through L1 derivation. This
// matches the XLayer requirement that a validator follow the sequencer purely
// from L1 even with EL/CL P2P turned off.
func NewXLayerRuntimeWithConfig(t devtest.T, cfg PresetConfig) *SingleChainRuntime {
	// Tell the proposer the non-zero genesis height so it skips proposing the
	// genesis block itself.
	cfg.ProposerOptions = append(cfg.ProposerOptions, WithProposerGenesisHeight(XLayerDefaultL2GenesisHeight))

	// TODO: making the L2 genesis *block number* actually equal
	// XLayerDefaultL2GenesisHeight requires an intentbuilder/op-deployer option
	// to offset the L2 genesis height; no such option exists in
	// op-devstack/sysgo/deployer.go today, so the world is currently built with
	// the standard genesis (height 0) and only the proposer is told the intended
	// height. Wiring the genesis offset is follow-up work once the deployer
	// exposes it.
	runtime := newSingleChainRuntimeWithConfig(t, cfg, singleChainRuntimeSpec{
		BuildWorld:      newDefaultSingleChainWorld,
		StartPrimary:    startDefaultSingleChainPrimary,
		StartBatcher:    true,
		StartProposer:   true,
		StartChallenger: false,
	})

	addSingleChainOpNode(t, runtime, XLayerValidatorNodeName, false, "", cfg.GlobalL2CLOptions...)
	return runtime
}
