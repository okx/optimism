// For xlayer: TEE-specific service initialization — wires parent index resolver into TeeRollupProposalSource.
package proposer

import (
	"context"
	"fmt"
	"math"

	"github.com/ethereum-optimism/optimism/op-proposer/contracts"
	"github.com/ethereum-optimism/optimism/op-proposer/proposer/source"
)

// initTeeSource wires the parent game index resolver into TeeRollupProposalSource.
// Must be called after NewL2OutputSubmitter returns so that driver.dgfContract is available.
// Returns an error if dgfContract is not *contracts.DisputeGameFactory, to prevent silent
// MaxUint32 sentinel usage in production.
func initTeeSource(ps *ProposerService, driver *L2OutputSubmitter) error {
	teeSource, ok := ps.ProposalSource.(*source.TeeRollupProposalSource)
	if !ok {
		return nil // not a TEE game type — nothing to wire
	}
	dgfCaller, ok := driver.dgfContract.(*contracts.DisputeGameFactory)
	if !ok {
		// dgfContract is not *contracts.DisputeGameFactory — fail fast to prevent silent MaxUint32 sentinel usage in production
		return fmt.Errorf("tee-rollup: dgfContract is not *contracts.DisputeGameFactory, cannot wire parentIdxFn")
	}
	proposer := driver.Txmgr.From()
	gameType := uint32(ps.ProposerConfig.DisputeGameType)
	teeSource.SetParentIdxFn(func(ctx context.Context) (uint32, bool, error) {
		idx, found, err := dgfCaller.FindLastGameIndex(ctx, gameType, proposer, contracts.TeeParentScanLimit)
		if err != nil {
			return 0, false, err
		}
		if !found {
			return 0, false, nil
		}
		if idx > math.MaxUint32 {
			return 0, false, fmt.Errorf("tee-rollup: game index %d exceeds uint32 range", idx)
		}
		return uint32(idx), true, nil
	})
	return nil
}
