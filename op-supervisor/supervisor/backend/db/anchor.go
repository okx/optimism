package db

import (
	"errors"
	"fmt"

	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/ethereum-optimism/optimism/op-supervisor/supervisor/types"
)

func (db *ChainsDB) ForceInitialized(id eth.ChainID) {
	db.initialized.Set(id, struct{}{})
}

func (db *ChainsDB) isInitialized(id eth.ChainID) bool {
	_, ok := db.initialized.Get(id)
	return ok
}

func (db *ChainsDB) IsInitialized(id eth.ChainID) bool {
	return db.isInitialized(id)
}

func (db *ChainsDB) GetAnchorBlock(id eth.ChainID) (types.DerivedBlockRefPair, bool) {
	anchor, ok := db.anchorBlocks.Get(id)
	return anchor, ok
}

func (db *ChainsDB) GetAnchorL2Block(id eth.ChainID) (eth.BlockRef, error) {
	anchor, ok := db.anchorBlocks.Get(id)
	if !ok {
		return eth.BlockRef{}, fmt.Errorf("no anchor block for chain %s", id)
	}
	return anchor.Derived, nil
}

func (db *ChainsDB) IsInInteropMode(id eth.ChainID) bool {
	_, ok := db.anchorBlocks.Get(id)
	return ok
}

func (db *ChainsDB) InitializeWithAnchor(id eth.ChainID, anchor types.DerivedBlockRefPair) {
	db.initFromAnchor(id, anchor)
}

func (db *ChainsDB) initFromAnchor(id eth.ChainID, anchor types.DerivedBlockRefPair) {
	// Check if the chain database is already initialized
	if db.isInitialized(id) && db.IsInInteropMode(id) {
		db.logger.Debug("chain database already initialized with anchor block")
		return
	}
	db.logger.Debug("initializing chain database from anchor point")

	// Initialize the local and cross safe databases
	if err := db.maybeInitSafeDB(id, anchor); err != nil {
		db.logger.Warn("failed to initialize local and cross safe databases", "err", err)
		return
	}

	// Initialize the events database
	if err := db.maybeInitEventsDB(id, anchor); err != nil {
		db.logger.Warn("failed to initialize events database", "err", err)
		return
	}

	// Store the anchor block
	db.anchorBlocks.Set(id, anchor)

	// Mark the chain database as initialized
	db.initialized.Set(id, struct{}{})

	db.logger.Info("Chain initialized with anchor block for interop",
		"chain", id,
		"anchorSource", anchor.Source,
		"anchorDerived", anchor.Derived)
}

func (db *ChainsDB) maybeInitSafeDB(id eth.ChainID, anchor types.DerivedBlockRefPair) error {
	logger := db.logger.New("chain", id, "derived", anchor.Derived, "source", anchor.Source)
	localDB, ok := db.localDBs.Get(id)
	if !ok {
		return types.ErrUnknownChain
	}
	first, err := localDB.First()
	if errors.Is(err, types.ErrFuture) {
		logger.Info("local database is empty, initializing")
		if err := db.initializedUpdateCrossSafe(id, anchor.Source, anchor.Derived); err != nil {
			return err
		}
		// "anchor" is not a node, so failure to update won't be caught by any SyncNode
		db.initializedUpdateLocalSafe(id, anchor.Source, anchor.Derived, "anchor")
	} else if err != nil {
		return fmt.Errorf("failed to check if chain database is initialized: %w", err)
	} else {
		logger.Debug("chain database already initialized")
		if first.Derived.Hash != anchor.Derived.Hash ||
			first.Source.Hash != anchor.Source.Hash {
			return fmt.Errorf("local database (%s) does not match anchor point (%s): %w",
				first,
				anchor,
				types.ErrConflict)
		}
	}
	return nil
}

func (db *ChainsDB) maybeInitEventsDB(id eth.ChainID, anchor types.DerivedBlockRefPair) error {
	logger := db.logger.New("chain", id, "derived", anchor.Derived, "source", anchor.Source)
	seal, _, _, err := db.OpenBlock(id, 0)
	if errors.Is(err, types.ErrFuture) {
		logger.Debug("initializing events database")

		// Seal the anchor block directly
		logDB, ok := db.logDBs.Get(id)
		if !ok {
			return fmt.Errorf("cannot SealBlock: %w: %v", types.ErrUnknownChain, id)
		}
		err := logDB.SealBlock(anchor.Derived.ParentHash, anchor.Derived.ID(), anchor.Derived.Time)
		if err != nil {
			return fmt.Errorf("failed to seal anchor block %v: %w", anchor.Derived, err)
		}

		logger.Info("Initialized events database")
	} else if err != nil {
		return fmt.Errorf("failed to check if logDB is initialized: %w", err)
	} else {
		logger.Debug("Events database already initialized")
		if seal.Hash != anchor.Derived.Hash {
			return fmt.Errorf("events database (%s) does not match anchor point (%s): %w",
				seal,
				anchor,
				types.ErrConflict)
		}
	}
	return nil
}
