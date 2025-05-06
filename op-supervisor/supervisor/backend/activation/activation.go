package activation

import (
	"github.com/ethereum/go-ethereum/log"

	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/ethereum-optimism/optimism/op-supervisor/supervisor/backend/depset"
)

// CheckFn is a function that checks if interop is active for a given chain and timestamp.
type CheckFn func(chain eth.ChainID, timestamp uint64) bool

func NewCheckFn(depSet depset.DependencySet, logger log.Logger) CheckFn {
	return func(chain eth.ChainID, timestamp uint64) bool {
		// If we don't have a dependency set then interop is never active
		if depSet == nil {
			return false
		}

		// Interop is active if the chain can initiate at the given timestamp
		canInitiate, err := depSet.CanInitiateAt(chain, timestamp)
		if err != nil {
			logger.Debug("Error checking interop activation", "chain", chain, "timestamp", timestamp, "err", err)
			return false
		}
		return canInitiate
	}
}
