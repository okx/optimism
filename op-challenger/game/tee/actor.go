package tee

import (
	"context"
	"encoding/hex"
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
	From() common.Address
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
	ProveTx(ctx context.Context, proofBytes []byte, from common.Address) (txmgr.TxCandidate, error)
	ResolveTx() (txmgr.TxCandidate, error)
}

// proveResult holds the result from a background ProveAndWait goroutine.
type proveResult struct {
	proofBytes []byte
	err        error
}

// Actor is a TEE dispute game actor that defends proposals by submitting TEE proofs.
// It uses a background goroutine pattern: when a prove is needed, a goroutine is started
// that calls ProveAndWait (which retries and polls at the user-configured interval). Act()
// checks for results via a non-blocking channel read.
type Actor struct {
	logger        log.Logger
	l1Clock       ClockReader
	contract      ProvableContract
	proverClient  *ProverClient
	txSender           TxSender
	gameStatusProvider GameStatusProvider
	factory            *contracts.DisputeGameFactoryContract
	proveTimeout  time.Duration    // total timeout for prove attempts including retries
	serviceCtx    context.Context  // service-level ctx, outlives individual Act() calls
	proveResultCh chan proveResult // buffered(1), receives result from background goroutine
	proveInFlight bool             // whether a background prove goroutine is running
	proveGivenUp  bool             // true after prove timeout or non-retryable error — no more retries
}

// ActorCreator returns a generic.ActorCreator that creates TEE Actors.
func ActorCreator(
	serviceCtx context.Context,
	l1Clock ClockReader,
	proverClient *ProverClient,
	proveTimeout time.Duration,
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
			gameStatusProvider: factory,
			factory:            factory,
			proveTimeout:       proveTimeout,
			serviceCtx:         serviceCtx,
			proveResultCh:      make(chan proveResult, 1),
		}, nil
	}
}

// NOTE: must be called from a single goroutine.
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
			// Two possible errors from ProveAndWait:
			// 1. context.DeadlineExceeded — proveTimeout (default 1h) expired after retries
			// 2. errNonRetryable — code=10001 invalid params, retrying won't help
			a.proveGivenUp = true
			if errors.Is(result.err, context.DeadlineExceeded) {
				a.logger.Error("TEE prove timed out, giving up",
					"timeout", a.proveTimeout, "game", a.contract.Addr())
			} else if errors.Is(result.err, errNonRetryable) {
				a.logger.Error("TEE prove failed with non-retryable error, giving up",
					"err", result.err, "game", a.contract.Addr())
			} else {
				a.logger.Error("TEE prove failed, giving up",
					"err", result.err, "game", a.contract.Addr())
			}
		} else {
			a.logger.Info("Background TEE prove finished, submitting proof", "game", a.contract.Addr())
			tx, err := a.contract.ProveTx(ctx, result.proofBytes, a.txSender.From())
			if err != nil {
				return fmt.Errorf("failed to create prove tx: %w", err)
			}
			txs = append(txs, tx)
		}
	default:
		// No result yet
	}

	// 2. Start background prove if needed, not already in flight, and not given up
	if len(txs) == 0 && !a.proveInFlight && !a.proveGivenUp {
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
		if errors.Is(err, contracts.ErrAnchorGameUnprovable) {
			a.proveGivenUp = true
			a.logger.Warn("Anchor-based game cannot be proved, giving up", "game", a.contract.Addr())
			return errNoProveRequired
		}
		return fmt.Errorf("failed to get prove params: %w", err)
	}

	req := ProveRequest{
		StartBlkHeight:    params.StartBlockNum,
		EndBlkHeight:      params.EndBlockNum,
		StartBlkHash:      params.StartBlockHash,
		EndBlkHash:        params.EndBlockHash,
		StartBlkStateHash: params.StartStateHash,
		EndBlkStateHash:   params.EndStateHash,
	}

	a.logger.Info("Starting background TEE prove",
		"startBlock", params.StartBlockNum,
		"endBlock", params.EndBlockNum,
		"game", a.contract.Addr())

	a.proveInFlight = true
	go func() {
		// Use proveTimeout to bound the total prove time (including retries).
		// The ctx is derived from serviceCtx so the goroutine survives individual
		// Act() calls, but is cancelled when the service shuts down or timeout expires.
		timeoutCtx, cancel := context.WithTimeout(a.serviceCtx, a.proveTimeout)
		defer cancel()
		proofBytes, err := a.proverClient.ProveAndWait(timeoutCtx, req)
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
	if a.proveGivenUp {
		status = append(status, "proveGivenUp", true)
	}
	return status, nil
}

func decodeProofBytes(hexStr string) ([]byte, error) {
	if len(hexStr) >= 2 && hexStr[:2] == "0x" {
		hexStr = hexStr[2:]
	}
	return hex.DecodeString(hexStr)
}
