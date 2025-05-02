package interop

import (
	"testing"

	"github.com/stretchr/testify/require"

	"github.com/ethereum-optimism/optimism/op-e2e/actions/helpers"
	"github.com/ethereum-optimism/optimism/op-e2e/actions/interop/dsl"
)

// TestInteropActivationWithMissedL1Blocks tests the behavior when some L1 blocks
// are missed during the activation period. This simulates L1 sync issues where
// the supervisor or nodes may miss some blocks during activation.
func TestInteropActivationWithMissedL1Blocks(gt *testing.T) {
	t := helpers.NewDefaultTesting(gt)

	// Create a system with a future activation time (after 5 L1 blocks)
	// Use a longer activation period to ensure we have more blocks to miss
	system := dsl.NewInteropDSL(t, dsl.SetInteropOffsetForAllL2s(5))
	actors := system.Actors

	// PHASE 1: PRE-ACTIVATION - Create blocks on both chains before interop activation
	t.Log("Phase 1: Creating pre-activation blocks")
	system.AddL2Block(actors.ChainA, dsl.WithL1BlockCrossUnsafe())
	system.AddL2Block(actors.ChainB, dsl.WithL1BlockCrossUnsafe())
	
	// Verify blocks exist at unsafe level but not at cross-unsafe level
	statusA := actors.ChainA.Sequencer.SyncStatus()
	statusB := actors.ChainB.Sequencer.SyncStatus()
	require.Equal(t, uint64(1), statusA.UnsafeL2.Number, "Chain A unsafe head should be at block 1")
	require.Equal(t, uint64(1), statusB.UnsafeL2.Number, "Chain B unsafe head should be at block 1")
	require.Equal(t, uint64(0), statusA.CrossUnsafeL2.Number, "Chain A cross-unsafe should still be at genesis")
	require.Equal(t, uint64(0), statusB.CrossUnsafeL2.Number, "Chain B cross-unsafe should still be at genesis")

	// PHASE 2: ACTIVATION WITH MISSED BLOCKS
	// Mine L1 blocks but don't signal some of them to simulate missed blocks
	t.Log("Phase 2: Creating L1 blocks with some blocks missed by the supervisor")
	
	// Mine the first L1 block and signal it
	actors.L1Miner.ActL1StartBlock(12)(t)
	actors.L1Miner.ActL1EndBlock(t)
	actors.Supervisor.SignalLatestL1(t)
	
	// Mine the second block but don't signal it (simulating a missed block)
	actors.L1Miner.ActL1StartBlock(12)(t)
	actors.L1Miner.ActL1EndBlock(t)
	// No SignalLatestL1 here to simulate missed block
	
	// Mine the third block but don't signal it (another missed block)
	actors.L1Miner.ActL1StartBlock(12)(t)
	actors.L1Miner.ActL1EndBlock(t)
	// No SignalLatestL1 here to simulate missed block
	
	// Mine and signal the remaining blocks to reach activation
	for i := 0; i < 3; i++ {
		actors.L1Miner.ActL1StartBlock(12)(t)
		actors.L1Miner.ActL1EndBlock(t)
		actors.Supervisor.SignalLatestL1(t)
	}

	// PHASE 3: POST-ACTIVATION - Create blocks and verify activation still happened
	t.Log("Phase 3: Creating post-activation blocks")
	
	// Catch up with the latest L1 blocks (this should process all blocks including the missed ones)
	actors.ChainA.Sequencer.ActL1HeadSignal(t)
	actors.ChainB.Sequencer.ActL1HeadSignal(t)
	
	// Add post-activation blocks
	system.AddL2Block(actors.ChainA)
	system.AddL2Block(actors.ChainB)
	
	// Check that post-activation blocks are processed at cross-unsafe level
	// The activation should happen even with missed blocks, as the node should sync up
	statusA = actors.ChainA.Sequencer.SyncStatus()
	statusB = actors.ChainB.Sequencer.SyncStatus()
	require.Equal(t, uint64(2), statusA.UnsafeL2.Number, "Chain A unsafe head should be at block 2")
	require.Equal(t, uint64(2), statusB.UnsafeL2.Number, "Chain B unsafe head should be at block 2")
	require.Equal(t, uint64(2), statusA.CrossUnsafeL2.Number, "Chain A cross-unsafe should progress to block 2")
	require.Equal(t, uint64(2), statusB.CrossUnsafeL2.Number, "Chain B cross-unsafe should progress to block 2")

	// PHASE 4: Batch submission and synchronization
	t.Log("Phase 4: Submitting batches and checking synchronization")
	
	// Submit batch data to progress blocks to safe status
	system.SubmitBatchData()
	
	// Verify that blocks progressed to local-safe and cross-safe
	statusA = actors.ChainA.Sequencer.SyncStatus()
	statusB = actors.ChainB.Sequencer.SyncStatus()
	require.Equal(t, uint64(2), statusA.UnsafeL2.Number)
	require.Equal(t, uint64(2), statusB.UnsafeL2.Number)
	require.Equal(t, uint64(2), statusA.LocalSafeL2.Number)
	require.Equal(t, uint64(2), statusB.LocalSafeL2.Number)
	require.Equal(t, uint64(2), statusA.SafeL2.Number)
	require.Equal(t, uint64(2), statusB.SafeL2.Number)
}