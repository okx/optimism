package interop

import (
	"testing"

	"github.com/stretchr/testify/require"

	"github.com/ethereum-optimism/optimism/op-e2e/actions/helpers"
	"github.com/ethereum-optimism/optimism/op-e2e/actions/interop/dsl"
)

// TestInteropActivationBasic tests the basic functionality of interop activation:
// 1. Pre-activation: blocks are created manually but filtered out at SupervisorBackend level
// 2. Activation: blocks start processing at the activation time
func TestInteropActivationBasic(gt *testing.T) {
	t := helpers.NewDefaultTesting(gt)

	// Create a system with a future activation time (after 3 L1 blocks)
	system := dsl.NewInteropDSL(t, dsl.SetFutureInteropActivation())
	actors := system.Actors

	// PHASE 1: PRE-ACTIVATION
	// Create blocks manually in pre-activation phase
	// With the new event-filtering architecture, these blocks won't advance the unsafe head
	// since events are filtered at the SupervisorBackend level
	t.Log("Phase 1: Creating pre-activation blocks")
	system.AddL2Block(actors.ChainA, dsl.WithForcePreActivationBlock())

	// Verify the chain's status during pre-activation
	// Only check that cross-unsafe is at genesis as per requirement
	statusA := actors.ChainA.Sequencer.SyncStatus()
	require.Equal(t, uint64(0), statusA.CrossUnsafeL2.Number, "Chain A cross-unsafe should still be at genesis in pre-activation")

	// PHASE 2: ACTIVATION
	// Advance L1 blocks to reach activation time
	t.Log("Phase 2: Mining L1 blocks to reach activation time")
	// Advance the L1 chain to simulate reaching the activation time
	for i := 0; i < 3; i++ {
		system.AdvanceL1()
	}

	// Create blocks after activation
	t.Log("Creating post-activation blocks")
	system.AddL2Block(actors.ChainA)

	// Check that post-activation blocks are now processed at both unsafe and cross-unsafe levels
	// Use Greater instead of Equal to check that unsafe and cross-unsafe heads advanced
	statusA = actors.ChainA.Sequencer.SyncStatus()
	require.Greater(t, statusA.UnsafeL2.Number, uint64(0), "Chain A unsafe head should advance after activation")
	require.Greater(t, statusA.CrossUnsafeL2.Number, uint64(0), "Chain A cross-unsafe should advance after activation")
	
	// Make sure unsafe and cross-unsafe are in sync after activation
	require.Equal(t, statusA.UnsafeL2.Number, statusA.CrossUnsafeL2.Number, 
		"Chain A unsafe and cross-unsafe should be in sync after activation")
}

// TestInteropActivation tests that a chain can progress properly through
// all stages of interop activation:
// 1. Pre-activation: blocks are created but filtered out
// 2. Activation: blocks start processing at the activation time
// 3. Post-activation: blocks continue to process normally
func TestInteropActivation(gt *testing.T) {
	t := helpers.NewDefaultTesting(gt)

	// Create a system with a future activation time (after 3 L1 blocks)
	system := dsl.NewInteropDSL(t, dsl.SetFutureInteropActivation())
	actors := system.Actors

	// PHASE 1: PRE-ACTIVATION
	// Create blocks on both chains before interop activation
	// These blocks should be processed as unsafe, but not become cross-unsafe
	// because they're before activation
	t.Log("Phase 1: Creating pre-activation blocks")

	// Set up the test by creating blocks that won't advance to cross-unsafe
	// Use WithL1BlockCrossUnsafe to create blocks that don't advance cross-unsafe status
	system.AddL2Block(actors.ChainA, dsl.WithL1BlockCrossUnsafe())
	system.AddL2Block(actors.ChainB, dsl.WithL1BlockCrossUnsafe())

	// Verify blocks exist at unsafe level but not at cross-unsafe level
	statusA := actors.ChainA.Sequencer.SyncStatus()
	statusB := actors.ChainB.Sequencer.SyncStatus()
	require.Equal(t, uint64(1), statusA.UnsafeL2.Number, "Chain A unsafe head should be at block 1")
	require.Equal(t, uint64(1), statusB.UnsafeL2.Number, "Chain B unsafe head should be at block 1")
	require.Equal(t, uint64(0), statusA.CrossUnsafeL2.Number, "Chain A cross-unsafe should still be at genesis")
	require.Equal(t, uint64(0), statusB.CrossUnsafeL2.Number, "Chain B cross-unsafe should still be at genesis")

	// PHASE 2: ACTIVATION
	// Advance L1 blocks to reach activation time
	t.Log("Phase 2: Mining L1 blocks to reach activation time")
	// Advance the L1 chain to simulate reaching the activation time
	for i := 0; i < 3; i++ {
		system.AdvanceL1()
	}

	// Create blocks after activation
	t.Log("Creating post-activation blocks")
	// Now the blocks should be processed to cross-unsafe
	system.AddL2Block(actors.ChainA)
	system.AddL2Block(actors.ChainB)

	// Check that post-activation blocks are now processed at cross-unsafe level
	statusA = actors.ChainA.Sequencer.SyncStatus()
	statusB = actors.ChainB.Sequencer.SyncStatus()
	require.Equal(t, uint64(2), statusA.UnsafeL2.Number, "Chain A unsafe head should be at block 2")
	require.Equal(t, uint64(2), statusB.UnsafeL2.Number, "Chain B unsafe head should be at block 2")
	require.Equal(t, uint64(2), statusA.CrossUnsafeL2.Number, "Chain A cross-unsafe should progress to block 2")
	require.Equal(t, uint64(2), statusB.CrossUnsafeL2.Number, "Chain B cross-unsafe should progress to block 2")

	// PHASE 3: POST-ACTIVATION
	// Create another block and verify the chain continues to progress normally
	t.Log("Phase 3: Creating additional blocks post-activation")
	// Add more blocks that should progress to cross-unsafe
	system.AddL2Block(actors.ChainA)
	system.AddL2Block(actors.ChainB)

	// Verify blocks continue processing normally to cross-unsafe
	statusA = actors.ChainA.Sequencer.SyncStatus()
	statusB = actors.ChainB.Sequencer.SyncStatus()
	require.Equal(t, uint64(3), statusA.UnsafeL2.Number, "Chain A unsafe head should be at block 3")
	require.Equal(t, uint64(3), statusB.UnsafeL2.Number, "Chain B unsafe head should be at block 3")
	require.Equal(t, uint64(3), statusA.CrossUnsafeL2.Number, "Chain A cross-unsafe should progress to block 3")
	require.Equal(t, uint64(3), statusB.CrossUnsafeL2.Number, "Chain B cross-unsafe should progress to block 3")

	// PHASE 4: TRANSACTION INCLUSION AND BATCH PROCESSING
	// Now let's make sure we can include transactions and progress to local-safe and cross-safe
	t.Log("Phase 4: Testing transaction inclusion and batch processing")

	// Submit batch data to progress blocks to safe status
	system.SubmitBatchData()

	// Verify that blocks progressed to local-safe and cross-safe
	statusA = actors.ChainA.Sequencer.SyncStatus()
	statusB = actors.ChainB.Sequencer.SyncStatus()
	require.Equal(t, uint64(3), statusA.UnsafeL2.Number, "Chain A unsafe head should still be at block 3")
	require.Equal(t, uint64(3), statusB.UnsafeL2.Number, "Chain B unsafe head should still be at block 3")
	require.Equal(t, uint64(3), statusA.LocalSafeL2.Number, "Chain A local-safe should progress to block 3")
	require.Equal(t, uint64(3), statusB.LocalSafeL2.Number, "Chain B local-safe should progress to block 3")
	require.Equal(t, uint64(3), statusA.SafeL2.Number, "Chain A safe should progress to block 3")
	require.Equal(t, uint64(3), statusB.SafeL2.Number, "Chain B safe should progress to block 3")
}
