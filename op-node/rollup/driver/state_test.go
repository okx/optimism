package driver

import (
	"context"
	"testing"
	"time"

	"github.com/stretchr/testify/mock"
	"github.com/stretchr/testify/require"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/log"
	"github.com/ethereum-optimism/optimism/op-node/rollup/engine"
	"github.com/ethereum-optimism/optimism/op-node/rollup/event"
	"github.com/ethereum-optimism/optimism/op-node/rollup/status"
	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/ethereum-optimism/optimism/op-service/testlog"
)

// mockL1Chain is a mock implementation of the L1Chain interface for testing
type mockL1Chain struct {
	mock.Mock
}

func (m *mockL1Chain) L1BlockRefByNumber(ctx context.Context, number uint64) (eth.L1BlockRef, error) {
	args := m.Called(ctx, number)
	return args.Get(0).(eth.L1BlockRef), args.Error(1)
}

func (m *mockL1Chain) L1BlockRefByHash(ctx context.Context, hash common.Hash) (eth.L1BlockRef, error) {
	args := m.Called(ctx, hash)
	return args.Get(0).(eth.L1BlockRef), args.Error(1)
}

// mockEventEmitter is a mock implementation of the event.Emitter interface for testing
type mockEventEmitter struct {
	mock.Mock
}

func (m *mockEventEmitter) Emit(event event.Event) {
	m.Called(event)
}

// TestDriverResetToL1 tests the ResetToL1 method of the Driver
func TestDriverResetToL1(t *testing.T) {
	// Create mock objects
	mockL1 := new(mockL1Chain)
	mockEmitter := new(mockEventEmitter)
	
	// Setup expected L1 block
	l1BlockNum := uint64(1000)
	l1Block := eth.L1BlockRef{
		Hash:       common.HexToHash("0x1234"),
		Number:     l1BlockNum,
		ParentHash: common.HexToHash("0x5678"),
		Time:       uint64(time.Now().Unix()),
	}
	
	// Setup expected behavior
	mockL1.On("L1BlockRefByNumber", mock.Anything, l1BlockNum).Return(l1Block, nil)
	mockEmitter.On("Emit", mock.MatchedBy(func(ev event.Event) bool {
		_, ok := ev.(engine.ResetEngineRequestEvent)
		return ok
	})).Return()
	mockEmitter.On("Emit", mock.MatchedBy(func(ev event.Event) bool {
		unsafeEv, ok := ev.(status.L1UnsafeEvent)
		return ok && unsafeEv.L1Unsafe == l1Block
	})).Return()
	mockEmitter.On("Emit", mock.MatchedBy(func(ev event.Event) bool {
		_, ok := ev.(StepReqEvent)
		return ok
	})).Return()
	
	// Create a test driver
	driver := &Driver{
		L1:            mockL1,
		emitter:       mockEmitter,
		log:           testlog.Logger(t, log.LvlInfo),
		driverCtx:     context.Background(),
		driverCancel:  func() {},
		forceReset:    make(chan chan struct{}, 1),
	}
	
	// Start a background goroutine to handle the force reset request
	go func() {
		select {
		case respCh := <-driver.forceReset:
			close(respCh)
		case <-time.After(2 * time.Second):
			t.Fail()
		}
	}()
	
	// Call the method under test
	err := driver.ResetToL1(context.Background(), l1BlockNum)
	
	// Assert expectations
	require.NoError(t, err)
	mockL1.AssertExpectations(t)
	mockEmitter.AssertExpectations(t)
}