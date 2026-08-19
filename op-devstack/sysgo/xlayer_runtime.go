package sysgo

import (
	"github.com/ethereum/go-ethereum/common/hexutil"

	"github.com/ethereum-optimism/optimism/op-chain-ops/devkeys"
	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
	"github.com/ethereum-optimism/optimism/op-e2e/e2eutils/intentbuilder"
	ps "github.com/ethereum-optimism/optimism/op-proposer/proposer"
	"github.com/ethereum-optimism/optimism/op-service/eth"
)

// XLayerDefaultL2GenesisHeight is the non-zero L2 genesis block height an XLayer
// devnet starts from. It mirrors the production XLayer devnet, whose L2 chain
// does not begin at block 0.
const XLayerDefaultL2GenesisHeight uint64 = 8_593_921

// XLayerValidatorNodeName is the runtime key of the single follower RPC node in
// the XLayer single-chain topology. It follows the sequencer via L1 derivation.
const XLayerValidatorNodeName = "rpc1"

// WithXLayerL2GenesisHeight returns a DeployerOption that starts the L2 genesis
// at the given block number instead of 0, by setting the op-deployer global
// override consumed as DeployConfig.L2GenesisBlockNumber (the L2 genesis block
// Number and the rollup config genesis height both derive from it).
func WithXLayerL2GenesisHeight(height uint64) DeployerOption {
	return func(_ devtest.T, _ devkeys.Keys, builder intentbuilder.Builder) {
		builder.WithGlobalOverride("l2GenesisBlockNumber", hexutil.Uint64(height))
	}
}

// WithProposerGenesisHeight returns a ProposerOption that informs op-proposer of
// the L2 genesis block height, so it does not try to propose an output for a
// non-existent pre-genesis block on chains whose genesis height is not zero.
func WithProposerGenesisHeight(height uint64) ProposerOption {
	return func(_ ComponentTarget, cfg *ps.CLIConfig) {
		cfg.GenesisHeight = height
	}
}

// buildXLayerWorld builds the XLayer single-chain world from the local
// contracts-bedrock artifacts (resolved from cfg.LocalContractArtifactsPath,
// i.e. OPTIMISM_ROOT/.../forge-artifacts). It mirrors buildSingleChainWorld but
// exists so the XLayer topology can layer on its own deployer options (chain
// IDs, non-zero L2 genesis height). The local deploy scripts call vm.isContext,
// which the op-deployer Go script host now implements (see cheatcodes host), so
// genesis runs against the in-repo contracts rather than a downloaded bundle.
func buildXLayerWorld(t devtest.T, keys devkeys.Keys, cfg PresetConfig) singleChainRuntimeWorld {
	wb := &worldBuilder{
		p:       t,
		logger:  t.Logger(),
		require: t.Require(),
		keys:    keys,
		builder: intentbuilder.New(),
	}

	applyConfigLocalContractSources(t, keys, wb.builder, cfg.LocalContractArtifactsPath)
	applyConfigCommons(t, keys, DefaultL1ID, wb.builder)
	applyConfigPrefundedL2(t, keys, DefaultL1ID, DefaultL2AID, wb.builder)
	applyConfigDeployerOptions(t, keys, wb.builder, cfg.DeployerOptions)
	wb.Build()

	t.Require().Len(wb.l2Chains, 1, "expected exactly one XLayer L2 chain")
	l2ID := wb.l2Chains[0]
	l1ID := eth.ChainIDFromUInt64(wb.output.AppliedIntent.L1ChainID)

	l1Net := &L1Network{
		name:      "l1",
		chainID:   l1ID,
		genesis:   wb.outL1Genesis,
		blockTime: 6,
	}
	l2Net := &L2Network{
		name:       "l2a",
		chainID:    l2ID,
		l1ChainID:  l1ID,
		genesis:    wb.outL2Genesis[l2ID],
		rollupCfg:  wb.outL2RollupCfg[l2ID],
		deployment: wb.outL2Deployment[l2ID],
		opcmImpl:   wb.output.ImplementationsDeployment.OpcmV2Impl,
		mipsImpl:   wb.output.ImplementationsDeployment.MipsImpl,
		keys:       keys,
	}

	// Install the gasless predeploys (CREATE2 factory + the whitelist runtime at
	// the fixed address the execution client reads gasless rules from) so the
	// gasless scenarios can enable and exercise gasless transactions on the
	// running devnet.
	injectXLayerGaslessPredeploys(t, l2Net, cfg.LocalContractArtifactsPath)

	return singleChainRuntimeWorld{L1Network: l1Net, L2Network: l2Net}
}

// xlayerSequencerPrimary returns a StartPrimary that launches the XLayer L2
// execution client as the sequencer. The execution binary and any per-node CLI
// arguments are supplied through cfg.OpRethOptions (e.g. OpRethWithBinary set by
// the preset from XLAYER_RETH_BIN); seqExtraOpts carries topology-specific args
// such as enabling the built-in flashblocks builder on the producer.
func xlayerSequencerPrimary(seqExtraOpts ...OpRethOption) func(devtest.T, devkeys.Keys, singleChainRuntimeWorld, *L1Geth, *L1CLNode, string, [32]byte, PresetConfig) singleChainPrimaryRuntime {
	return func(t devtest.T, keys devkeys.Keys, world singleChainRuntimeWorld, l1EL *L1Geth, l1CL *L1CLNode, jwtPath string, jwtSecret [32]byte, cfg PresetConfig) singleChainPrimaryRuntime {
		elOpts := append(append([]OpRethOption{}, cfg.OpRethOptions...), seqExtraOpts...)
		l2EL := startSequencerEL(t, world.L2Network, jwtPath, jwtSecret, NewELNodeIdentity(0), elOpts...)
		l2CL := startSequencerCL(t, keys, world.L1Network, world.L2Network, l1EL, l1CL, l2EL, jwtSecret, cfg.GlobalL2CLOptions)
		return singleChainPrimaryRuntime{EL: l2EL, CL: l2CL}
	}
}

// addXLayerFollowerNode starts an XLayer follower (RPC/validator or flashblocks
// relay) node and registers it in the runtime. The follower tracks the sequencer
// through L1 derivation; elOpts carries the execution binary and any per-node CLI
// arguments (e.g. the flashblocks subscription URL for a relay).
func addXLayerFollowerNode(t devtest.T, runtime *SingleChainRuntime, name string, elOpts []OpRethOption, clOpts []L2CLOption) *SingleChainNodeRuntime {
	jwtPath := runtime.L2EL.JWTPath()
	jwtSecret := readJWTSecretFromPath(t, jwtPath)
	l2EL := startL2ELForKey(t, runtime.L2Network, jwtPath, jwtSecret, name, NewELNodeIdentity(0), elOpts...)
	l2CL := startL2CLForKey(t, runtime.Keys, runtime.L1Network, runtime.L2Network, runtime.L1EL, runtime.L1CL, l2EL, jwtSecret, name, name, false, "", clOpts)
	node := newSingleChainNodeRuntime(name, false, l2EL, l2CL)
	runtime.Nodes[name] = node
	return node
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
// Every L2 execution node runs the XLayer reth binary supplied via
// cfg.OpRethOptions. The validator node is started with discovery disabled and no
// P2P link to the sequencer, so it can only advance its safe head through L1
// derivation, matching the XLayer requirement that a validator follow the
// sequencer purely from L1 even with EL/CL P2P turned off.
func NewXLayerRuntimeWithConfig(t devtest.T, cfg PresetConfig) *SingleChainRuntime {
	// Start the L2 genesis at the non-zero XLayer height, and tell the proposer
	// the same height so it does not propose the genesis block itself.
	cfg.DeployerOptions = append([]DeployerOption{WithXLayerL2GenesisHeight(XLayerDefaultL2GenesisHeight)}, cfg.DeployerOptions...)
	cfg.ProposerOptions = append(cfg.ProposerOptions, WithProposerGenesisHeight(XLayerDefaultL2GenesisHeight))

	runtime := newSingleChainRuntimeWithConfig(t, cfg, singleChainRuntimeSpec{
		BuildWorld:      buildXLayerWorld,
		StartPrimary:    xlayerSequencerPrimary(),
		StartBatcher:    true,
		StartProposer:   true,
		StartChallenger: false,
	})

	addXLayerFollowerNode(t, runtime, XLayerValidatorNodeName, cfg.OpRethOptions, cfg.GlobalL2CLOptions)
	return runtime
}
