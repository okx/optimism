package presets

import (
	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
	"github.com/ethereum-optimism/optimism/op-devstack/sysgo"
)

// XLayerDefaultL2GenesisHeight is the default non-zero L2 genesis block number
// used by the XLayer single-chain topology when a scenario does not override it.
// XLayer chains do not start at height 0, so the harness exercises a non-zero
// genesis by default while still allowing tests to override the value.
const XLayerDefaultL2GenesisHeight uint64 = 1

// XLayerSingleChain is the XLayer single-chain devnet target. It embeds the
// generic Minimal target (L1/L2 networks, sequencer EL/CL, batcher, faucets,
// funders) and adds nothing that would leak XLayer behavior into the upstream
// presets.
type XLayerSingleChain struct {
	*Minimal
}

// XLayerFlashblocks is the XLayer Flashblocks devnet target: a single producer
// sequencer with the op-rbuilder and rollup-boost front, from which relay RPC
// nodes consume the flashblocks stream. There is exactly one sequencer.
type XLayerFlashblocks struct {
	*SingleChainWithFlashblocks
}

const xLayerSingleChainPresetSupportedOptionKinds = optionKindDeployer |
	optionKindBatcher |
	optionKindProposer |
	optionKindOpReth |
	optionKindTimeTravel |
	optionKindAfterBuild

const xLayerFlashblocksPresetSupportedOptionKinds = optionKindDeployer |
	optionKindOPRBuilder |
	optionKindOpReth |
	optionKindAfterBuild

// DefaultXLayerChainOptions returns the XLayer chain parameters applied when a
// scenario does not supply its own: the devnet default chain ids (left unset so
// the generic devnet ids stand) and a non-zero L2 genesis height.
func DefaultXLayerChainOptions() sysgo.XLayerChainOptions {
	return sysgo.XLayerChainOptions{
		GenesisHeight: XLayerDefaultL2GenesisHeight,
	}
}

// NewXLayerSingleChain creates a fresh XLayer single-chain target for the current
// test using the default XLayer chain parameters. Additional preset options are
// applied after the runtime is built.
func NewXLayerSingleChain(t devtest.T, opts ...Option) *XLayerSingleChain {
	return NewXLayerSingleChainWithChainOptions(t, DefaultXLayerChainOptions(), opts...)
}

// NewXLayerSingleChainWithChainOptions creates an XLayer single-chain target with
// caller-supplied chain parameters (chain id, genesis height, genesis alloc,
// op-reth flags), so scenarios can cover chain-id, genesis-height, and
// genesis-alloc overrides.
func NewXLayerSingleChainWithChainOptions(t devtest.T, xl sysgo.XLayerChainOptions, opts ...Option) *XLayerSingleChain {
	presetCfg, presetOpts := collectSupportedPresetConfig(t, "NewXLayerSingleChain", opts, xLayerSingleChainPresetSupportedOptionKinds)
	out := xLayerSingleChainFromRuntime(t, sysgo.NewXLayerSingleChainRuntimeWithConfig(t, presetCfg, xl))
	presetOpts.applyPreset(out.Minimal)
	return out
}

// NewXLayerFlashblocks creates a fresh XLayer Flashblocks target for the current
// test using the default XLayer chain parameters.
func NewXLayerFlashblocks(t devtest.T, opts ...Option) *XLayerFlashblocks {
	return NewXLayerFlashblocksWithChainOptions(t, DefaultXLayerChainOptions(), opts...)
}

// NewXLayerFlashblocksWithChainOptions creates an XLayer Flashblocks target with
// caller-supplied chain parameters.
func NewXLayerFlashblocksWithChainOptions(t devtest.T, xl sysgo.XLayerChainOptions, opts ...Option) *XLayerFlashblocks {
	presetCfg, _ := collectSupportedPresetConfig(t, "NewXLayerFlashblocks", opts, xLayerFlashblocksPresetSupportedOptionKinds)
	runtime := sysgo.NewXLayerFlashblocksRuntimeWithConfig(t, presetCfg, xl)
	return xLayerFlashblocksFromRuntime(t, runtime)
}
