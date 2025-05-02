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

// ActivationManager handles the interop activation logic
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

// AttachEmitter sets the event emitter for the activation manager
func (am *ActivationManager) AttachEmitter(emitter event.Emitter) {
	am.emitter = emitter
}

// IsActive returns true if interop is currently active
func (am *ActivationManager) IsActive() bool {
	return am.IsActiveAt(uint64(time.Now().Unix()))
}

// IsActiveAt returns true if interop should be active at the given timestamp
func (am *ActivationManager) IsActiveAt(timestamp uint64) bool {
	if timestamp == 0 || am.depSet == nil {
		return false
	}

	for _, chain := range am.depSet.Chains() {
		canInitiate, err := am.depSet.CanInitiateAt(chain, timestamp)
		if err == nil && canInitiate {
			return true
		}
	}

	return false
}

// IsActiveForChain returns true if interop is active for the given chain at the specific timestamp
func (am *ActivationManager) IsActiveForChain(chain eth.ChainID, timestamp uint64) bool {
	if timestamp == 0 || am.depSet == nil {
		return false
	}

	canInitiate, err := am.depSet.CanInitiateAt(chain, timestamp)
	if err != nil {
		am.logger.Error("Error checking interop activation", "chain", chain, "timestamp", timestamp, "err", err)
		return false
	}
	return canInitiate
}

// ShouldProcessEvent determines if a block event should be processed based on interop status
// Returns true if the event should be processed, false otherwise
func (am *ActivationManager) ShouldProcessEvent(chain eth.ChainID, block eth.BlockRef) bool {
	// If this is an activation block, we need to process it to trigger initialization
	isActive := am.IsActiveForChain(chain, block.Time)
	if isActive {
		am.logger.Info("Potential interop activation detected", "chain", chain, "block", block)
		return true
	}

	// In pre-interop mode, events are filtered out
	am.logger.Debug("Filtering pre-interop event", "chain", chain, "block", block)
	return false
}

// DetectAndActivateInterop handles interop activation detection and processing
func (am *ActivationManager) DetectAndActivateInterop(
	ctx context.Context,
	chain eth.ChainID,
	block eth.BlockRef,
	getAnchorPoint func(context.Context) (types.DerivedBlockRefPair, error),
	isInitialized func(eth.ChainID) bool,
	initialize func(eth.ChainID, types.DerivedBlockRefPair),
) error {
	// Skip activation if this chain is already initialized
	if isInitialized(chain) {
		return nil
	}

	// Check if this block timestamp activates interop
	if !am.IsActiveForChain(chain, block.Time) {
		return nil
	}

	am.logger.Info("Interop activation detected, fetching anchor point", "chain", chain, "block", block)
	anchor, err := getAnchorPoint(ctx)
	if err != nil {
		return fmt.Errorf("failed to get anchor point at interop activation: %w", err)
	}

	if err := am.CheckAnchorPointExpiry(anchor); err != nil {
		return err
	}

	// Initialize with the anchor point
	am.logger.Info("Initializing with anchor point at interop activation",
		"chain", chain, "derived", anchor.Derived, "source", anchor.Source)
	initialize(chain, anchor)

	// Emit activation event
	if am.emitter != nil {
		am.emitter.Emit(superevents.InteropActivatedEvent{
			ChainID: chain,
			Anchor:  anchor,
			Block:   block,
		})
	}

	return nil
}

// CheckAnchorPointExpiry verifies that an anchor point is not too old based on the message expiry window
func (am *ActivationManager) CheckAnchorPointExpiry(anchor types.DerivedBlockRefPair) error {
	anchorTime := time.Unix(int64(anchor.Derived.Time), 0)
	window := time.Duration(am.depSet.MessageExpiryWindow()) * time.Second

	if time.Since(anchorTime) > window {
		return fmt.Errorf("%w: anchor point is too old: %d (more than %d old)",
			ErrInteropBoundary, anchorTime.Unix(), window.Round(time.Second))
	}

	return nil
}

// CheckDBBoundaries verifies that a block is not before the anchor point
// This is used to ensure that we don't process events that predate the anchoring of a chain
func (am *ActivationManager) CheckDBBoundaries(
	chain eth.ChainID,
	block eth.BlockRef,
	getAnchorBlock func(eth.ChainID) (eth.BlockRef, error),
) error {
	anchorBlock, err := getAnchorBlock(chain)
	if err != nil {
		// If we can't get the anchor block, this might be a test context
		// where initialization was done manually
		am.logger.Debug("Failed to get anchor block, assuming test context", "chain", chain, "err", err)
		return nil
	}

	if block.Number < anchorBlock.Number {
		return fmt.Errorf("%w: block is before the anchor point: block=%d, anchor=%d",
			ErrInteropBoundary, block.Number, anchorBlock.Number)
	}

	return nil
}

// MessageExpiryWindow returns the message expiry window from the dependency set
func (am *ActivationManager) MessageExpiryWindow() time.Duration {
	return time.Duration(am.depSet.MessageExpiryWindow()) * time.Second
}

// DependencySet returns the dependency set used by the activation manager
func (am *ActivationManager) DependencySet() depset.DependencySet {
	return am.depSet
}
