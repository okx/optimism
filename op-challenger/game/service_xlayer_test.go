package game

import (
	"context"
	"testing"

	"github.com/ethereum-optimism/optimism/op-challenger/game/types"
	"github.com/ethereum/go-ethereum/common"
	"github.com/stretchr/testify/require"
)

func TestFilteredGameSource(t *testing.T) {
	allGames := []types.GameMetadata{
		{Proxy: common.Address{0x01}, GameType: 1960},
		{Proxy: common.Address{0x02}, GameType: 42},
		{Proxy: common.Address{0x03}, GameType: 1960},
		{Proxy: common.Address{0x04}, GameType: 1},
	}
	stub := &stubFilterSource{games: allGames}

	t.Run("FiltersToEnabledTypes", func(t *testing.T) {
		source := newFilteredGameSource(stub, []types.GameType{1960})
		games, err := source.GetGamesAtOrAfter(context.Background(), common.Hash{}, 0)
		require.NoError(t, err)
		require.Len(t, games, 2)
		require.Equal(t, uint32(1960), games[0].GameType)
		require.Equal(t, uint32(1960), games[1].GameType)
	})

	t.Run("EmptyTypesReturnsAll", func(t *testing.T) {
		source := newFilteredGameSource(stub, nil)
		games, err := source.GetGamesAtOrAfter(context.Background(), common.Hash{}, 0)
		require.NoError(t, err)
		require.Len(t, games, 4)
	})

	t.Run("MultipleEnabledTypes", func(t *testing.T) {
		source := newFilteredGameSource(stub, []types.GameType{1960, 1})
		games, err := source.GetGamesAtOrAfter(context.Background(), common.Hash{}, 0)
		require.NoError(t, err)
		require.Len(t, games, 3)
	})

	t.Run("NoMatchingTypes", func(t *testing.T) {
		source := newFilteredGameSource(stub, []types.GameType{999})
		games, err := source.GetGamesAtOrAfter(context.Background(), common.Hash{}, 0)
		require.NoError(t, err)
		require.Empty(t, games)
	})
}

type stubFilterSource struct {
	games []types.GameMetadata
}

func (s *stubFilterSource) GetGamesAtOrAfter(_ context.Context, _ common.Hash, _ uint64) ([]types.GameMetadata, error) {
	return s.games, nil
}
