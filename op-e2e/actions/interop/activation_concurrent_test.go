package interop

import (
	"testing"

	"github.com/stretchr/testify/require"

	"github.com/ethereum-optimism/optimism/op-e2e/actions/helpers"
	"github.com/ethereum-optimism/optimism/op-e2e/actions/interop/dsl"
)

// TestInteropActivationWithConcurrentL2Blocks tests the behavior when multiple L2 chains
// produce blocks concurrently right at the activation boundary. This verifies that
// all blocks are properly processed according to the activation rules.
func TestInteropActivationWithConcurrentL2Blocks(gt *testing.T) {
	t := helpers.NewDefaultTesting(gt)

	// Create a system with activation after exactly 2 L1 blocks
	system := dsl.NewInteropDSL(t, dsl.SetInteropOffsetForAllL2s(2))
	actors := system.Actors

	// Create users for both chains
	aliceA := system.CreateUser()
	aliceB := system.CreateUser()

	// Deploy emitter contract on both chains
	emitter := system.DeployEmitterContracts()

	// PHASE 1: PRE-ACTIVATION - Create blocks and messages before activation
	t.Log("Phase 1: Creating pre-activation blocks and messages")
	
	// Create messages on both chains
	msgA1 := "message A1 (pre-activation)"
	messageA1 := dsl.NewMessage(system, actors.ChainA, emitter, msgA1).Emit()
	
	msgB1 := "message B1 (pre-activation)"
	messageB1 := dsl.NewMessage(system, actors.ChainB, emitter, msgB1).Emit()
	
	// Verify blocks exist at unsafe level but not at cross-unsafe level
	statusA := actors.ChainA.Sequencer.SyncStatus()
	statusB := actors.ChainB.Sequencer.SyncStatus()
	require.Equal(t, uint64(2), statusA.UnsafeL2.Number) // 1 for deploy, 1 for message
	require.Equal(t, uint64(2), statusB.UnsafeL2.Number) // 1 for deploy, 1 for message
	require.Equal(t, uint64(0), statusA.CrossUnsafeL2.Number, "Chain A cross-unsafe should still be at genesis")
	require.Equal(t, uint64(0), statusB.CrossUnsafeL2.Number, "Chain B cross-unsafe should still be at genesis")

	// PHASE 2: APPROACHING ACTIVATION - Mine first L1 block, still pre-activation
	t.Log("Phase 2: Approaching activation boundary")
	system.AdvanceL1()
	
	// Create more blocks that are still pre-activation
	msgA2 := "message A2 (approaching activation)"
	messageA2 := dsl.NewMessage(system, actors.ChainA, emitter, msgA2).Emit()
	
	msgB2 := "message B2 (approaching activation)"
	messageB2 := dsl.NewMessage(system, actors.ChainB, emitter, msgB2).Emit()
	
	// These blocks should still be at unsafe but not cross-unsafe
	statusA = actors.ChainA.Sequencer.SyncStatus()
	statusB = actors.ChainB.Sequencer.SyncStatus()
	require.Equal(t, uint64(3), statusA.UnsafeL2.Number)
	require.Equal(t, uint64(3), statusB.UnsafeL2.Number)
	require.Equal(t, uint64(0), statusA.CrossUnsafeL2.Number, "Chain A cross-unsafe should still be at genesis")
	require.Equal(t, uint64(0), statusB.CrossUnsafeL2.Number, "Chain B cross-unsafe should still be at genesis")

	// PHASE 3: ACTIVATION - Mine one more L1 block to reach the activation boundary
	t.Log("Phase 3: Reaching activation and creating concurrent blocks")
	system.AdvanceL1()
	
	// Both chains create blocks exactly at the activation boundary in parallel
	actors.ChainA.Sequencer.ActL2StartBlock(t)
	actors.ChainB.Sequencer.ActL2StartBlock(t)

	// Prepare messages to be included in the blocks created at activation time
	msgA3 := "message A3 (at activation)"
	genTxA := emitter.EmitMessage(aliceA, msgA3)(actors.ChainA)
	genTxA.Include() // Include without checking for successful execution
	
	msgB3 := "message B3 (at activation)"
	genTxB := emitter.EmitMessage(aliceB, msgB3)(actors.ChainB)
	genTxB.Include() // Include without checking for successful execution
	
	// Finish the blocks on both chains
	actors.ChainA.Sequencer.ActL2EndBlock(t)
	actors.ChainB.Sequencer.ActL2EndBlock(t)

	// Sync the supervisor and process events
	actors.ChainA.Sequencer.SyncSupervisor(t)
	actors.ChainB.Sequencer.SyncSupervisor(t)
	actors.Supervisor.ProcessFull(t)
	actors.ChainA.Sequencer.ActL2PipelineFull(t)
	actors.ChainB.Sequencer.ActL2PipelineFull(t)

	// Verify blocks are now processed at the cross-unsafe level since we're at activation
	statusA = actors.ChainA.Sequencer.SyncStatus()
	statusB = actors.ChainB.Sequencer.SyncStatus()
	require.Equal(t, uint64(4), statusA.UnsafeL2.Number)
	require.Equal(t, uint64(4), statusB.UnsafeL2.Number)
	require.Equal(t, uint64(4), statusA.CrossUnsafeL2.Number, "Chain A cross-unsafe should progress after activation")
	require.Equal(t, uint64(4), statusB.CrossUnsafeL2.Number, "Chain B cross-unsafe should progress after activation")

	// PHASE 4: CROSS-CHAIN MESSAGING - Test cross-chain message execution after activation
	t.Log("Phase 4: Testing cross-chain messaging post-activation")
	
	// Execute messages from chain B on chain A
	executingTxB1 := messageB1.ExecuteOn(actors.ChainA)
	executingTxB2 := messageB2.ExecuteOn(actors.ChainA)
	
	// Execute messages from chain A on chain B
	executingTxA1 := messageA1.ExecuteOn(actors.ChainB)
	executingTxA2 := messageA2.ExecuteOn(actors.ChainB)
	
	// Submit batch data to progress blocks to safe status
	system.SubmitBatchData()
	
	// Verify message executions were successful for both chains
	executingTxA1.CheckExecuted()
	executingTxA2.CheckExecuted()
	executingTxB1.CheckExecuted()
	executingTxB2.CheckExecuted()
	
	// Check that all status levels have advanced for both chains
	statusA = actors.ChainA.Sequencer.SyncStatus()
	statusB = actors.ChainB.Sequencer.SyncStatus()
	require.Equal(t, uint64(6), statusA.UnsafeL2.Number) // +2 for message executions
	require.Equal(t, uint64(6), statusB.UnsafeL2.Number) // +2 for message executions
	require.Greater(t, statusA.LocalSafeL2.Number, uint64(4), "Chain A local-safe should have progressed")
	require.Greater(t, statusB.LocalSafeL2.Number, uint64(4), "Chain B local-safe should have progressed")
	require.Greater(t, statusA.SafeL2.Number, uint64(4), "Chain A safe should have progressed")
	require.Greater(t, statusB.SafeL2.Number, uint64(4), "Chain B safe should have progressed")
}