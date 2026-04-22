package contracts

import (
	"context"
	"errors"
	"math"
	"math/big"
	"testing"
	"time"

	contractMetrics "github.com/ethereum-optimism/optimism/op-challenger/game/fault/contracts/metrics"
	faultTypes "github.com/ethereum-optimism/optimism/op-challenger/game/fault/types"
	gameTypes "github.com/ethereum-optimism/optimism/op-challenger/game/types"
	"github.com/ethereum-optimism/optimism/op-service/sources/batching"
	"github.com/ethereum-optimism/optimism/op-service/sources/batching/rpcblock"
	batchingTest "github.com/ethereum-optimism/optimism/op-service/sources/batching/test"
	"github.com/ethereum-optimism/optimism/op-service/txmgr"
	"github.com/ethereum-optimism/optimism/packages/contracts-bedrock/snapshots"
	"github.com/ethereum/go-ethereum/common"
	"github.com/stretchr/testify/require"
)

var (
	teeGameAddr = common.Address{0xee, 0xee, 0x01}
)

func TestTeeSimpleGetters(t *testing.T) {
	tests := []struct {
		methodAlias string
		method      string
		args        []interface{}
		result      interface{}
		expected    interface{}
		call        func(game TeeDisputeGameContract) (any, error)
	}{
		{
			methodAlias: "status",
			method:      methodStatus,
			result:      gameTypes.GameStatusChallengerWon,
			call: func(game TeeDisputeGameContract) (any, error) {
				return game.GetStatus(context.Background())
			},
		},
		{
			methodAlias: "l1Head",
			method:      methodL1Head,
			result:      common.Hash{0xdd, 0xbb},
			call: func(game TeeDisputeGameContract) (any, error) {
				return game.GetL1Head(context.Background())
			},
		},
		{
			methodAlias: "resolve",
			method:      methodResolve,
			result:      gameTypes.GameStatusInProgress,
			call: func(game TeeDisputeGameContract) (any, error) {
				return game.CallResolve(context.Background())
			},
		},
		{
			methodAlias: "resolvedAt",
			method:      methodResolvedAt,
			result:      uint64(240402),
			expected:    time.Unix(240402, 0),
			call: func(game TeeDisputeGameContract) (any, error) {
				return game.GetResolvedAt(context.Background(), rpcblock.Latest)
			},
		},
	}
	for _, test := range tests {
		test := test
		t.Run(test.methodAlias, func(t *testing.T) {
			stubRpc, game := setupTeeDisputeGameTest(t)
			stubRpc.SetResponse(teeGameAddr, test.method, rpcblock.Latest, nil, []interface{}{test.result})
			status, err := test.call(game)
			require.NoError(t, err)
			expected := test.expected
			if expected == nil {
				expected = test.result
			}
			require.Equal(t, expected, status)
		})
	}
}

func TestTeeGetMetadata(t *testing.T) {
	stubRpc, contract := setupTeeDisputeGameTest(t)
	expectedL1Head := common.Hash{0x0a, 0x0b}
	expectedL2BlockNumber := uint64(123)
	expectedRootClaim := common.Hash{0x01, 0x02}
	expectedStatus := gameTypes.GameStatusChallengerWon
	block := rpcblock.ByNumber(889)
	stubRpc.SetResponse(teeGameAddr, methodL1Head, block, nil, []interface{}{expectedL1Head})
	stubRpc.SetResponse(teeGameAddr, methodL2SequenceNumber, block, nil, []interface{}{new(big.Int).SetUint64(expectedL2BlockNumber)})
	stubRpc.SetResponse(teeGameAddr, methodRootClaim, block, nil, []interface{}{expectedRootClaim})
	stubRpc.SetResponse(teeGameAddr, methodStatus, block, nil, []interface{}{expectedStatus})
	actual, err := contract.GetMetadata(context.Background(), block)
	expected := GenericGameMetadata{
		L1Head:        expectedL1Head,
		L2SequenceNum: expectedL2BlockNumber,
		ProposedRoot:  expectedRootClaim,
		Status:        expectedStatus,
	}
	require.NoError(t, err)
	require.Equal(t, expected, actual)
}

func TestTeeGetGameRange(t *testing.T) {
	stubRpc, contract := setupTeeDisputeGameTest(t)
	expectedStart := uint64(65)
	expectedEnd := uint64(102)
	stubRpc.SetResponse(teeGameAddr, methodStartingBlockNumber, rpcblock.Latest, nil, []interface{}{new(big.Int).SetUint64(expectedStart)})
	stubRpc.SetResponse(teeGameAddr, methodL2SequenceNumber, rpcblock.Latest, nil, []interface{}{new(big.Int).SetUint64(expectedEnd)})
	start, end, err := contract.GetGameRange(context.Background())
	require.NoError(t, err)
	require.Equal(t, expectedStart, start)
	require.Equal(t, expectedEnd, end)
}

func TestTeeResolveTx(t *testing.T) {
	stubRpc, game := setupTeeDisputeGameTest(t)
	stubRpc.SetResponse(teeGameAddr, methodResolve, rpcblock.Latest, nil, nil)
	tx, err := game.ResolveTx()
	require.NoError(t, err)
	stubRpc.VerifyTxCandidate(tx)
}

func TestTeeGetChallengerMetadata(t *testing.T) {
	stubRpc, contract := setupTeeDisputeGameTest(t)
	expectedParentIndex := uint32(525)
	expectedProposalStatus := ProposalStatusChallengedAndValidProofProvided
	counteredBy := common.Address{0xad}
	prover := common.Address{0xac}
	expectedL2BlockNumber := uint64(123)
	expectedRootClaim := common.Hash{0x01, 0x02}
	expectedDeadline := time.Unix(84928429020, 0)
	block := rpcblock.ByNumber(889)
	stubRpc.SetResponse(teeGameAddr, methodClaimData, block, nil, []interface{}{
		expectedParentIndex, counteredBy, prover, expectedRootClaim, expectedProposalStatus, uint64(expectedDeadline.Unix()),
	})
	stubRpc.SetResponse(teeGameAddr, methodL2SequenceNumber, block, nil, []interface{}{new(big.Int).SetUint64(expectedL2BlockNumber)})
	actual, err := contract.GetChallengerMetadata(context.Background(), block)
	expected := ChallengerMetadata{
		ParentIndex:      expectedParentIndex,
		ProposalStatus:   expectedProposalStatus,
		ProposedRoot:     expectedRootClaim,
		L2SequenceNumber: expectedL2BlockNumber,
		Deadline:         expectedDeadline,
	}
	require.NoError(t, err)
	require.Equal(t, expected, actual)
}

func TestTeeProveTx(t *testing.T) {
	stubRpc, game := setupTeeDisputeGameTest(t)
	proofBytes := []byte{0xde, 0xad, 0xbe, 0xef}
	// prove() returns uint8 ProposalStatus
	stubRpc.SetResponse(teeGameAddr, methodProve, rpcblock.Latest, []interface{}{proofBytes}, []interface{}{uint8(ProposalStatusChallengedAndValidProofProvided)})
	tx, err := game.ProveTx(context.Background(), proofBytes, common.Address{0xaa})
	require.NoError(t, err)
	stubRpc.VerifyTxCandidate(tx)
}

func TestTeeGetProposer(t *testing.T) {
	stubRpc, game := setupTeeDisputeGameTest(t)
	expectedProposer := common.Address{0xaa, 0xbb, 0xcc}
	stubRpc.SetResponse(teeGameAddr, methodProposer, rpcblock.Latest, nil, []interface{}{expectedProposer})
	actual, err := game.GetProposer(context.Background())
	require.NoError(t, err)
	require.Equal(t, expectedProposer, actual)
}

func TestTeeGetCredit(t *testing.T) {
	stubRpc, game := setupTeeDisputeGameTest(t)
	addr := common.Address{0x01}
	expectedCredit := big.NewInt(4284)
	expectedStatus := gameTypes.GameStatusChallengerWon
	stubRpc.SetResponse(teeGameAddr, methodCredit, rpcblock.Latest, []interface{}{addr}, []interface{}{expectedCredit})
	stubRpc.SetResponse(teeGameAddr, methodStatus, rpcblock.Latest, nil, []interface{}{expectedStatus})

	actualCredit, actualStatus, err := game.GetCredit(context.Background(), addr)
	require.NoError(t, err)
	require.Equal(t, expectedCredit, actualCredit)
	require.Equal(t, expectedStatus, actualStatus)
}

func TestTeeClaimCreditTx(t *testing.T) {
	t.Run("Success", func(t *testing.T) {
		stubRpc, game := setupTeeDisputeGameTest(t)
		addr := common.Address{0xaa}
		stubRpc.SetResponse(teeGameAddr, methodClaimCredit, rpcblock.Latest, []interface{}{addr}, nil)
		tx, err := game.ClaimCreditTx(context.Background(), addr)
		require.NoError(t, err)
		stubRpc.VerifyTxCandidate(tx)
	})

	t.Run("SimulationFails", func(t *testing.T) {
		stubRpc, game := setupTeeDisputeGameTest(t)
		addr := common.Address{0xaa}
		stubRpc.SetError(teeGameAddr, methodClaimCredit, rpcblock.Latest, []interface{}{addr}, errors.New("still locked"))
		tx, err := game.ClaimCreditTx(context.Background(), addr)
		require.ErrorIs(t, err, ErrSimulationFailed)
		require.Equal(t, txmgr.TxCandidate{}, tx)
	})
}

func TestTeeGetBondDistributionMode(t *testing.T) {
	stubRpc, game := setupTeeDisputeGameTest(t)
	stubRpc.SetResponse(teeGameAddr, methodBondDistributionMode, rpcblock.Latest, nil, []interface{}{uint8(faultTypes.NormalDistributionMode)})

	mode, err := game.GetBondDistributionMode(context.Background(), rpcblock.Latest)
	require.NoError(t, err)
	require.Equal(t, faultTypes.NormalDistributionMode, mode)
}

func TestTeeCloseGameTx(t *testing.T) {
	t.Run("Success", func(t *testing.T) {
		stubRpc, game := setupTeeDisputeGameTest(t)
		stubRpc.SetResponse(teeGameAddr, methodCloseGame, rpcblock.Latest, nil, nil)
		tx, err := game.CloseGameTx(context.Background())
		require.NoError(t, err)
		stubRpc.VerifyTxCandidate(tx)
	})

	t.Run("SimulationFails", func(t *testing.T) {
		stubRpc, game := setupTeeDisputeGameTest(t)
		stubRpc.SetError(teeGameAddr, methodCloseGame, rpcblock.Latest, nil, errors.New("game not ready"))
		tx, err := game.CloseGameTx(context.Background())
		require.ErrorIs(t, err, ErrSimulationFailed)
		require.Equal(t, txmgr.TxCandidate{}, tx)
	})
}

func TestTeeGetProveParams(t *testing.T) {
	parentGameAddr := common.Address{0xbb, 0xcc, 0x01}
	factoryAddr := common.Address{0xff, 0xaa, 0x01}

	t.Run("WithParentGame", func(t *testing.T) {
		stubRpc, game := setupTeeDisputeGameTest(t)
		teeAbi := snapshots.LoadTeeDisputeGameABI()

		// Set up factory contract on the same stub RPC
		factoryAbi := snapshots.LoadDisputeGameFactoryABI()
		stubRpc.AddContract(factoryAddr, factoryAbi)
		caller := batching.NewMultiCaller(stubRpc, batching.DefaultBatchSize)
		factory := newDisputeGameFactoryContract(contractMetrics.NoopContractMetrics, factoryAddr, caller, factoryAbi, getGameArgsNoOp)

		// Current game responses
		expectedEndBlockNum := uint64(200)
		expectedEndBlockHash := common.Hash{0x11}
		expectedEndStateHash := common.Hash{0x22}
		expectedStartBlockNum := uint64(100)
		expectedVerifier := common.Address{0xee, 0xff}
		parentIndex := uint32(5)
		stubRpc.SetResponse(teeGameAddr, methodL2SequenceNumber, rpcblock.Latest, nil, []interface{}{new(big.Int).SetUint64(expectedEndBlockNum)})
		stubRpc.SetResponse(teeGameAddr, methodBlockHash, rpcblock.Latest, nil, []interface{}{expectedEndBlockHash})
		stubRpc.SetResponse(teeGameAddr, methodStateHash, rpcblock.Latest, nil, []interface{}{expectedEndStateHash})
		stubRpc.SetResponse(teeGameAddr, methodStartingBlockNumber, rpcblock.Latest, nil, []interface{}{new(big.Int).SetUint64(expectedStartBlockNum)})
		stubRpc.SetResponse(teeGameAddr, methodClaimData, rpcblock.Latest, nil, []interface{}{
			parentIndex, common.Address{}, common.Address{}, common.Hash{}, uint8(0), uint64(0),
		})
		stubRpc.SetResponse(teeGameAddr, methodTeeProofVerifier, rpcblock.Latest, nil, []interface{}{expectedVerifier})

		// Factory returns parent game address
		stubRpc.SetResponse(factoryAddr, methodGameAtIndex, rpcblock.Latest,
			[]interface{}{new(big.Int).SetUint64(uint64(parentIndex))},
			[]interface{}{uint32(gameTypes.TeeGameType), uint64(0), parentGameAddr})

		// Parent game responses
		expectedStartBlockHash := common.Hash{0x33}
		expectedStartStateHash := common.Hash{0x44}
		stubRpc.AddContract(parentGameAddr, teeAbi)
		stubRpc.SetResponse(parentGameAddr, methodBlockHash, rpcblock.Latest, nil, []interface{}{expectedStartBlockHash})
		stubRpc.SetResponse(parentGameAddr, methodStateHash, rpcblock.Latest, nil, []interface{}{expectedStartStateHash})

		params, err := game.GetProveParams(context.Background(), factory)
		require.NoError(t, err)
		require.Equal(t, TeeProveParams{
			StartBlockHash:   expectedStartBlockHash,
			StartStateHash:   expectedStartStateHash,
			EndBlockHash:     expectedEndBlockHash,
			EndStateHash:     expectedEndStateHash,
			StartBlockNum:    expectedStartBlockNum,
			EndBlockNum:      expectedEndBlockNum,
			TeeProofVerifier: expectedVerifier,
		}, params)
	})

	t.Run("AnchorGame", func(t *testing.T) {
		stubRpc, game := setupTeeDisputeGameTest(t)

		factoryAbi := snapshots.LoadDisputeGameFactoryABI()
		stubRpc.AddContract(factoryAddr, factoryAbi)
		caller := batching.NewMultiCaller(stubRpc, batching.DefaultBatchSize)
		factory := newDisputeGameFactoryContract(contractMetrics.NoopContractMetrics, factoryAddr, caller, factoryAbi, getGameArgsNoOp)

		// parentIndex = MaxUint32 means anchor game
		stubRpc.SetResponse(teeGameAddr, methodL2SequenceNumber, rpcblock.Latest, nil, []interface{}{new(big.Int).SetUint64(200)})
		stubRpc.SetResponse(teeGameAddr, methodBlockHash, rpcblock.Latest, nil, []interface{}{common.Hash{0x11}})
		stubRpc.SetResponse(teeGameAddr, methodStateHash, rpcblock.Latest, nil, []interface{}{common.Hash{0x22}})
		stubRpc.SetResponse(teeGameAddr, methodStartingBlockNumber, rpcblock.Latest, nil, []interface{}{new(big.Int).SetUint64(100)})
		stubRpc.SetResponse(teeGameAddr, methodClaimData, rpcblock.Latest, nil, []interface{}{
			uint32(math.MaxUint32), common.Address{}, common.Address{}, common.Hash{}, uint8(0), uint64(0),
		})
		stubRpc.SetResponse(teeGameAddr, methodTeeProofVerifier, rpcblock.Latest, nil, []interface{}{common.Address{0xee}})

		_, err := game.GetProveParams(context.Background(), factory)
		require.ErrorIs(t, err, ErrAnchorGameUnprovable)
	})
}

func setupTeeDisputeGameTest(t *testing.T) (*batchingTest.AbiBasedRpc, TeeDisputeGameContract) {
	teeAbi := snapshots.LoadTeeDisputeGameABI()
	stubRpc := batchingTest.NewAbiBasedRpc(t, teeGameAddr, teeAbi)
	caller := batching.NewMultiCaller(stubRpc, batching.DefaultBatchSize)
	game, err := NewTeeDisputeGameContract(contractMetrics.NoopContractMetrics, teeGameAddr, caller)
	require.NoError(t, err)
	return stubRpc, game
}
