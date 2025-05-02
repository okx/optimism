package syncnode

import (
	"context"
	"testing"
	"time"

	"github.com/ethereum-optimism/optimism/op-node/rollup/event"
	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/ethereum-optimism/optimism/op-service/testlog"
	"github.com/ethereum-optimism/optimism/op-supervisor/supervisor/backend/superevents"
	"github.com/ethereum-optimism/optimism/op-supervisor/supervisor/types"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/log"
	"github.com/stretchr/testify/require"
)

func TestEventResponse(t *testing.T) {
	chainID := eth.ChainIDFromUInt64(1)
	logger := testlog.Logger(t, log.LvlInfo)
	syncCtrl := &mockSyncControl{}
	backend := &mockBackend{}

	ex := event.NewGlobalSynchronous(context.Background())
	eventSys := event.NewSystem(logger, ex)

	mon := &eventMonitor{}
	eventSys.Register("monitor", mon, event.DefaultRegisterOpts())

	node := NewManagedNode(logger, chainID, syncCtrl, backend, false)
	eventSys.Register("node", node, event.DefaultRegisterOpts())

	emitter := eventSys.Register("test", nil, event.DefaultRegisterOpts())

	crossUnsafe := 0
	crossSafe := 0
	finalized := 0

	nodeExhausted := 0

	// the node will call UpdateCrossUnsafe when a cross-unsafe event is received from the database
	syncCtrl.updateCrossUnsafeFn = func(ctx context.Context, id eth.BlockID) error {
		crossUnsafe++
		return nil
	}
	// the node will call UpdateCrossSafe when a cross-safe event is received from the database
	syncCtrl.updateCrossSafeFn = func(ctx context.Context, derived eth.BlockID, source eth.BlockID) error {
		crossSafe++
		return nil
	}
	// the node will call UpdateFinalized when a finalized event is received from the database
	syncCtrl.updateFinalizedFn = func(ctx context.Context, id eth.BlockID) error {
		finalized++
		return nil
	}

	// the node will call ProvideL1 when the node is exhausted and needs a new L1 derivation source
	syncCtrl.provideL1Fn = func(ctx context.Context, nextL1 eth.BlockRef) error {
		nodeExhausted++
		return nil
	}

	node.Start()

	// send events and continue to do so until at least one of each type has been received
	require.Eventually(t, func() bool {
		// send in one event of each type
		emitter.Emit(superevents.CrossUnsafeUpdateEvent{ChainID: chainID})
		emitter.Emit(superevents.CrossSafeUpdateEvent{ChainID: chainID})
		emitter.Emit(superevents.FinalizedL2UpdateEvent{ChainID: chainID})

		syncCtrl.subscribeEvents.Send(&types.ManagedEvent{
			UnsafeBlock: &eth.BlockRef{Number: 1}})
		syncCtrl.subscribeEvents.Send(&types.ManagedEvent{
			DerivationUpdate: &types.DerivedBlockRefPair{Source: eth.BlockRef{Number: 1}, Derived: eth.BlockRef{Number: 2}}})
		syncCtrl.subscribeEvents.Send(&types.ManagedEvent{
			ExhaustL1: &types.DerivedBlockRefPair{Source: eth.BlockRef{Number: 1}, Derived: eth.BlockRef{Number: 2}}})
		syncCtrl.subscribeEvents.Send(&types.ManagedEvent{
			DerivationOriginUpdate: &eth.BlockRef{Number: 1}})

		require.NoError(t, ex.Drain())

		return crossUnsafe >= 1 &&
			crossSafe >= 1 &&
			finalized >= 1 &&
			mon.receivedLocalUnsafe >= 1 &&
			mon.localDerived >= 1 &&
			nodeExhausted >= 1 &&
			mon.localDerivedOriginUpdate >= 1
	}, 4*time.Second, 250*time.Millisecond)
}

func TestResetRequesterIntegration(t *testing.T) {
	chainID := eth.ChainIDFromUInt64(1)
	logger := testlog.Logger(t, log.LvlDebug)
	syncCtrl := &mockSyncControl{}
	backend := &mockBackend{}

	// Set up the event system
	ex := event.NewGlobalSynchronous(context.Background())
	eventSys := event.NewSystem(logger, ex)
	node := NewManagedNode(logger, chainID, syncCtrl, backend, true) // Set to true for synchronous operation
	eventSys.Register("node", node, event.DefaultRegisterOpts())

	// Track RequestReset calls via our special interface
	var requestResetCalled int
	var lastResetL1BlockNumber uint64

	// Create a new implementation that satisfies both SyncControl and ResetRequester interfaces
	resetRequesterImpl := struct {
		SyncControl
		ResetRequester
	}{
		SyncControl: syncCtrl,
		ResetRequester: &mockResetRequester{
			requestResetFn: func(ctx context.Context, l1BlockNumber uint64) error {
				requestResetCalled++
				lastResetL1BlockNumber = l1BlockNumber
				return nil
			},
		},
	}

	// Assign the composited implementation to node's Node field
	node.Node = resetRequesterImpl

	// Set up the backend to return a source block
	backend.safeDerivedAtFn = func(ctx context.Context, chainID eth.ChainID, source eth.BlockID) (derived eth.BlockID, err error) {
		return eth.BlockID{Number: 42}, nil
	}

	// Set up the local safe pair to have a known source
	backend.localSafeFn = func(ctx context.Context, chainID eth.ChainID) (types.DerivedIDPair, error) {
		return types.DerivedIDPair{
			Derived: eth.BlockID{Number: 100},
			Source:  eth.BlockID{Number: 50},
		}, nil
	}

	// Set up the block reference lookup
	syncCtrl.blockRefByNumFn = func(ctx context.Context, number uint64) (eth.BlockRef, error) {
		return eth.BlockRef{Number: number, Hash: common.Hash{0xaa}}, nil
	}

	// mock: control whether the blocks appear valid or not
	var pivot uint64
	backend.isLocalSafeFn = func(ctx context.Context, chainID eth.ChainID, id eth.BlockID) error {
		if id.Number > uint64(pivot) {
			return types.ErrConflict
		}
		return nil
	}

	// Mock FindSealedBlock so we can initialize the reset range
	backend.findSealedBlockFn = func(ctx context.Context, chainID eth.ChainID, num uint64) (eth.BlockID, error) {
		return eth.BlockID{Number: 1, Hash: common.Hash{0xaa}}, nil
	}

	// Test the reset functionality with a target block
	targetBlock := eth.BlockID{Number: 100, Hash: common.Hash{0xaa}}
	node.resetTracker.beginBisectionReset(targetBlock)

	// The reset tracker should call RequestReset with the L1 block number (50)
	require.Equal(t, 1, requestResetCalled, "RequestReset should be called once")
	require.Equal(t, uint64(50), lastResetL1BlockNumber, "Reset should target L1 block 50")

	// Test with a different target block that requires SafeDerivedAt
	requestResetCalled = 0 // Reset the counter
	targetBlock = eth.BlockID{Number: 200, Hash: common.Hash{0xbb}}
	node.resetTracker.beginBisectionReset(targetBlock)

	// The reset tracker should now use SafeDerivedAt to find the L1 block (42)
	require.Equal(t, 1, requestResetCalled, "RequestReset should be called once")
	require.Equal(t, uint64(42), lastResetL1BlockNumber, "Reset should target L1 block 42")
}

// mockResetRequester is a mock implementation of the ResetRequester interface for testing
type mockResetRequester struct {
	requestResetFn func(ctx context.Context, l1BlockNumber uint64) error
}

// RequestReset implements the ResetRequester interface
func (m *mockResetRequester) RequestReset(ctx context.Context, l1BlockNumber uint64) error {
	if m.requestResetFn != nil {
		return m.requestResetFn(ctx, l1BlockNumber)
	}
	return nil
}
