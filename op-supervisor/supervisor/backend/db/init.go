package db

import (
	"errors"
	"fmt"

	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/ethereum-optimism/optimism/op-supervisor/supervisor/backend/depset"
	"github.com/ethereum-optimism/optimism/op-supervisor/supervisor/backend/superevents"
	"github.com/ethereum-optimism/optimism/op-supervisor/supervisor/types"
)

// ForceInitialized marks the derived chain databases as initialized, even if it is not.
// This function is for testing purposes only and should not be used in production code.
func (db *ChainsDB) ForceInitialized(id eth.ChainID) {
	db.initialized.Set(id, struct{}{})
}

// isInitialized returns whether the derived DBs for the given chain id are initialized.
// Initialization of the logs db is not covered by the initialization flag.
func (db *ChainsDB) isInitialized(id eth.ChainID) bool {
	_, ok := db.initialized.Get(id)
	return ok
}

// isLocalInitialized returns whether the local-safe DB for the given chain id is initialized.
func (db *ChainsDB) isLocalInitialized(id eth.ChainID) bool {
	ldb, ok := db.localDBs.Get(id)
	return ok && !ldb.IsEmpty()
}

// isCrossInitialized returns whether the cross-safe DB for the given chain id is initialized.
func (db *ChainsDB) isCrossInitialized(id eth.ChainID) bool {
	xdb, ok := db.crossDBs.Get(id)
	return ok && !xdb.IsEmpty()
}

// EnsureInitialized initializes all databases from the given genesis block, if not already initialized.
// It should be used at startup to initialize the ChainsDB for chains that have Interop enabled at genesis.
// Chains that do not have Interop enabled at genesis will be initialized by managed sync-node events.
func (db *ChainsDB) EnsureInitialized(id eth.ChainID, genesis depset.Genesis) error {
	logger := db.logger.New("chain", id, "genesisL2", genesis.L2, "genesisL1", genesis.L1)
	if db.isInitialized(id) {
		logger.Debug("DB already initialized")
		return nil
	}

	genesisRef := types.DerivedBlockRefPair{
		// Initialization skips parent checks, so zero parents are ok.
		Source:  genesis.L1.WithZeroParent(),
		Derived: genesis.L2.WithZeroParent(),
	}
	if err := db.maybeInitFromUnsafe(id, genesisRef.Derived); err != nil {
		return fmt.Errorf("initializing logs database from genesis: %w", err)
	}
	if !db.isCrossInitialized(id) {
		if err := db.initCrossSafe(id, genesisRef); err != nil {
			return fmt.Errorf("initializing cross-safe database from genesis: %w", err)
		}
	}
	if !db.isLocalInitialized(id) {
		if err := db.initLocalSafe(id, genesisRef); err != nil {
			return fmt.Errorf("initializing local-safe database from genesis: %w", err)
		}
	}

	/* TODO: not sure this is needed
	db.finalizedL1.Set(genesisRef.Source)
	fin, err := db.Finalized(chain)
	if err != nil {
		return fmt.Errorf("getting finalized block: %w", err)
	}
	// TODO: set status
	*/

	return nil
}

// initCrossSafe initializes the cross-safe DB of the ChainsDB.
// The local-safe DB needs to be initialized separately using [ChainsDB.initLocalSafe] and it
// needs to be initialized after the cross-safe DB.
// The logs DB needs to be initialized separately using [ChainsDB.maybeInitFromUnsafe].
// initCrossSafe should only be called on an uninitialized cross-safe DB, otherwise it will return an error.
func (db *ChainsDB) initCrossSafe(id eth.ChainID, derived types.DerivedBlockRefPair) error {
	logger := db.logger.New("chain", id, "derived", derived.Derived, "source", derived.Source)

	logger.Debug("initializing cross-safe database from derived block")
	crossDB, ok := db.crossDBs.Get(id)
	if !ok {
		return types.ErrUnknownChain
	} else if !crossDB.IsEmpty() {
		return errors.New("cross-safe database already initialized")
	}

	if err := crossDB.AddDerived(derived.Source, derived.Derived, types.Revision(0)); err != nil {
		return fmt.Errorf("setting first entry in cross-safe DB: %w", err)
	}

	logger.Info("Cross-safe database initialized")
	db.emitter.Emit(superevents.CrossSafeUpdateEvent{
		ChainID:      id,
		NewCrossSafe: derived.Seals(),
	})
	db.m.RecordCrossSafeRef(id, derived.Derived)
	return nil
}

// initLocalSafe initializes the local-safe DB of the ChainsDB.
// The cross-safe DB needs to initialized separately using [ChainsDB.initCrossSafe] and it
// needs to be initialized before the local-safe DB.
// The logs DB needs to be initialized separately using [ChainsDB.maybeInitFromUnsafe].
// initLocalSafe should only be called on an uninitialized local-safe DB, otherwise it will return an error.
func (db *ChainsDB) initLocalSafe(id eth.ChainID, derived types.DerivedBlockRefPair) error {
	logger := db.logger.New("chain", id, "derived", derived.Derived, "source", derived.Source)

	if !db.isCrossInitialized(id) {
		return errors.New("cross-safe database must be initialized before local-safe")
	}

	logger.Debug("initializing local-safe database from derived block")
	localDB, ok := db.localDBs.Get(id)
	if !ok {
		return types.ErrUnknownChain
	} else if !localDB.IsEmpty() {
		return errors.New("local-safe database already initialized")
	}

	if err := localDB.AddDerived(derived.Source, derived.Derived, types.Revision(0)); err != nil {
		return fmt.Errorf("setting first entry in local-safe DB: %w", err)
	}

	// Mark the derived chain databases as initialized
	db.initialized.Set(id, struct{}{})

	logger.Info("Local-safe database initialized")
	db.emitter.Emit(superevents.LocalSafeUpdateEvent{
		ChainID:      id,
		NewLocalSafe: derived.Seals(),
	})
	return nil
}

/*
// initFromDerived initializes the derived DBs (local & cross-safe) of the ChainsDB.
// The logs DB needs to initialized separately using [ChainsDB.maybeInitFromUnsafe].
// initFromDerived should only be called on an uninitialized ChainsDB, otherwise it will return an error.
func (db *ChainsDB) initFromDerived(id eth.ChainID, derived types.DerivedBlockRefPair) error {
	logger := db.logger.New("chain", id, "derived", derived.Derived, "source", derived.Source)
	if db.isInitialized(id) {
		return errors.New("derived databases already initialized")
	}

	logger.Debug("initializing derived databases from derived block")
	localDB, ok := db.localDBs.Get(id)
	if !ok {
		return types.ErrUnknownChain
	}

	first, err := localDB.First()
	if errors.Is(err, types.ErrFuture) {
		logger.Info("Initializing derived databases")
		if err := db.initializedUpdateCrossSafe(id, derived.Source, derived.Derived); err != nil {
			return err
		}
		// "anchor" is not a node, so failure to update won't be caught by any SyncNode
		db.initializedUpdateLocalSafe(id, derived.Source, derived.Derived, "anchor")

		// Mark the derived chain databases as initialized
		db.initialized.Set(id, struct{}{})
		return nil
	} else if err != nil {
		return fmt.Errorf("failed to check if chain database is initialized: %w", err)
	} else {
		logger.Warn("derived databases already initialized")
		if first.Derived.Hash != derived.Derived.Hash ||
			first.Source.Hash != derived.Source.Hash {
			return fmt.Errorf("initialized local safe (%s) does not match anchor point (%s): %w",
				first,
				derived,
				types.ErrConflict)
		}
		return errors.New("derived databases already initialized")
	}
}
*/

func (db *ChainsDB) maybeInitFromUnsafe(id eth.ChainID, unsafe eth.BlockRef) error {
	logger := db.logger.New("chain", id, "unsafe", unsafe)
	seal, err := db.FindSealedBlock(id, unsafe.Number)
	if errors.Is(err, types.ErrFuture) {
		logger.Debug("Initializing events database")
		err := db.sealBlock(id, unsafe, true)
		if err != nil {
			return err
		}
		logger.Info("Initialized events database")
		if err := db.UpdateCrossUnsafe(id, types.BlockSealFromRef(unsafe)); err != nil {
			return fmt.Errorf("failed updating cross unsafe: %w", err)
		}
		return nil
	} else if err != nil {
		return fmt.Errorf("failed to check if logDB is initialized: %w", err)
	}

	logger.Warn("Events database already initialized")
	// TODO(#15774): make sure the Rewinder can handle reorgs of the activation block
	if seal.Hash != unsafe.Hash {
		return fmt.Errorf("events database (%s) does not match anchor point (%s): %w",
			seal,
			unsafe,
			types.ErrConflict)
	}
	return nil
}
