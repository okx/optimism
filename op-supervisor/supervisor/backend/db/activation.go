package db

import (
	"fmt"

	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/ethereum-optimism/optimism/op-supervisor/supervisor/types"
)

// GetAnchorL2Block returns the first block used as an anchor point for the chain
func (db *ChainsDB) GetAnchorL2Block(chain eth.ChainID) (eth.BlockRef, error) {
	crossDB, ok := db.crossDBs.Get(chain)
	if !ok {
		return eth.BlockRef{}, fmt.Errorf("no cross-safe DB for chain %s", chain)
	}

	firstPair, err := crossDB.First()
	if err != nil {
		return eth.BlockRef{}, fmt.Errorf("failed to get first entry from cross-safe DB: %w", err)
	}

	return eth.BlockRef{
		Hash:   firstPair.Derived.Hash,
		Number: firstPair.Derived.Number,
		Time:   firstPair.Derived.Timestamp,
	}, nil
}

// InitializeWithAnchor initializes the database with the given anchor point
func (db *ChainsDB) InitializeWithAnchor(chain eth.ChainID, anchor types.DerivedBlockRefPair) {
	db.initFromAnchor(chain, anchor)
}
