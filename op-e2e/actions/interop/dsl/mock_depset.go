package dsl

import (
	"fmt"
	"time"

	"github.com/ethereum-optimism/optimism/op-chain-ops/interopgen"
	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/ethereum-optimism/optimism/op-supervisor/supervisor/backend/depset"
	"github.com/ethereum-optimism/optimism/op-supervisor/supervisor/types"
)

// SetInteropOffsetForAllL2s returns a setup option that sets the L1 block number 
// at which interop will activate for all L2 chains. It uses the RecipeToDepSet function
// to set the activation time based on the L1 block timestamp at that offset.
func SetInteropOffsetForAllL2s(offset uint64) setupOption {
	return func(recipe *interopgen.InteropDevRecipe) {
		// Store the offset globally for use in RecipeToDepSet
		testInteropActivationOffset = offset
	}
}

// Global variable to store the activation offset for testing
var testInteropActivationOffset uint64

// SetFutureInteropActivation returns a setup option that sets the interop
// activation to be in the future, requiring L1 blocks to be mined to reach it.
func SetFutureInteropActivation() setupOption {
	// Set activation to occur after 3 L1 blocks (12 seconds per block)
	return SetInteropOffsetForAllL2s(3)
}

// MockDependencySet is a test implementation of the dependency set interface
type MockDependencySet struct {
	chainConfigs   map[eth.ChainID]*depset.StaticConfigDependency
	messageExpiry  uint64
	activationTime uint64
}

// NewMockDependencySet creates a new mock dependency set with activation at the specified time
func NewMockDependencySet(activationTime uint64, messageExpiry uint64) *MockDependencySet {
	return &MockDependencySet{
		chainConfigs:   make(map[eth.ChainID]*depset.StaticConfigDependency),
		messageExpiry:  messageExpiry,
		activationTime: activationTime,
	}
}

// AddChain adds a chain to the mock dependency set
func (m *MockDependencySet) AddChain(chainID eth.ChainID, activationTime uint64) {
	// Extract uint64 value from chainID
	chainValue, ok := chainID.Uint64()
	if !ok {
		panic(fmt.Errorf("chain ID too large: %v", chainID))
	}
	
	m.chainConfigs[chainID] = &depset.StaticConfigDependency{
		ChainIndex:     types.ChainIndex(chainValue),
		ActivationTime: activationTime,
		HistoryMinTime: activationTime - 1, // Set HistoryMinTime just before activation time
	}
}

// Chains returns all chains in the dependency set
func (m *MockDependencySet) Chains() []eth.ChainID {
	chains := make([]eth.ChainID, 0, len(m.chainConfigs))
	for chain := range m.chainConfigs {
		chains = append(chains, chain)
	}
	return chains
}

// CanInitiateAt checks if the chain can initiate messages at the given timestamp
func (m *MockDependencySet) CanInitiateAt(chain eth.ChainID, timestamp uint64) (bool, error) {
	cfg, ok := m.chainConfigs[chain]
	if !ok {
		return false, fmt.Errorf("unknown chain: %s", chain)
	}
	
	// Can initiate if timestamp > activation time (changed from >= to > for consistency with the fix in TS PR)
	return timestamp > cfg.ActivationTime, nil
}

// CanReceiveAt checks if the chain can receive messages at the given timestamp
func (m *MockDependencySet) CanReceiveAt(chain eth.ChainID, timestamp uint64) (bool, error) {
	// Use same logic as initiation for the mock
	return m.CanInitiateAt(chain, timestamp)
}

// MessageExpiryWindow returns the message expiry window in seconds
func (m *MockDependencySet) MessageExpiryWindow() uint64 {
	return m.messageExpiry
}

// ReverseChainLookup looks up a chain ID by index
func (m *MockDependencySet) ReverseChainLookup(idx types.ChainIndex) (eth.ChainID, error) {
	for chain, cfg := range m.chainConfigs {
		if cfg.ChainIndex == idx {
			return chain, nil
		}
	}
	return eth.ChainID{}, fmt.Errorf("unknown chain index: %d", idx)
}

// ValidMessageLifespan checks if a message is within its valid lifespan
func (m *MockDependencySet) ValidMessageLifespan(timestamp uint64) (bool, error) {
	now := uint64(time.Now().Unix())
	
	if timestamp > now {
		return false, fmt.Errorf("message timestamp is in the future")
	}
	
	age := now - timestamp
	return age <= m.messageExpiry, nil
}