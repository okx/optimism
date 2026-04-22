package game

import (
	"context"

	"github.com/ethereum-optimism/optimism/op-challenger/game/types"
	"github.com/ethereum/go-ethereum/common"
)

// filteredGameSource wraps a gameSource and only returns games whose type is in the enabled set.
// When enabledTypes is empty, all games are returned (no filtering).
type filteredGameSource struct {
	inner        gameSource
	enabledTypes map[uint32]bool
}

func newFilteredGameSource(inner gameSource, gameTypes []types.GameType) gameSource {
	if len(gameTypes) == 0 {
		return inner
	}
	enabled := make(map[uint32]bool, len(gameTypes))
	for _, t := range gameTypes {
		enabled[uint32(t)] = true
	}
	return &filteredGameSource{inner: inner, enabledTypes: enabled}
}

func (f *filteredGameSource) GetGamesAtOrAfter(ctx context.Context, blockHash common.Hash, earliestTimestamp uint64) ([]types.GameMetadata, error) {
	games, err := f.inner.GetGamesAtOrAfter(ctx, blockHash, earliestTimestamp)
	if err != nil {
		return nil, err
	}
	filtered := make([]types.GameMetadata, 0, len(games))
	for _, g := range games {
		if f.enabledTypes[g.GameType] {
			filtered = append(filtered, g)
		}
	}
	return filtered, nil
}
