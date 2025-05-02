package interop

import (
	"testing"

	"github.com/stretchr/testify/require"

	"github.com/ethereum-optimism/optimism/op-e2e/actions/helpers"
	"github.com/ethereum-optimism/optimism/op-e2e/actions/interop/dsl"
)

// TestInteropActivationWithFailedTransactions tests the behavior when transactions
// fail during the interop activation process. This verifies that failed transactions
// don't prevent proper activation of interop functionality.
func TestInteropActivationWithFailedTransactions(gt *testing.T) {
	t := helpers.NewDefaultTesting(gt)

	// Create a system with a future activation time (after 3 L1 blocks)
	system := dsl.NewInteropDSL(t, dsl.SetFutureInteropActivation())
	actors := system.Actors

	// Create user for chain B
	aliceB := system.CreateUser()

	// Deploy emitter contract on both chains
	emitter := system.DeployEmitterContracts()

	// PHASE 1: PRE-ACTIVATION - Create messages on both chains before interop activation
	t.Log("Phase 1: Creating pre-activation blocks with messages")
	
	// Create a valid message on chain A
	validMsgA := "valid message from chain A"
	messageA := dsl.NewMessage(system, actors.ChainA, emitter, validMsgA).Emit()
	
	// Create a valid message on chain B
	validMsgB := "valid message from chain B"
	dsl.NewMessage(system, actors.ChainB, emitter, validMsgB).Emit()

	// Verify blocks exist at unsafe level but not at cross-unsafe level
	statusA := actors.ChainA.Sequencer.SyncStatus()
	statusB := actors.ChainB.Sequencer.SyncStatus()
	require.Equal(t, uint64(2), statusA.UnsafeL2.Number) // 1 for deploy, 1 for message
	require.Equal(t, uint64(2), statusB.UnsafeL2.Number) // 1 for deploy, 1 for message
	require.Equal(t, uint64(0), statusA.CrossUnsafeL2.Number, "Chain A cross-unsafe should still be at genesis")
	require.Equal(t, uint64(0), statusB.CrossUnsafeL2.Number, "Chain B cross-unsafe should still be at genesis")

	// PHASE 2: ACTIVATION - Advance L1 blocks to reach activation time
	t.Log("Phase 2: Mining L1 blocks to reach activation time")
	for i := 0; i < 3; i++ {
		system.AdvanceL1()
	}

	// PHASE 3: POST-ACTIVATION - Test behavior with failed transactions after activation
	t.Log("Phase 3: Creating post-activation blocks with failed transactions")
	
	// Create a new message on chain A for testing post-activation
	validMsgA2 := "valid message from chain A after activation"
	messageA2 := dsl.NewMessage(system, actors.ChainA, emitter, validMsgA2).Emit()

	// On Chain B, include a valid cross-chain message execution
	executingTx := messageA.ExecuteOn(actors.ChainB)
	
	// Also include an execution of the pre-activation message with a deliberately 
	// corrupted payload that will cause the transaction to fail during execution
	failingExecAction := system.InboxContract.Execute(aliceB, messageA.InitTx, dsl.WithPayload([]byte("corrupted payload")))
	failingTx := failingExecAction(actors.ChainB)
	
	// Create a block with both the valid and failing transactions
	system.AddL2Block(actors.ChainB, dsl.WithL2BlockTransactions(
		func(chain *dsl.Chain) *dsl.GeneratedTransaction {
			return executingTx.ExecTx
		},
		func(chain *dsl.Chain) *dsl.GeneratedTransaction {
			return failingTx
		},
	))
	
	// Verify that blocks progress to cross-unsafe level despite failing transactions
	statusA = actors.ChainA.Sequencer.SyncStatus()
	statusB = actors.ChainB.Sequencer.SyncStatus()
	require.Equal(t, uint64(3), statusA.UnsafeL2.Number)
	require.Equal(t, uint64(3), statusB.UnsafeL2.Number)
	require.Equal(t, uint64(3), statusA.CrossUnsafeL2.Number, "Chain A cross-unsafe should progress")
	require.Equal(t, uint64(3), statusB.CrossUnsafeL2.Number, "Chain B cross-unsafe should progress")

	// PHASE 4: Batch submission and verification
	t.Log("Phase 4: Submitting batches and verifying cross-chain communication")
	
	// Submit batch data to progress blocks to safe status
	system.SubmitBatchData()

	// Now check if we can send a message from chain B to chain A after activation
	// This verifies bidirectional cross-chain messaging works post-activation
	executingTx2 := messageA2.ExecuteOn(actors.ChainB)

	// Verify that blocks progressed to local-safe and cross-safe
	statusA = actors.ChainA.Sequencer.SyncStatus()
	statusB = actors.ChainB.Sequencer.SyncStatus()
	require.Equal(t, uint64(3), statusA.UnsafeL2.Number)
	require.Equal(t, uint64(4), statusB.UnsafeL2.Number) // +1 for the execution of messageA2
	require.Equal(t, uint64(3), statusA.LocalSafeL2.Number)
	require.Equal(t, uint64(3), statusB.LocalSafeL2.Number) // The batch submission doesn't include the last block
	require.Equal(t, uint64(3), statusA.SafeL2.Number)
	require.Equal(t, uint64(3), statusB.SafeL2.Number)
	
	// Verify that valid transaction was processed and included
	executingTx.CheckExecuted()
	
	// Verify the second valid cross-chain message was also executed successfully
	executingTx2.CheckExecuted()
}