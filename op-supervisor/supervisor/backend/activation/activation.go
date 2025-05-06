package activation

import (
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/ethereum/go-ethereum/log"

	"github.com/ethereum-optimism/optimism/op-node/rollup/event"
	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/ethereum-optimism/optimism/op-supervisor/supervisor/backend/depset"
	"github.com/ethereum-optimism/optimism/op-supervisor/supervisor/backend/superevents"
	"github.com/ethereum-optimism/optimism/op-supervisor/supervisor/types"
)

var ErrInteropBoundary = errors.New("interop boundary error")

type AnchorProvider interface {
	GetAnchorPoint(ctx context.Context, chainID eth.ChainID) (types.DerivedBlockRefPair, error)
}

type ChainInitializer interface {
	IsInitialized(chainID eth.ChainID) bool
	InitializeWithAnchor(chainID eth.ChainID, anchor types.DerivedBlockRefPair)
}

type AnchorBlockProvider interface {
	GetAnchorBlock(chainID eth.ChainID) (eth.BlockRef, error)
}

type ActivationManager struct {
	depSet  depset.DependencySet
	logger  log.Logger
	emitter event.Emitter
}

func NewActivationManager(depSet depset.DependencySet, logger log.Logger) *ActivationManager {
	return &ActivationManager{
		depSet: depSet,
		logger: logger,
	}
}

func (am *ActivationManager) AttachEmitter(emitter event.Emitter) {
	am.emitter = emitter
}

func (am *ActivationManager) IsActive() bool {
	return am.IsActiveAt(uint64(time.Now().Unix()))
}

func (am *ActivationManager) IsActiveAt(timestamp uint64) bool {
	if timestamp == 0 || am.depSet == nil {
		return false
	}

	for _, chain := range am.depSet.Chains() {
		if am.IsActiveForChain(chain, timestamp) {
			return true
		}
	}

	return false
}

func (am *ActivationManager) IsActiveForChain(chain eth.ChainID, timestamp uint64) bool {
	if timestamp == 0 || am.depSet == nil {
		return false
	}

	canInitiate, err := am.depSet.CanInitiateAt(chain, timestamp)
	if err != nil {
		am.logger.Debug("Error checking interop activation", "chain", chain, "timestamp", timestamp, "err", err)
		return false
	}
	return canInitiate
}

func (am *ActivationManager) ShouldProcessEvent(chain eth.ChainID, block eth.BlockRef) bool {
	isActive := am.IsActiveForChain(chain, block.Time)
	if isActive {
		am.logger.Info("Potential interop activation detected", "chain", chain, "block", block)
		return true
	}

	am.logger.Debug("Filtering pre-interop event", "chain", chain, "block", block)
	return false
}

func (am *ActivationManager) DetectAndActivateInterop(
	ctx context.Context,
	chain eth.ChainID,
	block eth.BlockRef,
	getAnchorPoint func(ctx context.Context) (types.DerivedBlockRefPair, error),
	isInitialized func(id eth.ChainID) bool,
	initialize func(id eth.ChainID, anchor types.DerivedBlockRefPair),
) error {
	if isInitialized(chain) {
		return nil
	}

	if !am.IsActiveForChain(chain, block.Time) {
		return nil
	}

	am.logger.Info("Interop activation detected, fetching anchor point", "chain", chain, "block", block)
	anchor, err := getAnchorPoint(ctx)
	if err != nil {
		return fmt.Errorf("failed to get anchor point at interop activation: %w", err)
	}

	am.logger.Info("Initializing with anchor point at interop activation",
		"chain", chain, "derived", anchor.Derived, "source", anchor.Source)
	initialize(chain, anchor)

	if am.emitter != nil {
		am.emitter.Emit(superevents.InteropActivatedEvent{
			ChainID: chain,
			Anchor:  anchor,
			Block:   block,
		})
	}

	return nil
}

func (am *ActivationManager) CheckAnchorPointExpiry(anchor types.DerivedBlockRefPair) error {
	anchorTime := time.Unix(int64(anchor.Derived.Time), 0)
	window := time.Duration(am.depSet.MessageExpiryWindow()) * time.Second

	if time.Since(anchorTime) > window {
		return fmt.Errorf("%w: anchor point is too old: %d (more than %d old)",
			ErrInteropBoundary, anchorTime.Unix(), window.Round(time.Second))
	}

	return nil
}

func (am *ActivationManager) CheckDBBoundaries(
	chain eth.ChainID,
	block eth.BlockRef,
	getAnchorBlock func(eth.ChainID) (eth.BlockRef, error),
) error {
	anchorBlock, err := getAnchorBlock(chain)
	if err != nil {
		return fmt.Errorf("failed to get anchor block: %w", err)
	}

	if block.Number < anchorBlock.Number {
		return fmt.Errorf("%w: block is before the anchor point: block=%d, anchor=%d",
			ErrInteropBoundary, block.Number, anchorBlock.Number)
	}

	return nil
}

func (am *ActivationManager) MessageExpiryWindow() time.Duration {
	return time.Duration(am.depSet.MessageExpiryWindow()) * time.Second
}

func (am *ActivationManager) DependencySet() depset.DependencySet {
	return am.depSet
}

func (am *ActivationManager) DetectAndActivateInteropWithInterfaces(
	ctx context.Context,
	chain eth.ChainID,
	block eth.BlockRef,
	anchorProvider AnchorProvider,
	initializer ChainInitializer,
) error {
	return am.DetectAndActivateInterop(
		ctx,
		chain,
		block,
		func(ctx context.Context) (types.DerivedBlockRefPair, error) {
			return anchorProvider.GetAnchorPoint(ctx, chain)
		},
		func(id eth.ChainID) bool {
			return initializer.IsInitialized(id)
		},
		func(id eth.ChainID, anchor types.DerivedBlockRefPair) {
			initializer.InitializeWithAnchor(id, anchor)
		},
	)
}

func (am *ActivationManager) CheckDBBoundariesWithProvider(
	chain eth.ChainID,
	block eth.BlockRef,
	anchorBlockProvider AnchorBlockProvider,
) error {
	return am.CheckDBBoundaries(
		chain,
		block,
		func(id eth.ChainID) (eth.BlockRef, error) {
			return anchorBlockProvider.GetAnchorBlock(id)
		},
	)
}
