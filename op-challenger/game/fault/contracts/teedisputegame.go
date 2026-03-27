package contracts

import (
	"context"
	"fmt"
	"math"
	"math/big"
	"time"

	"github.com/ethereum-optimism/optimism/op-challenger/game/fault/contracts/metrics"
	"github.com/ethereum-optimism/optimism/op-challenger/game/fault/types"
	gameTypes "github.com/ethereum-optimism/optimism/op-challenger/game/types"
	"github.com/ethereum-optimism/optimism/op-service/sources/batching"
	"github.com/ethereum-optimism/optimism/op-service/sources/batching/rpcblock"
	"github.com/ethereum-optimism/optimism/op-service/txmgr"
	"github.com/ethereum-optimism/optimism/packages/contracts-bedrock/snapshots"
	"github.com/ethereum/go-ethereum/common"
)

var (
	methodProve     = "prove"
	methodProposer  = "proposer"
	methodBlockHash = "blockHash"
	methodStateHash = "stateHash"

	// ErrAnchorGameUnprovable is returned when a game uses anchor state (parentIndex=MaxUint32)
	// and cannot be proved because individual start hashes are not recoverable from the combined anchor root.
	ErrAnchorGameUnprovable = fmt.Errorf("anchor-based game (parentIndex=MaxUint32) cannot be proved: start hashes not recoverable")
)

// TeeProveParams contains the parameters needed to request a TEE proof.
type TeeProveParams struct {
	StartBlockHash common.Hash
	StartStateHash common.Hash
	EndBlockHash   common.Hash
	EndStateHash   common.Hash
	StartBlockNum  uint64
	EndBlockNum    uint64
}

// TeeDisputeGameContract defines the interface for interacting with TeeDisputeGame.
type TeeDisputeGameContract interface {
	DisputeGameContract

	GetChallengerMetadata(ctx context.Context, block rpcblock.Block) (ChallengerMetadata, error)
	GetProveParams(ctx context.Context, factory *DisputeGameFactoryContract) (TeeProveParams, error)
	ProveTx(ctx context.Context, proofBytes []byte, from common.Address) (txmgr.TxCandidate, error)
	GetProposer(ctx context.Context) (common.Address, error)

	// Bond-related (BondContract interface)
	GetCredit(ctx context.Context, recipient common.Address) (*big.Int, gameTypes.GameStatus, error)
	ClaimCreditTx(ctx context.Context, recipient common.Address) (txmgr.TxCandidate, error)
	GetBondDistributionMode(ctx context.Context, block rpcblock.Block) (types.BondDistributionMode, error)
	CloseGameTx(ctx context.Context) (txmgr.TxCandidate, error)
}

// TeeDisputeGameContractLatest implements TeeDisputeGameContract.
type TeeDisputeGameContractLatest struct {
	metrics     metrics.ContractMetricer
	multiCaller *batching.MultiCaller
	contract    *batching.BoundContract
}

var _ TeeDisputeGameContract = (*TeeDisputeGameContractLatest)(nil)
var _ DisputeGameContract = (*TeeDisputeGameContractLatest)(nil)

func NewTeeDisputeGameContract(
	m metrics.ContractMetricer,
	addr common.Address,
	caller *batching.MultiCaller,
) (*TeeDisputeGameContractLatest, error) {
	contractAbi := snapshots.LoadTeeDisputeGameABI()
	return &TeeDisputeGameContractLatest{
		metrics:     m,
		multiCaller: caller,
		contract:    batching.NewBoundContract(contractAbi, addr),
	}, nil
}

func (g *TeeDisputeGameContractLatest) Addr() common.Address {
	return g.contract.Addr()
}

func (g *TeeDisputeGameContractLatest) GetMetadata(ctx context.Context, block rpcblock.Block) (GenericGameMetadata, error) {
	defer g.metrics.StartContractRequest("GetMetadata")()
	results, err := g.multiCaller.Call(ctx, block,
		g.contract.Call(methodL1Head),
		g.contract.Call(methodL2SequenceNumber),
		g.contract.Call(methodRootClaim),
		g.contract.Call(methodStatus),
	)
	if err != nil {
		return GenericGameMetadata{}, fmt.Errorf("failed to retrieve game metadata: %w", err)
	}
	if len(results) != 4 {
		return GenericGameMetadata{}, fmt.Errorf("expected 4 results but got %v", len(results))
	}
	l1Head := results[0].GetHash(0)
	l2SequenceNumber := getBlockNumber(results[1], 0)
	rootClaim := results[2].GetHash(0)
	status, err := gameTypes.GameStatusFromUint8(results[3].GetUint8(0))
	if err != nil {
		return GenericGameMetadata{}, fmt.Errorf("failed to convert game status: %w", err)
	}
	return GenericGameMetadata{
		L1Head:        l1Head,
		L2SequenceNum: l2SequenceNumber,
		ProposedRoot:  rootClaim,
		Status:        status,
	}, nil
}

func (g *TeeDisputeGameContractLatest) GetL1Head(ctx context.Context) (common.Hash, error) {
	defer g.metrics.StartContractRequest("GetL1Head")()
	result, err := g.multiCaller.SingleCall(ctx, rpcblock.Latest, g.contract.Call(methodL1Head))
	if err != nil {
		return common.Hash{}, fmt.Errorf("failed to fetch L1 head: %w", err)
	}
	return result.GetHash(0), nil
}

func (g *TeeDisputeGameContractLatest) GetStatus(ctx context.Context) (gameTypes.GameStatus, error) {
	defer g.metrics.StartContractRequest("GetStatus")()
	result, err := g.multiCaller.SingleCall(ctx, rpcblock.Latest, g.contract.Call(methodStatus))
	if err != nil {
		return 0, fmt.Errorf("failed to fetch status: %w", err)
	}
	return gameTypes.GameStatusFromUint8(result.GetUint8(0))
}

func (g *TeeDisputeGameContractLatest) GetGameRange(ctx context.Context) (prestateBlock uint64, poststateBlock uint64, retErr error) {
	defer g.metrics.StartContractRequest("GetGameRange")()
	results, err := g.multiCaller.Call(ctx, rpcblock.Latest,
		g.contract.Call(methodStartingBlockNumber),
		g.contract.Call(methodL2SequenceNumber))
	if err != nil {
		retErr = fmt.Errorf("failed to retrieve game block range: %w", err)
		return
	}
	if len(results) != 2 {
		retErr = fmt.Errorf("expected 2 results but got %v", len(results))
		return
	}
	prestateBlock = getBlockNumber(results[0], 0)
	poststateBlock = getBlockNumber(results[1], 0)
	return
}

func (g *TeeDisputeGameContractLatest) GetResolvedAt(ctx context.Context, block rpcblock.Block) (time.Time, error) {
	defer g.metrics.StartContractRequest("GetResolvedAt")()
	result, err := g.multiCaller.SingleCall(ctx, block, g.contract.Call(methodResolvedAt))
	if err != nil {
		return time.Time{}, fmt.Errorf("failed to retrieve resolution time: %w", err)
	}
	resolvedAt := time.Unix(int64(result.GetUint64(0)), 0)
	return resolvedAt, nil
}

func (g *TeeDisputeGameContractLatest) CallResolve(ctx context.Context) (gameTypes.GameStatus, error) {
	defer g.metrics.StartContractRequest("CallResolve")()
	call := g.contract.Call(methodResolve)
	result, err := g.multiCaller.SingleCall(ctx, rpcblock.Latest, call)
	if err != nil {
		return gameTypes.GameStatusInProgress, fmt.Errorf("failed to call resolve: %w", err)
	}
	return gameTypes.GameStatusFromUint8(result.GetUint8(0))
}

func (g *TeeDisputeGameContractLatest) ResolveTx() (txmgr.TxCandidate, error) {
	call := g.contract.Call(methodResolve)
	return call.ToTxCandidate()
}

// GetChallengerMetadata reads the claimData struct and l2SequenceNumber.
func (g *TeeDisputeGameContractLatest) GetChallengerMetadata(ctx context.Context, block rpcblock.Block) (ChallengerMetadata, error) {
	defer g.metrics.StartContractRequest("GetChallengerMetadata")()
	results, err := g.multiCaller.Call(ctx, block,
		g.contract.Call(methodClaimData),
		g.contract.Call(methodL2SequenceNumber))
	if err != nil {
		return ChallengerMetadata{}, fmt.Errorf("failed to retrieve challenger metadata: %w", err)
	}
	if len(results) != 2 {
		return ChallengerMetadata{}, fmt.Errorf("expected 2 results but got %v", len(results))
	}
	data := g.decodeClaimData(results[0])
	l2SeqNum := getBlockNumber(results[1], 0)
	return ChallengerMetadata{
		ParentIndex:      data.ParentIndex,
		ProposalStatus:   data.Status,
		ProposedRoot:     data.Claim,
		L2SequenceNumber: l2SeqNum,
		Deadline:         time.Unix(int64(data.Deadline), 0),
	}, nil
}

// GetProveParams reads blockHash/stateHash from the current game and the parent game
// to build the parameters needed for a TEE proof request.
func (g *TeeDisputeGameContractLatest) GetProveParams(ctx context.Context, factory *DisputeGameFactoryContract) (TeeProveParams, error) {
	defer g.metrics.StartContractRequest("GetProveParams")()

	// Read end-side data from current game
	results, err := g.multiCaller.Call(ctx, rpcblock.Latest,
		g.contract.Call(methodL2SequenceNumber),
		g.contract.Call(methodBlockHash),
		g.contract.Call(methodStateHash),
		g.contract.Call(methodStartingBlockNumber),
		g.contract.Call(methodClaimData),
	)
	if err != nil {
		return TeeProveParams{}, fmt.Errorf("failed to retrieve prove params: %w", err)
	}
	if len(results) != 5 {
		return TeeProveParams{}, fmt.Errorf("expected 5 results but got %v", len(results))
	}

	endBlockNum := getBlockNumber(results[0], 0)
	endBlockHash := results[1].GetHash(0)
	endStateHash := results[2].GetHash(0)
	startBlockNum := getBlockNumber(results[3], 0)
	data := g.decodeClaimData(results[4])

	params := TeeProveParams{
		EndBlockHash:  endBlockHash,
		EndStateHash:  endStateHash,
		StartBlockNum: startBlockNum,
		EndBlockNum:   endBlockNum,
	}

	// Read start-side data from parent game
	parentIndex := data.ParentIndex
	if parentIndex == math.MaxUint32 {
		// Anchor-based games cannot be proved — the anchor root is a combined hash
		// keccak256(blockHash, stateHash) and individual hashes are not recoverable.
		// The game will resolve based on deadline expiry.
		return TeeProveParams{}, ErrAnchorGameUnprovable
	}

	// Get parent game address from factory
	parentGame, err := factory.GetGame(ctx, uint64(parentIndex), rpcblock.Latest)
	if err != nil {
		return TeeProveParams{}, fmt.Errorf("failed to get parent game at index %d: %w", parentIndex, err)
	}

	// Read parent game's blockHash() and stateHash() CWIA getters
	parentContract := batching.NewBoundContract(snapshots.LoadTeeDisputeGameABI(), parentGame.Proxy)
	parentResults, err := g.multiCaller.Call(ctx, rpcblock.Latest,
		parentContract.Call(methodBlockHash),
		parentContract.Call(methodStateHash),
	)
	if err != nil {
		return TeeProveParams{}, fmt.Errorf("failed to read parent game hashes: %w", err)
	}
	if len(parentResults) != 2 {
		return TeeProveParams{}, fmt.Errorf("expected 2 parent results but got %v", len(parentResults))
	}
	params.StartBlockHash = parentResults[0].GetHash(0)
	params.StartStateHash = parentResults[1].GetHash(0)

	return params, nil
}

// ProveTx constructs the prove(bytes) transaction.
// The from address is required for eth_call simulation because the contract checks msg.sender == proposer.
func (g *TeeDisputeGameContractLatest) ProveTx(ctx context.Context, proofBytes []byte, from common.Address) (txmgr.TxCandidate, error) {
	defer g.metrics.StartContractRequest("ProveTx")()
	call := g.contract.Call(methodProve, proofBytes)
	call.From = from
	_, err := g.multiCaller.SingleCall(ctx, rpcblock.Latest, call)
	if err != nil {
		return txmgr.TxCandidate{}, fmt.Errorf("%w: %w", ErrSimulationFailed, err)
	}
	return call.ToTxCandidate()
}

// GetProposer reads the proposer storage variable.
func (g *TeeDisputeGameContractLatest) GetProposer(ctx context.Context) (common.Address, error) {
	defer g.metrics.StartContractRequest("GetProposer")()
	result, err := g.multiCaller.SingleCall(ctx, rpcblock.Latest, g.contract.Call(methodProposer))
	if err != nil {
		return common.Address{}, fmt.Errorf("failed to fetch proposer: %w", err)
	}
	return result.GetAddress(0), nil
}

func (g *TeeDisputeGameContractLatest) GetCredit(ctx context.Context, recipient common.Address) (*big.Int, gameTypes.GameStatus, error) {
	defer g.metrics.StartContractRequest("GetCredit")()
	results, err := g.multiCaller.Call(ctx, rpcblock.Latest,
		g.contract.Call(methodCredit, recipient),
		g.contract.Call(methodStatus))
	if err != nil {
		return nil, gameTypes.GameStatusInProgress, err
	}
	if len(results) != 2 {
		return nil, gameTypes.GameStatusInProgress, fmt.Errorf("expected 2 results but got %v", len(results))
	}
	credit := results[0].GetBigInt(0)
	status, err := gameTypes.GameStatusFromUint8(results[1].GetUint8(0))
	if err != nil {
		return nil, gameTypes.GameStatusInProgress, fmt.Errorf("invalid game status %v: %w", status, err)
	}
	return credit, status, nil
}

func (g *TeeDisputeGameContractLatest) ClaimCreditTx(ctx context.Context, recipient common.Address) (txmgr.TxCandidate, error) {
	defer g.metrics.StartContractRequest("ClaimCredit")()
	call := g.contract.Call(methodClaimCredit, recipient)
	_, err := g.multiCaller.SingleCall(ctx, rpcblock.Latest, call)
	if err != nil {
		return txmgr.TxCandidate{}, fmt.Errorf("%w: %w", ErrSimulationFailed, err)
	}
	return call.ToTxCandidate()
}

func (g *TeeDisputeGameContractLatest) GetBondDistributionMode(ctx context.Context, block rpcblock.Block) (types.BondDistributionMode, error) {
	defer g.metrics.StartContractRequest("GetBondDistributionMode")()
	result, err := g.multiCaller.SingleCall(ctx, block, g.contract.Call(methodBondDistributionMode))
	if err != nil {
		return 0, fmt.Errorf("failed to fetch bond mode: %w", err)
	}
	return types.BondDistributionMode(result.GetUint8(0)), nil
}

func (g *TeeDisputeGameContractLatest) CloseGameTx(ctx context.Context) (txmgr.TxCandidate, error) {
	defer g.metrics.StartContractRequest("CloseGame")()
	call := g.contract.Call(methodCloseGame)
	_, err := g.multiCaller.SingleCall(ctx, rpcblock.Latest, call)
	if err != nil {
		return txmgr.TxCandidate{}, fmt.Errorf("%w: %w", ErrSimulationFailed, err)
	}
	return call.ToTxCandidate()
}

func (g *TeeDisputeGameContractLatest) decodeClaimData(result *batching.CallResult) claimData {
	parentIndex := result.GetUint32(0)
	counteredBy := result.GetAddress(1)
	prover := result.GetAddress(2)
	claim := result.GetHash(3)
	status := result.GetUint8(4)
	deadline := result.GetUint64(5)
	return claimData{
		ParentIndex: parentIndex,
		CounteredBy: counteredBy,
		Prover:      prover,
		Claim:       claim,
		Status:      ProposalStatus(status),
		Deadline:    deadline,
	}
}
