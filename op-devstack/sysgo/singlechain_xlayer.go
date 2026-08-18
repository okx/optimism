package sysgo

import (
	"math/big"

	"github.com/ethereum-optimism/optimism/op-chain-ops/devkeys"
	"github.com/ethereum-optimism/optimism/op-devstack/devtest"
	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/ethereum/go-ethereum/core/types"
)

// XLayerChainOptions carries the chain parameters that distinguish an XLayer L2
// from the generic single-chain devnet: a dedicated L2 chain id, a non-zero
// genesis height, extra genesis allocations, and any XLayer-specific execution
// client flags. The zero value keeps every generic default untouched, so a
// non-XLayer preset built from the same helpers is never affected by XLayer
// behavior.
type XLayerChainOptions struct {
	// L2ChainID overrides the L2 chain id when non-zero.
	L2ChainID uint64
	// GenesisHeight sets a non-zero L2 genesis block number when non-zero, so
	// scenarios can exercise a chain that does not start at height 0.
	GenesisHeight uint64
	// ExtraGenesisAlloc merges additional prefunded or predeployed accounts into
	// the L2 genesis without disturbing the accounts the deployer already sets.
	ExtraGenesisAlloc types.GenesisAlloc
	// OpRethExtraArgs are appended to the XLayer op-reth execution client flags
	// (for example gasless-enabling or flashblocks-publisher switches).
	OpRethExtraArgs []string
}

// applyToWorld imprints the XLayer chain parameters onto an already-built
// single-chain world. It only mutates the XLayer L2 network; the shared L1 and
// the generic defaults are left as-is so a non-XLayer topology built from the
// same underlying helpers keeps its original chain configuration and node count.
func (o XLayerChainOptions) applyToWorld(world singleChainRuntimeWorld) {
	l2 := world.L2Network
	if l2 == nil || l2.genesis == nil {
		return
	}
	if o.L2ChainID != 0 {
		if l2.genesis.Config != nil {
			l2.genesis.Config.ChainID = new(big.Int).SetUint64(o.L2ChainID)
		}
		l2.chainID = eth.ChainIDFromUInt64(o.L2ChainID)
	}
	if o.GenesisHeight != 0 {
		l2.genesis.Number = o.GenesisHeight
	}
	if len(o.ExtraGenesisAlloc) > 0 {
		if l2.genesis.Alloc == nil {
			l2.genesis.Alloc = make(types.GenesisAlloc, len(o.ExtraGenesisAlloc))
		}
		for addr, acc := range o.ExtraGenesisAlloc {
			l2.genesis.Alloc[addr] = acc
		}
	}
}

// presetConfigWithXLayerArgs returns a copy of cfg with the XLayer op-reth flags
// appended, so the execution client receives them without mutating the caller's
// original configuration slice.
func (o XLayerChainOptions) presetConfigWithXLayerArgs(cfg PresetConfig) PresetConfig {
	if len(o.OpRethExtraArgs) == 0 {
		return cfg
	}
	merged := append([]OpRethOption{}, cfg.OpRethOptions...)
	merged = append(merged, OpRethWithExtraArgs(o.OpRethExtraArgs...))
	cfg.OpRethOptions = merged
	return cfg
}

// buildWorld wraps the generic default world builder and applies the XLayer
// chain parameters, giving the XLayer topologies a non-zero genesis and XLayer
// chain configuration without changing newDefaultSingleChainWorld itself.
func (o XLayerChainOptions) buildWorld(t devtest.T, keys devkeys.Keys, cfg PresetConfig) singleChainRuntimeWorld {
	world := newDefaultSingleChainWorld(t, keys, cfg)
	o.applyToWorld(world)
	return world
}

// NewXLayerSingleChainRuntimeWithConfig starts the XLayer single-chain devnet:
// L1 execution + beacon, an L2 sequencer, one L2 RPC/validator, op-node, batcher
// and proposer. It reuses the generic single-chain driver but supplies the
// XLayer world so the L2 runs XLayer chain configuration with a non-zero genesis.
// No challenger, op-conductor, or fault-proof dependencies are started.
func NewXLayerSingleChainRuntimeWithConfig(t devtest.T, cfg PresetConfig, xl XLayerChainOptions) *SingleChainRuntime {
	cfg = xl.presetConfigWithXLayerArgs(cfg)
	return newSingleChainRuntimeWithConfig(t, cfg, singleChainRuntimeSpec{
		BuildWorld:      xl.buildWorld,
		StartPrimary:    startDefaultSingleChainPrimary,
		StartBatcher:    true,
		StartProposer:   true,
		StartChallenger: false,
	})
}

// NewXLayerFlashblocksRuntimeWithConfig starts the XLayer Flashblocks devnet: a
// single producer sequencer fronted by rollup-boost plus the op-rbuilder, from
// which relay RPC nodes consume the flashblocks stream. It mirrors the generic
// flashblocks primary and deliberately never starts a second sequencer, so the
// only block producer is the single sequencer.
func NewXLayerFlashblocksRuntimeWithConfig(t devtest.T, cfg PresetConfig, xl XLayerChainOptions) *SingleChainRuntime {
	cfg = xl.presetConfigWithXLayerArgs(cfg)
	return newSingleChainRuntimeWithConfig(t, cfg, singleChainRuntimeSpec{
		BuildWorld:      xl.buildWorld,
		StartPrimary:    startFlashblocksSingleChainPrimary,
		StartBatcher:    false,
		StartProposer:   false,
		StartChallenger: false,
	})
}
