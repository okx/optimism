package depset

import (
	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/ethereum-optimism/optimism/op-supervisor/supervisor/types"
)

type MockDependencySet struct {
	CanExecuteAtFn        func(chainID eth.ChainID, execTimestamp uint64) (bool, error)
	CanInitiateAtFn       func(chainID eth.ChainID, initTimestamp uint64) (bool, error)
	ChainsFn              func() []eth.ChainID
	HasChainFn            func(chainID eth.ChainID) bool
	ChainIndexFromIDFn    func(id eth.ChainID) (types.ChainIndex, error)
	ChainIDFromIndexFn    func(index types.ChainIndex) (eth.ChainID, error)
	MessageExpiryWindowFn func() uint64
}

func (m *MockDependencySet) CanExecuteAt(chainID eth.ChainID, execTimestamp uint64) (bool, error) {
	if m.CanExecuteAtFn != nil {
		return m.CanExecuteAtFn(chainID, execTimestamp)
	}
	return true, nil
}

func (m *MockDependencySet) CanInitiateAt(chainID eth.ChainID, initTimestamp uint64) (bool, error) {
	if m.CanInitiateAtFn != nil {
		return m.CanInitiateAtFn(chainID, initTimestamp)
	}
	return true, nil
}

func (m *MockDependencySet) Chains() []eth.ChainID {
	if m.ChainsFn != nil {
		return m.ChainsFn()
	}
	return []eth.ChainID{}
}

func (m *MockDependencySet) HasChain(chainID eth.ChainID) bool {
	if m.HasChainFn != nil {
		return m.HasChainFn(chainID)
	}
	return true
}

func (m *MockDependencySet) ChainIndexFromID(id eth.ChainID) (types.ChainIndex, error) {
	if m.ChainIndexFromIDFn != nil {
		return m.ChainIndexFromIDFn(id)
	}
	return types.ChainIndex(0), nil
}

func (m *MockDependencySet) ChainIDFromIndex(index types.ChainIndex) (eth.ChainID, error) {
	if m.ChainIDFromIndexFn != nil {
		return m.ChainIDFromIndexFn(index)
	}
	return eth.ChainID{}, nil
}

func (m *MockDependencySet) MessageExpiryWindow() uint64 {
	if m.MessageExpiryWindowFn != nil {
		return m.MessageExpiryWindowFn()
	}
	return uint64(7 * 24 * 60 * 60)
}

// Ensure MockDependencySet implements DependencySet
var _ DependencySet = (*MockDependencySet)(nil)
