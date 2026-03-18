package tee

import (
	"context"
	"errors"
	"fmt"
	"math"
	"time"

	"github.com/ethereum-optimism/optimism/op-challenger/game/fault/contracts"
	"github.com/ethereum-optimism/optimism/op-challenger/game/generic"
	gameTypes "github.com/ethereum-optimism/optimism/op-challenger/game/types"
	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/ethereum-optimism/optimism/op-service/sources/batching/rpcblock"
	"github.com/ethereum-optimism/optimism/op-service/txmgr"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/log"
)

var (
	errNoProveRequired      = errors.New("no prove required")
	errNoResolutionRequired = errors.New("no resolution required")
)

// ClockReader provides the current time.
type ClockReader interface {
	Now() time.Time
}

// TxSender sends transactions.
type TxSender interface {
	SendAndWaitSimple(txPurpose string, txs ...txmgr.TxCandidate) error
}

// GameStatusProvider queries parent game status from the factory.
type GameStatusProvider interface {
	GetGameStatus(ctx context.Context, idx uint64) (gameTypes.GameStatus, error)
}

// ProvableContract defines the contract methods the TEE Actor needs.
type ProvableContract interface {
	Addr() common.Address
	GetChallengerMetadata(ctx context.Context, block rpcblock.Block) (contracts.ChallengerMetadata, error)
	GetProveParams(ctx context.Context, factory *contracts.DisputeGameFactoryContract) (contracts.TeeProveParams, error)
	ProveTx(ctx context.Context, proofBytes []byte) (txmgr.TxCandidate, error)
	ResolveTx() (txmgr.TxCandidate, error)
}

// proveResult holds the result from a background ProveAndWait goroutine.
type proveResult struct {
	proofBytes []byte
	err        error
}

// Actor is a TEE dispute game actor that defends proposals by submitting TEE proofs.
// It uses a background goroutine pattern: when a prove is needed, a goroutine is started
// that calls ProveAndWait (which polls at the user-configured interval). Act() checks
// for results via a non-blocking channel read.
type Actor struct {
	logger             log.Logger
	l1Clock            ClockReader
	contract           ProvableContract
	proverClient       *ProverClient
	txSender           TxSender
	gameStatusProvider GameStatusProvider
	factory            *contracts.DisputeGameFactoryContract
	serviceCtx         context.Context  // service-level ctx, outlives individual Act() calls
	proveResultCh      chan proveResult // buffered(1), receives result from background goroutine
	proveInFlight      bool             // whether a background prove goroutine is running
}

// ActorCreator returns a generic.ActorCreator that creates TEE Actors.
func ActorCreator(
	serviceCtx context.Context,
	l1Clock ClockReader,
	proverClient *ProverClient,
	gameStatusProvider GameStatusProvider,
	contract ProvableContract,
	txSender TxSender,
	factory *contracts.DisputeGameFactoryContract,
) generic.ActorCreator {
	return func(ctx context.Context, logger log.Logger, l1Head eth.BlockID) (generic.Actor, error) {
		return &Actor{
			logger:             logger,
			l1Clock:            l1Clock,
			contract:           contract,
			proverClient:       proverClient,
			txSender:           txSender,
			gameStatusProvider: gameStatusProvider,
			factory:            factory,
			serviceCtx:         serviceCtx,
			proveResultCh:      make(chan proveResult, 1),
		}, nil
	}
}

func (a *Actor) Act(ctx context.Context) error {
	metadata, err := a.contract.GetChallengerMetadata(ctx, rpcblock.Latest)
	if err != nil {
		return fmt.Errorf("failed to get tee game state: %w", err)
	}

	var txs []txmgr.TxCandidate

	// 1. Non-blocking check for background prove result
	select {
	case result := <-a.proveResultCh:
		a.proveInFlight = false
		if result.err != nil {
			a.logger.Error("Background TEE prove failed", "err", result.err, "game", a.contract.Addr())
			// Don't return error — allow resolve logic to proceed, prove can retry next Act cycle
		} else {
			a.logger.Info("Background TEE prove finished, submitting proof", "game", a.contract.Addr())
			tx, err := a.contract.ProveTx(ctx, result.proofBytes)
			if err != nil {
				return fmt.Errorf("failed to create prove tx: %w", err)
			}
			txs = append(txs, tx)
		}
	default:
		// No result yet
	}

	// 2. Start background prove if needed and not already in flight
	if len(txs) == 0 && !a.proveInFlight {
		if err := a.tryStartProve(ctx, metadata); errors.Is(err, errNoProveRequired) {
			a.logger.Debug("No prove required")
		} else if err != nil {
			return err
		}
	}

	// 3. Try resolve
	if tx, err := a.createResolveTx(ctx, metadata); errors.Is(err, errNoResolutionRequired) {
		a.logger.Debug("No resolution required")
	} else if err != nil {
		return err
	} else {
		txs = append(txs, tx)
	}

	if len(txs) == 0 {
		return nil
	}
	if err := a.txSender.SendAndWaitSimple(fmt.Sprintf("respond to tee game %v", a.contract.Addr()), txs...); err != nil {
		return fmt.Errorf("failed to send transactions for tee game %v: %w", a.contract.Addr(), err)
	}
	return nil
}

// tryStartProve checks if a TEE proof needs to be submitted and starts a background goroutine.
func (a *Actor) tryStartProve(ctx context.Context, metadata contracts.ChallengerMetadata) error {
	if metadata.ProposalStatus != contracts.ProposalStatusChallenged {
		return errNoProveRequired
	}
	if metadata.Deadline.Before(a.l1Clock.Now()) {
		return errNoProveRequired
	}

	params, err := a.contract.GetProveParams(ctx, a.factory)
	if err != nil {
		return fmt.Errorf("failed to get prove params: %w", err)
	}

	req := ProveRequest{
		PreAppHash:       params.StartStateHash,
		PostAppHash:      params.EndStateHash,
		StartBlockHeight: params.StartBlockNum,
		EndBlockHeight:   params.EndBlockNum,
		StartBlockHash:   params.StartBlockHash,
		EndBlockHash:     params.EndBlockHash,
	}

	a.logger.Info("Starting background TEE prove",
		"startBlock", params.StartBlockNum,
		"endBlock", params.EndBlockNum,
		"game", a.contract.Addr())

	a.proveInFlight = true
	go func() {
		// Use serviceCtx so the goroutine survives individual Act() calls,
		// but is cancelled when the service shuts down.
		proofBytes, err := a.proverClient.ProveAndWait(a.serviceCtx, req)
		a.proveResultCh <- proveResult{proofBytes: proofBytes, err: err}
	}()

	return nil
}

// createResolveTx determines if the game should be resolved and constructs the transaction.
func (a *Actor) createResolveTx(ctx context.Context, metadata contracts.ChallengerMetadata) (txmgr.TxCandidate, error) {
	if metadata.ProposalStatus == contracts.ProposalStatusResolved {
		return txmgr.TxCandidate{}, errNoResolutionRequired
	}

	deadlineExpired := metadata.Deadline.Before(a.l1Clock.Now())

	// Check parent game status
	if metadata.ParentIndex != math.MaxUint32 {
		parentStatus, err := a.gameStatusProvider.GetGameStatus(ctx, uint64(metadata.ParentIndex))
		if err != nil {
			return txmgr.TxCandidate{}, fmt.Errorf("failed to get parent game status: %w", err)
		}
		if parentStatus == gameTypes.GameStatusInProgress {
			return txmgr.TxCandidate{}, errNoResolutionRequired
		}
		if parentStatus == gameTypes.GameStatusChallengerWon {
			return a.contract.ResolveTx()
		}
	}

	// Resolve if a valid proof has been provided
	if metadata.ProposalStatus == contracts.ProposalStatusChallengedAndValidProofProvided ||
		metadata.ProposalStatus == contracts.ProposalStatusUnchallengedAndValidProofProvided {
		return a.contract.ResolveTx()
	}

	// Resolve if deadline expired
	if deadlineExpired {
		return a.contract.ResolveTx()
	}

	return txmgr.TxCandidate{}, errNoResolutionRequired
}

func (a *Actor) AdditionalStatus(ctx context.Context) ([]any, error) {
	metadata, err := a.contract.GetChallengerMetadata(ctx, rpcblock.Latest)
	if err != nil {
		return nil, fmt.Errorf("failed to get challenger metadata: %w", err)
	}
	status := []any{"proposalStatus", metadata.ProposalStatus}
	if a.proveInFlight {
		status = append(status, "proveInFlight", true)
	}
	return status, nil
}

func decodeProofBytes(hexStr string) ([]byte, error) {
	if len(hexStr) >= 2 && hexStr[:2] == "0x" {
		hexStr = hexStr[2:]
	}
	return common.Hex2Bytes(hexStr), nil
}
