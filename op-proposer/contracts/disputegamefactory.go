package contracts

import (
	"context"
	"fmt"
	"math/big"
	"strings"
	"time"

	"github.com/ethereum-optimism/optimism/op-service/bigs"
	"github.com/ethereum-optimism/optimism/op-service/sources/batching"
	"github.com/ethereum-optimism/optimism/op-service/sources/batching/rpcblock"
	"github.com/ethereum-optimism/optimism/op-service/txmgr"
	"github.com/ethereum-optimism/optimism/packages/contracts-bedrock/snapshots"
	"github.com/ethereum/go-ethereum/accounts/abi"
	"github.com/ethereum/go-ethereum/common"
)

const (
	methodGameCount   = "gameCount"
	methodGameAtIndex = "gameAtIndex"
	methodInitBonds   = "initBonds"
	methodCreateGame  = "create"
	methodVersion     = "version"

	methodClaim = "claimData"

	teeGameType uint32 = 1960 // For xlayer: TEE game type (TeeRollup)
)

// For xlayer: ABI for new game contract's no-arg claimData() struct getter
const newGameClaimDataABIJSON = `[{"name":"claimData","type":"function","inputs":[],"outputs":[{"name":"parentIndex","type":"uint32"},{"name":"counteredBy","type":"address"},{"name":"prover","type":"address"},{"name":"claim","type":"bytes32"},{"name":"status","type":"uint8"},{"name":"deadline","type":"uint64"}],"stateMutability":"view"}]`

// For xlayer: parsed ABI for new game contract's claimData() getter
var newGameClaimDataABI abi.ABI

func init() {
	var err error
	newGameClaimDataABI, err = abi.JSON(strings.NewReader(newGameClaimDataABIJSON))
	if err != nil {
		panic(fmt.Sprintf("failed to parse new game claim data ABI: %v", err))
	}
}

type gameMetadata struct {
	GameType  uint32
	Timestamp time.Time
	Address   common.Address
	Proposer  common.Address
	Claim     common.Hash
}

type DisputeGameFactory struct {
	caller         *batching.MultiCaller
	contract       *batching.BoundContract
	gameABI        *abi.ABI
	networkTimeout time.Duration
}

func NewDisputeGameFactory(addr common.Address, caller *batching.MultiCaller, networkTimeout time.Duration) *DisputeGameFactory {
	factoryABI := snapshots.LoadDisputeGameFactoryABI()
	// Note: Games might have different ABIs (eg SuperFaultDisputeGame) but since only a very small part of the ABI
	// is actually needed, proposer always uses the latest FaultDisputeGameABI. Compatibility with other ABIs is tested
	// in disputegamefactory_test.go
	gameABI := snapshots.LoadFaultDisputeGameABI()
	return &DisputeGameFactory{
		caller:         caller,
		contract:       batching.NewBoundContract(factoryABI, addr),
		gameABI:        gameABI,
		networkTimeout: networkTimeout,
	}
}

func (f *DisputeGameFactory) Version(ctx context.Context) (string, error) {
	cCtx, cancel := context.WithTimeout(ctx, f.networkTimeout)
	defer cancel()
	result, err := f.caller.SingleCall(cCtx, rpcblock.Latest, f.contract.Call(methodVersion))
	if err != nil {
		return "", fmt.Errorf("failed to get version: %w", err)
	}
	return result.GetString(0), nil
}

// HasProposedSince attempts to find a game with the specified game type created by the specified proposer after the
// given cut off time. If one is found, returns true and the time the game was created at.
// If no matching proposal is found, returns false, time.Time{}, nil
func (f *DisputeGameFactory) HasProposedSince(ctx context.Context, proposer common.Address, cutoff time.Time, gameType uint32) (bool, time.Time, common.Hash, error) {
	gameCount, err := f.gameCount(ctx)
	if err != nil {
		return false, time.Time{}, common.Hash{}, fmt.Errorf("failed to get dispute game count: %w", err)
	}
	if gameCount == 0 {
		return false, time.Time{}, common.Hash{}, nil
	}
	for idx := gameCount - 1; ; idx-- {
		game, err := f.gameAtIndex(ctx, idx)
		if err != nil {
			return false, time.Time{}, common.Hash{}, fmt.Errorf("failed to get dispute game %d: %w", idx, err)
		}
		if game.Timestamp.Before(cutoff) {
			// Reached a game that is before the expected cutoff, so we haven't found a suitable proposal
			return false, time.Time{}, common.Hash{}, nil
		}
		if game.GameType == gameType && game.Proposer == proposer {
			// Found a matching proposal
			return true, game.Timestamp, game.Claim, nil
		}
		if idx == 0 { // Need to check here rather than in the for condition to avoid underflow
			// Checked every game and didn't find a match
			return false, time.Time{}, common.Hash{}, nil
		}
	}
}

func (f *DisputeGameFactory) ProposalTx(ctx context.Context, gameType uint32, outputRoot common.Hash, extraData []byte) (txmgr.TxCandidate, error) {
	cCtx, cancel := context.WithTimeout(ctx, f.networkTimeout)
	defer cancel()
	result, err := f.caller.SingleCall(cCtx, rpcblock.Latest, f.contract.Call(methodInitBonds, gameType))
	if err != nil {
		return txmgr.TxCandidate{}, fmt.Errorf("failed to fetch init bond: %w", err)
	}
	initBond := result.GetBigInt(0)
	call := f.contract.Call(methodCreateGame, gameType, outputRoot, extraData)
	candidate, err := call.ToTxCandidate()
	if err != nil {
		return txmgr.TxCandidate{}, err
	}
	candidate.Value = initBond
	return candidate, err
}

func (f *DisputeGameFactory) gameCount(ctx context.Context) (uint64, error) {
	cCtx, cancel := context.WithTimeout(ctx, f.networkTimeout)
	defer cancel()
	result, err := f.caller.SingleCall(cCtx, rpcblock.Latest, f.contract.Call(methodGameCount))
	if err != nil {
		return 0, fmt.Errorf("failed to load game count: %w", err)
	}
	return bigs.Uint64Strict(result.GetBigInt(0)), nil
}

func (f *DisputeGameFactory) gameAtIndex(ctx context.Context, idx uint64) (gameMetadata, error) {
	cCtx, cancel := context.WithTimeout(ctx, f.networkTimeout)
	defer cancel()
	result, err := f.caller.SingleCall(cCtx, rpcblock.Latest, f.contract.Call(methodGameAtIndex, new(big.Int).SetUint64(idx)))
	if err != nil {
		return gameMetadata{}, fmt.Errorf("failed to load game %v: %w", idx, err)
	}
	gameType := result.GetUint32(0)
	timestamp := result.GetUint64(1)
	address := result.GetAddress(2)

	var claimant common.Address
	var claim common.Hash
	if gameType == teeGameType {
		// For xlayer: TEE game type (1960) uses new contract ABI — claimData() takes no args,
		// returns (parentIndex, counteredBy, prover, claim, status, deadline).
		// prover is at index 2, claim (bytes32) is at index 3.
		newGameContract := batching.NewBoundContract(&newGameClaimDataABI, address)
		cCtx, cancel = context.WithTimeout(ctx, f.networkTimeout)
		defer cancel()
		result, err = f.caller.SingleCall(cCtx, rpcblock.Latest, newGameContract.Call(methodClaim))
		if err != nil {
			return gameMetadata{}, fmt.Errorf("failed to load root claim of game %v: %w", idx, err)
		}
		claimant = result.GetAddress(2)
		claim = result.GetHash(3)
	} else {
		gameContract := batching.NewBoundContract(f.gameABI, address)
		cCtx, cancel = context.WithTimeout(ctx, f.networkTimeout)
		defer cancel()
		result, err = f.caller.SingleCall(cCtx, rpcblock.Latest, gameContract.Call(methodClaim, big.NewInt(0)))
		if err != nil {
			return gameMetadata{}, fmt.Errorf("failed to load root claim of game %v: %w", idx, err)
		}
		// We don't need most of the claim data, only the claim and the claimant which is the game proposer
		claimant = result.GetAddress(2)
		claim = result.GetHash(4)
	}

	return gameMetadata{
		GameType:  gameType,
		Timestamp: time.Unix(int64(timestamp), 0),
		Address:   address,
		Proposer:  claimant,
		Claim:     claim,
	}, nil
}
