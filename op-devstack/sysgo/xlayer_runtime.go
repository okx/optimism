package sysgo

import (
	"math/big"

	"github.com/ethereum/go-ethereum/accounts/abi/bind"
	"github.com/ethereum/go-ethereum/common/hexutil"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/crypto"
	"github.com/ethereum/go-ethereum/ethclient"

	"github.com/ethereum-optimism/optimism/op-chain-ops/devkeys"
	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
	bindings "github.com/ethereum-optimism/optimism/op-e2e/bindings"
	"github.com/ethereum-optimism/optimism/op-e2e/e2eutils/intentbuilder"
	"github.com/ethereum-optimism/optimism/op-e2e/e2eutils/wait"
	ps "github.com/ethereum-optimism/optimism/op-proposer/proposer"
	"github.com/ethereum-optimism/optimism/op-service/eth"
)

const (
	// XLayerDefaultL2ChainID matches the toolkit devnet chain ID without
	// changing the upstream devstack defaults used by other presets.
	XLayerDefaultL2ChainID uint64 = 195

	// XLayerDefaultL2GenesisHeight is the non-zero L2 genesis block height an
	// XLayer devnet starts from. It mirrors the production XLayer devnet, whose
	// L2 chain does not begin at block 0.
	XLayerDefaultL2GenesisHeight uint64 = 8_593_921

	// XLayerDefaultL1BlockTime accelerates L1 derivation in the local devnet.
	XLayerDefaultL1BlockTime uint64 = 2

	// XLayerDefaultL2BlockTime matches XLayer's one-second production cadence.
	XLayerDefaultL2BlockTime uint64 = 1
	XLayerFlashblockTimeMS   uint64 = 200

	// XLayer's fee-market settings mirror the toolkit devnet. Large EIP-1559
	// denominators intentionally make the base fee nearly static at the one-
	// second block cadence.
	XLayerEIP1559Denominator uint64 = 100_000_000
	XLayerEIP1559Elasticity  uint64 = 1
	XLayerMinBaseFee         uint64 = 99_999_999
	// XLayer deliberately disables DA-footprint gas accounting in the toolkit
	// devnet. Use the same value in both the L2 genesis intent and the live L1
	// SystemConfig update so the first L1 attributes transaction cannot change it.
	XLayerDAFootprintGasScalar     uint16 = 0
	XLayerSequencerWindowSize      uint64 = 7_200
	XLayerGenesisGasLimit          uint64 = 210_000_000
	XLayerGenesisBaseFeePerGas     uint64 = 100_401_605
	XLayerEcotoneBaseFeeScalar     uint32 = 0
	XLayerEcotoneBlobBaseFeeScalar uint32 = 0
)

const (
	// XLayerSequencerNodeName is the component name of the primary L2 sequencer
	// in XLayer runtimes and is used by target-aware execution-node options.
	XLayerSequencerNodeName = "sequencer"

	// XLayerValidatorNodeName is the runtime key of the single follower RPC node
	// in the XLayer single-chain topology. It follows the sequencer via L1 derivation.
	XLayerValidatorNodeName = "rpc1"
)

// WithXLayerL2GenesisHeight returns a DeployerOption that starts the L2 genesis
// at the given block number instead of 0, by setting the op-deployer global
// override consumed as DeployConfig.L2GenesisBlockNumber (the L2 genesis block
// Number and the rollup config genesis height both derive from it).
func WithXLayerL2GenesisHeight(height uint64) DeployerOption {
	return func(_ devtest.T, _ devkeys.Keys, builder intentbuilder.Builder) {
		builder.WithGlobalOverride("l2GenesisBlockNumber", hexutil.Uint64(height))
	}
}

// WithXLayerFeeMarketConfig configures the L2 genesis half of XLayer's toolkit
// fee market. configureXLayerSystemConfig applies the matching values through
// the typed L1 SystemConfig API before any L2 components start.
func WithXLayerFeeMarketConfig() DeployerOption {
	return func(_ devtest.T, _ devkeys.Keys, builder intentbuilder.Builder) {
		// The toolkit keeps the SystemConfig gas limit at the standard 60m but
		// overrides the genesis block header limit independently.
		builder.WithGlobalOverride("l2GenesisBlockGasLimit", hexutil.Uint64(XLayerGenesisGasLimit))
		builder.WithGlobalOverride("l2GenesisBlockBaseFeePerGas", hexutil.EncodeUint64(XLayerGenesisBaseFeePerGas))
		builder.WithGlobalOverride("minBaseFee", XLayerMinBaseFee)
		builder.WithGlobalOverride("gasPriceOracleBaseFeeScalar", XLayerEcotoneBaseFeeScalar)
		builder.WithGlobalOverride("gasPriceOracleBlobBaseFeeScalar", XLayerEcotoneBlobBaseFeeScalar)
		builder.WithGlobalOverride("eip1559DenominatorCanyon", XLayerEIP1559Denominator)

		for _, l2 := range builder.L2s() {
			l2.WithEIP1559Denominator(XLayerEIP1559Denominator)
			l2.WithEIP1559Elasticity(XLayerEIP1559Elasticity)
			l2.WithEIP1559DenominatorCanyon(XLayerEIP1559Denominator)
			l2.WithDAFootprintGasScalar(XLayerDAFootprintGasScalar)
		}
	}
}

// configureXLayerSystemConfig applies the L1 half of XLayer's fee-market
// configuration through SystemConfig's typed owner API.
//
// UPSTREAM(optimism): DeployOPChainInput currently initializes only the Ecotone
// fee scalars and gas limit. EIP-1559 parameters, min base fee, and DA footprint
// scalar are not part of the typed OPCM deployment input, despite being present
// in ChainIntent. Until that initialization path is extended, configure these
// values as owner transactions after L1 starts and before any L2 component does.
func configureXLayerSystemConfig(t devtest.T, keys devkeys.Keys, world singleChainRuntimeWorld, l1EL *L1Geth) {
	require := t.Require()
	client, err := ethclient.DialContext(t.Ctx(), l1EL.UserRPC())
	require.NoError(err, "dial L1 for XLayer SystemConfig initialization")
	t.Cleanup(client.Close)

	systemConfig, err := bindings.NewSystemConfig(world.L2Network.deployment.SystemConfigProxyAddr(), client)
	require.NoError(err, "bind XLayer SystemConfig")
	ownerKey, err := keys.Secret(devkeys.SystemConfigOwner.Key(world.L2Network.ChainID().ToBig()))
	require.NoError(err, "derive XLayer SystemConfig owner key")
	owner := crypto.PubkeyToAddress(ownerKey.PublicKey)
	nonce, err := client.PendingNonceAt(t.Ctx(), owner)
	require.NoError(err, "read XLayer SystemConfig owner nonce")

	type systemConfigCall struct {
		name string
		send func(*bind.TransactOpts) (*types.Transaction, error)
	}
	calls := []systemConfigCall{
		{name: "setGasConfigEcotone", send: func(opts *bind.TransactOpts) (*types.Transaction, error) {
			return systemConfig.SetGasConfigEcotone(opts, XLayerEcotoneBaseFeeScalar, XLayerEcotoneBlobBaseFeeScalar)
		}},
		{name: "setEIP1559Params", send: func(opts *bind.TransactOpts) (*types.Transaction, error) {
			return systemConfig.SetEIP1559Params(opts, uint32(XLayerEIP1559Denominator), uint32(XLayerEIP1559Elasticity))
		}},
		{name: "setMinBaseFee", send: func(opts *bind.TransactOpts) (*types.Transaction, error) {
			return systemConfig.SetMinBaseFee(opts, XLayerMinBaseFee)
		}},
		{name: "setDAFootprintGasScalar", send: func(opts *bind.TransactOpts) (*types.Transaction, error) {
			return systemConfig.SetDAFootprintGasScalar(opts, XLayerDAFootprintGasScalar)
		}},
	}

	txs := make([]*types.Transaction, 0, len(calls))
	for i, call := range calls {
		opts, err := bind.NewKeyedTransactorWithChainID(ownerKey, world.L1Network.ChainID().ToBig())
		require.NoError(err, "create signer for %s", call.name)
		opts.Context = t.Ctx()
		opts.Nonce = new(big.Int).SetUint64(nonce + uint64(i))
		opts.GasLimit = 200_000
		tx, err := call.send(opts)
		require.NoError(err, "submit SystemConfig.%s", call.name)
		txs = append(txs, tx)
	}

	// Waiting for the highest nonce first lets all four independent updates land
	// in one L1 block; checking every receipt afterwards still catches a revert in
	// an earlier transaction.
	_, err = wait.ForReceiptOK(t.Ctx(), client, txs[len(txs)-1].Hash())
	require.NoError(err, "wait for final XLayer SystemConfig update")
	for i, tx := range txs[:len(txs)-1] {
		_, err := wait.ForReceiptOK(t.Ctx(), client, tx.Hash())
		require.NoError(err, "SystemConfig.%s failed", calls[i].name)
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
	applyConfigPrefundedL2(t, keys, DefaultL1ID, eth.ChainIDFromUInt64(XLayerDefaultL2ChainID), wb.builder)
	applyConfigDeployerOptions(t, keys, wb.builder, cfg.DeployerOptions)
	wb.Build()

	t.Require().Len(wb.l2Chains, 1, "expected exactly one XLayer L2 chain")
	l2ID := wb.l2Chains[0]
	l1ID := eth.ChainIDFromUInt64(wb.output.AppliedIntent.L1ChainID)

	l1Net := &L1Network{
		name:      "l1",
		chainID:   l1ID,
		genesis:   wb.outL1Genesis,
		blockTime: XLayerDefaultL1BlockTime,
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
		l2EL := startL2ELForKey(t, world.L2Network, jwtPath, jwtSecret, XLayerSequencerNodeName, NewELNodeIdentity(0), elOpts...)
		l2CL := startL2CLForKey(t, keys, world.L1Network, world.L2Network, l1EL, l1CL, l2EL, jwtSecret, XLayerSequencerNodeName, XLayerSequencerNodeName, true, "", cfg.GlobalL2CLOptions)
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
	// Start the L2 genesis at the non-zero XLayer height with the production
	// one-second block cadence, and tell the proposer the same height so it does
	// not propose the genesis block itself.
	cfg.DeployerOptions = append([]DeployerOption{
		WithXLayerL2GenesisHeight(XLayerDefaultL2GenesisHeight),
		WithUniformL2BlockTimes(XLayerDefaultL2BlockTime),
		WithXLayerFeeMarketConfig(),
		WithSequencingWindow(XLayerSequencerWindowSize),
	}, cfg.DeployerOptions...)
	cfg.ProposerOptions = append(cfg.ProposerOptions, WithProposerGenesisHeight(XLayerDefaultL2GenesisHeight))
	producerOpts := xlayerFlashblocksProducerOption(xlayerFreePort(t))

	runtime := newSingleChainRuntimeWithConfig(t, cfg, singleChainRuntimeSpec{
		BuildWorld:      buildXLayerWorld,
		ConfigureL1:     configureXLayerSystemConfig,
		StartPrimary:    xlayerSequencerPrimary(producerOpts),
		StartBatcher:    true,
		StartProposer:   true,
		StartChallenger: false,
	})

	addXLayerFollowerNode(t, runtime, XLayerValidatorNodeName, cfg.OpRethOptions, cfg.GlobalL2CLOptions)
	return runtime
}
