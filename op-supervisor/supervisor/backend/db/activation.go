package db

import (
	"fmt"

	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/ethereum-optimism/optimism/op-supervisor/supervisor/types"
)

// GetAnchorL2Block returns the anchor block for the specified chain
// Used by the activation manager for boundary checks
func (db *ChainsDB) GetAnchorL2Block(chain eth.ChainID) (eth.BlockRef, error) {
	anchor, ok := db.GetAnchorBlock(chain)
	if !ok {
		return eth.BlockRef{}, fmt.Errorf("no anchor block for chain %s", chain)
	}
	return anchor.Derived, nil
}

// InitializeWithAnchor initializes the database with the given anchor point
func (db *ChainsDB) InitializeWithAnchor(chain eth.ChainID, anchor types.DerivedBlockRefPair) {
	db.initFromAnchor(chain, anchor)
}
