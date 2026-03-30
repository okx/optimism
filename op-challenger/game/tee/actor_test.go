package tee

import (
	"context"
	"errors"
	"math"
	"testing"
	"time"

	"github.com/ethereum-optimism/optimism/op-challenger/game/fault/contracts"
	"github.com/ethereum-optimism/optimism/op-challenger/game/types"
	"github.com/ethereum-optimism/optimism/op-service/clock"
	"github.com/ethereum-optimism/optimism/op-service/sources/batching/rpcblock"
	"github.com/ethereum-optimism/optimism/op-service/testlog"
	"github.com/ethereum-optimism/optimism/op-service/txmgr"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/log"
	"github.com/stretchr/testify/require"
)

var (
	proveData   = "prove"
	resolveData = "resolve"
	l1Time      = time.Unix(9892842, 0)
)

type teeTestStubs struct {
	contract *stubContract
	sender   *stubTxSender
}

func TestActor(t *testing.T) {
	tests := []struct {
		name    string
		setup   func(t *testing.T, actor *Actor, stubs *teeTestStubs)
		prove   bool
		resolve bool
	}{
		{
			name: "UnchallengedNotExpired",
			setup: func(t *testing.T, actor *Actor, stubs *teeTestStubs) {
				stubs.contract.setDeadlineNotReached()
				// Default is Unchallenged — no action expected
			},
		},
		{
			name: "UnchallengedExpired",
			setup: func(t *testing.T, actor *Actor, stubs *teeTestStubs) {
				stubs.contract.setDeadlineExpired()
			},
			resolve: true,
		},
		{
			name: "ChallengedSubmitProve",
			setup: func(t *testing.T, actor *Actor, stubs *teeTestStubs) {
				stubs.contract.setDeadlineNotReached()
				stubs.contract.challenge(t)
				// Actor should start background prove — no tx sent this cycle
				// But since we use a real ProverClient stub, we need to simulate it differently:
				// The actor will call tryStartProve which calls proverClient.ProveAndWait in a goroutine.
				// Since we don't have a real HTTP server, we bypass by checking proveInFlight.
			},
			// No prove or resolve tx expected on this first Act cycle — goroutine starts but hasn't returned
		},
		{
			name: "ChallengedProveInFlight",
			setup: func(t *testing.T, actor *Actor, stubs *teeTestStubs) {
				stubs.contract.setDeadlineNotReached()
				stubs.contract.challenge(t)
				actor.proveInFlight = true // Simulate already in-flight
			},
			// No action — prove already running, no resolve conditions met
		},
		{
			name: "ChallengedProveResultReady",
			setup: func(t *testing.T, actor *Actor, stubs *teeTestStubs) {
				stubs.contract.setDeadlineNotReached()
				stubs.contract.challenge(t)
				// Pre-load result into channel to simulate background goroutine completion
				actor.proveResultCh <- proveResult{proofBytes: []byte{0xde, 0xad}, err: nil}
				actor.proveInFlight = true
			},
			prove: true,
		},
		{
			name: "ChallengedProveResultError",
			setup: func(t *testing.T, actor *Actor, stubs *teeTestStubs) {
				stubs.contract.setDeadlineNotReached()
				stubs.contract.challenge(t)
				// Pre-load error result — actor should set proveGivenUp=true
				actor.proveResultCh <- proveResult{err: errors.New("tee prover failed")}
				actor.proveInFlight = true
			},
			// No prove or resolve tx — error consumed, proveGivenUp=true, no retry
		},
		{
			name: "ChallengedExpiredNoProof",
			setup: func(t *testing.T, actor *Actor, stubs *teeTestStubs) {
				stubs.contract.setDeadlineExpired()
				stubs.contract.challenge(t)
			},
			resolve: true,
		},
		{
			name: "ChallengedAndProven",
			setup: func(t *testing.T, actor *Actor, stubs *teeTestStubs) {
				stubs.contract.setDeadlineNotReached()
				stubs.contract.challenge(t)
				stubs.contract.prove(t)
			},
			resolve: true,
		},
		{
			name: "UnchallengedAndProven",
			setup: func(t *testing.T, actor *Actor, stubs *teeTestStubs) {
				stubs.contract.setDeadlineNotReached()
				stubs.contract.prove(t)
			},
			resolve: true,
		},
		{
			name: "AlreadyResolved",
			setup: func(t *testing.T, actor *Actor, stubs *teeTestStubs) {
				stubs.contract.markResolved()
			},
		},
		{
			name: "ParentInProgress",
			setup: func(t *testing.T, actor *Actor, stubs *teeTestStubs) {
				stubs.contract.setDeadlineExpired()
				stubs.contract.setParentStatus(types.GameStatusInProgress)
			},
			// Cannot resolve because parent is still in progress
		},
		{
			name: "ParentChallengerWon",
			setup: func(t *testing.T, actor *Actor, stubs *teeTestStubs) {
				stubs.contract.setDeadlineNotReached()
				stubs.contract.setParentStatus(types.GameStatusChallengerWon)
			},
			resolve: true,
		},
		{
			name: "AnchorGameExpired",
			setup: func(t *testing.T, actor *Actor, stubs *teeTestStubs) {
				stubs.contract.setDeadlineExpired()
				stubs.contract.parentIndex = math.MaxUint32
			},
			resolve: true,
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			actor, stubs := setupTeeActorTest(t)
			if tt.setup != nil {
				tt.setup(t, actor, stubs)
			}
			err := actor.Act(context.Background())
			require.NoError(t, err)
			expectedTxCount := 0
			if tt.prove {
				require.Contains(t, stubs.sender.sentData, proveData)
				expectedTxCount++
			}
			if tt.resolve {
				require.Contains(t, stubs.sender.sentData, resolveData)
				expectedTxCount++
			}
			require.Len(t, stubs.sender.sentData, expectedTxCount)
		})
	}
}

func TestActorProveResultClearsInFlight(t *testing.T) {
	actor, stubs := setupTeeActorTest(t)
	stubs.contract.setDeadlineNotReached()
	stubs.contract.challenge(t)

	// Simulate background goroutine completing
	actor.proveInFlight = true
	actor.proveResultCh <- proveResult{proofBytes: []byte{0xaa}, err: nil}

	err := actor.Act(context.Background())
	require.NoError(t, err)
	require.False(t, actor.proveInFlight, "proveInFlight should be cleared after result consumed")
}

func TestActorProveErrorSetsGivenUp(t *testing.T) {
	actor, stubs := setupTeeActorTest(t)
	stubs.contract.setDeadlineNotReached()
	stubs.contract.challenge(t)

	actor.proveInFlight = true
	actor.proveResultCh <- proveResult{err: errors.New("prover down")}

	err := actor.Act(context.Background())
	require.NoError(t, err)
	require.False(t, actor.proveInFlight, "proveInFlight should be cleared after error consumed")
	require.True(t, actor.proveGivenUp, "proveGivenUp should be set after error")
}

func TestActorProveTimeoutSetsGivenUp(t *testing.T) {
	actor, stubs := setupTeeActorTest(t)
	stubs.contract.setDeadlineNotReached()
	stubs.contract.challenge(t)

	actor.proveInFlight = true
	actor.proveResultCh <- proveResult{err: context.DeadlineExceeded}

	err := actor.Act(context.Background())
	require.NoError(t, err)
	require.True(t, actor.proveGivenUp, "proveGivenUp should be set after timeout")
}

func TestActorProveGivenUpSkipsProve(t *testing.T) {
	actor, stubs := setupTeeActorTest(t)
	stubs.contract.setDeadlineNotReached()
	stubs.contract.challenge(t)
	actor.proveGivenUp = true // Already given up

	err := actor.Act(context.Background())
	require.NoError(t, err)
	require.False(t, actor.proveInFlight, "should not start prove goroutine when given up")
	require.Len(t, stubs.sender.sentData, 0, "no tx should be sent")
}

func setupTeeActorTest(t *testing.T) (*Actor, *teeTestStubs) {
	logger := testlog.Logger(t, log.LvlInfo)
	l1Clock := clock.NewDeterministicClock(l1Time)
	contract := &stubContract{
		parentIndex:  482,
		parentStatus: types.GameStatusDefenderWon,
	}
	contract.setDeadlineNotReached()
	txSender := &stubTxSender{}

	// Provide a dummy ProverClient so goroutines don't panic on nil.
	// Tests that check prove results use the proveResultCh channel directly.
	dummyProver := NewProverClient("http://127.0.0.1:1", 10*time.Millisecond, logger)
	actor := &Actor{
		logger:             logger,
		l1Clock:            l1Clock,
		l1ChainID:          1,
		contract:           contract,
		proverClient:       dummyProver,
		txSender:           txSender,
		gameStatusProvider: contract,
		factory:            nil,
		proveTimeout:       1 * time.Hour,
		serviceCtx:         context.Background(),
		proveResultCh:      make(chan proveResult, 1),
	}
	return actor, &teeTestStubs{
		contract: contract,
		sender:   txSender,
	}
}

// --- Stubs ---

type stubContract struct {
	parentIndex    uint32
	parentStatus   types.GameStatus
	proposalStatus contracts.ProposalStatus
	deadline       time.Time
	proveParams    contracts.TeeProveParams
}

func (s *stubContract) Addr() common.Address {
	return common.Address{0x67, 0x67, 0x67}
}

func (s *stubContract) challenge(t *testing.T) {
	require.Equal(t, contracts.ProposalStatusUnchallenged, s.proposalStatus, "game not in challengable state")
	s.proposalStatus = contracts.ProposalStatusChallenged
}

func (s *stubContract) prove(t *testing.T) {
	if s.proposalStatus == contracts.ProposalStatusUnchallenged {
		s.proposalStatus = contracts.ProposalStatusUnchallengedAndValidProofProvided
		return
	}
	require.Equal(t, contracts.ProposalStatusChallenged, s.proposalStatus, "game not in provable state")
	s.proposalStatus = contracts.ProposalStatusChallengedAndValidProofProvided
}

func (s *stubContract) setDeadlineExpired() {
	s.deadline = l1Time.Add(-1 * time.Second)
}

func (s *stubContract) setDeadlineNotReached() {
	s.deadline = l1Time.Add(1 * time.Second)
}

func (s *stubContract) markResolved() {
	s.proposalStatus = contracts.ProposalStatusResolved
}

func (s *stubContract) setParentStatus(status types.GameStatus) {
	s.parentStatus = status
}

func (s *stubContract) GetGameStatus(_ context.Context, idx uint64) (types.GameStatus, error) {
	if idx != uint64(s.parentIndex) {
		return 0, errors.New("unexpected parent index")
	}
	if idx == math.MaxUint32 {
		return 0, errors.New("execution reverted")
	}
	return s.parentStatus, nil
}

func (s *stubContract) GetChallengerMetadata(_ context.Context, _ rpcblock.Block) (contracts.ChallengerMetadata, error) {
	return contracts.ChallengerMetadata{
		ParentIndex:    s.parentIndex,
		ProposalStatus: s.proposalStatus,
		Deadline:       s.deadline,
	}, nil
}

func (s *stubContract) GetProveParams(_ context.Context, _ *contracts.DisputeGameFactoryContract) (contracts.TeeProveParams, error) {
	return s.proveParams, nil
}

func (s *stubContract) ProveTx(_ context.Context, _ []byte, _ common.Address) (txmgr.TxCandidate, error) {
	return txmgr.TxCandidate{
		TxData: []byte(proveData),
	}, nil
}

func (s *stubContract) ResolveTx() (txmgr.TxCandidate, error) {
	return txmgr.TxCandidate{
		TxData: []byte(resolveData),
	}, nil
}

type stubTxSender struct {
	sentData []string
	sendErr  error
}

func (s *stubTxSender) From() common.Address {
	return common.Address{0xaa}
}

func (s *stubTxSender) SendAndWaitSimple(_ string, candidates ...txmgr.TxCandidate) error {
	for _, candidate := range candidates {
		s.sentData = append(s.sentData, string(candidate.TxData))
	}
	if s.sendErr != nil {
		return s.sendErr
	}
	return nil
}
