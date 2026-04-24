package tee

import (
	"context"
	"fmt"

	"github.com/ethereum-optimism/optimism/op-challenger/config"
	"github.com/ethereum-optimism/optimism/op-challenger/game/client"
	"github.com/ethereum-optimism/optimism/op-challenger/game/fault/claims"
	"github.com/ethereum-optimism/optimism/op-challenger/game/fault/contracts"
	"github.com/ethereum-optimism/optimism/op-challenger/game/generic"
	"github.com/ethereum-optimism/optimism/op-challenger/game/scheduler"
	gameTypes "github.com/ethereum-optimism/optimism/op-challenger/game/types"
	"github.com/ethereum-optimism/optimism/op-challenger/metrics"
	"github.com/ethereum/go-ethereum/log"
)

// RegisterGameTypes registers the TEE game type with the game type registry.
func RegisterGameTypes(
	ctx context.Context,
	l1Clock ClockReader,
	logger log.Logger,
	m metrics.Metricer,
	cfg *config.Config,
	registry Registry,
	txSender TxSender,
	clients *client.Provider,
	factoryContract *contracts.DisputeGameFactoryContract,
) error {
	if !cfg.GameTypeEnabled(gameTypes.TeeGameType) {
		return nil
	}

	// Query L1 chain ID once at startup for EIP-712 domain hints in prove requests.
	l1ChainID, err := clients.L1Client().ChainID(ctx)
	if err != nil {
		return fmt.Errorf("failed to get L1 chain ID: %w", err)
	}

	proverClient := NewProverClient(cfg.TeeProverRpc, cfg.TeeProvePollInterval, cfg.TeeProverHTTPTimeout, logger)
	proveTimeout := cfg.TeeProveTimeout

	registry.RegisterGameType(gameTypes.TeeGameType, func(game gameTypes.GameMetadata, dir string) (scheduler.GamePlayer, error) {
		contract, err := contracts.NewTeeDisputeGameContract(m, game.Proxy, clients.MultiCaller())
		if err != nil {
			return nil, fmt.Errorf("failed to create tee dispute game bindings: %w", err)
		}
		return generic.NewGenericGamePlayer(
			ctx,
			logger,
			game.Proxy,
			contract,
			&client.NoopSyncStatusValidator{},
			nil,
			clients.L1Client(),
			ActorCreator(ctx, l1Clock, l1ChainID.Uint64(), proverClient, proveTimeout, contract, txSender, factoryContract),
		)
	})

	registry.RegisterBondContract(gameTypes.TeeGameType, func(game gameTypes.GameMetadata) (claims.BondContract, error) {
		return contracts.NewTeeDisputeGameContract(m, game.Proxy, clients.MultiCaller())
	})

	return nil
}

// Registry is the interface for registering game types.
type Registry interface {
	RegisterGameType(gameType gameTypes.GameType, creator scheduler.PlayerCreator)
	RegisterBondContract(gameType gameTypes.GameType, creator claims.BondContractCreator)
}
