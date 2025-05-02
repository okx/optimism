package interop

import (
	"testing"

	"github.com/stretchr/testify/require"

	"github.com/ethereum-optimism/optimism/op-e2e/actions/helpers"
	"github.com/ethereum-optimism/optimism/op-e2e/actions/interop/dsl"
)

// TestInteropActivationWithInvalidMessages tests the behavior when invalid messages
// are attempted during the activation period.
func TestInteropActivationWithInvalidMessages(gt *testing.T) {
	t := helpers.NewDefaultTesting(gt)

	// Create a system with a future activation time (after 3 L1 blocks)
	system := dsl.NewInteropDSL(t, dsl.SetFutureInteropActivation())
	actors := system.Actors

	// Create user for chain B
	aliceB := system.CreateUser()

	// Deploy emitter contract on both chains
	emitter := system.DeployEmitterContracts()

	// PHASE 1: PRE-ACTIVATION - Create messages on both chains before interop activation
	t.Log("Phase 1: Creating pre-activation messages")
	
	// Create a valid message on chain A
	validMsgA := "valid message from chain A"
	messageA := dsl.NewMessage(system, actors.ChainA, emitter, validMsgA).Emit()
	
	// Attempt to execute the message from chain A on chain B (this should be filtered out pre-activation)
	execAction := system.InboxContract.Execute(aliceB, messageA.InitTx, dsl.WithPayload([]byte("invalid payload")))
	invalidTx := execAction(actors.ChainB)
	system.AddL2Block(actors.ChainB, dsl.WithL2BlockTransactions(
		func(chain *dsl.Chain) *dsl.GeneratedTransaction {
			return invalidTx
		},
	))

	// Check that the blocks exist at unsafe but haven't progressed to cross-unsafe
	statusA := actors.ChainA.Sequencer.SyncStatus()
	statusB := actors.ChainB.Sequencer.SyncStatus()
	require.Equal(t, uint64(1), statusA.UnsafeL2.Number, "Chain A unsafe head should be at block 1")
	require.Equal(t, uint64(1), statusB.UnsafeL2.Number, "Chain B unsafe head should be at block 1")
	require.Equal(t, uint64(0), statusA.CrossUnsafeL2.Number, "Chain A cross-unsafe should still be at genesis")
	require.Equal(t, uint64(0), statusB.CrossUnsafeL2.Number, "Chain B cross-unsafe should still be at genesis")

	// PHASE 2: ACTIVATION - Advance L1 blocks to reach activation time
	t.Log("Phase 2: Mining L1 blocks to reach activation time")
	for i := 0; i < 3; i++ {
		system.AdvanceL1()
	}

	// PHASE 3: POST-ACTIVATION - Test behavior with invalid messages after activation
	t.Log("Phase 3: Testing invalid messages post-activation")
	
	// Create a new valid message on chain A
	validMsgA2 := "valid message from chain A after activation"
	messageA2 := dsl.NewMessage(system, actors.ChainA, emitter, validMsgA2).Emit()
	
	// Attempt to execute the valid message with an invalid payload on chain B
	// This should be processed but fail validation
	execAction2 := system.InboxContract.Execute(aliceB, messageA2.InitTx, dsl.WithPayload([]byte("modified invalid payload")))
	invalidTx2 := execAction2(actors.ChainB)
	system.AddL2Block(actors.ChainB, dsl.WithL2BlockTransactions(
		func(chain *dsl.Chain) *dsl.GeneratedTransaction {
			return invalidTx2
		},
	))

	// Check that chains progress to cross-unsafe after activation
	statusA = actors.ChainA.Sequencer.SyncStatus()
	statusB = actors.ChainB.Sequencer.SyncStatus()
	require.Equal(t, uint64(2), statusA.UnsafeL2.Number, "Chain A unsafe head should be at block 2")
	require.Equal(t, uint64(2), statusB.UnsafeL2.Number, "Chain B unsafe head should be at block 2")
	require.Equal(t, uint64(2), statusA.CrossUnsafeL2.Number, "Chain A cross-unsafe should progress")
	require.Equal(t, uint64(2), statusB.CrossUnsafeL2.Number, "Chain B cross-unsafe should progress")

	// PHASE 4: Verify invalid transactions are properly detected and handled
	t.Log("Phase 4: Verifying invalid message handling")
	
	// Submit batch data to progress blocks to safe status
	system.SubmitBatchData()
	
	// The invalid transaction should be reorged out by the replacement block
	invalidTx.CheckNotIncluded()
	invalidTx2.CheckNotIncluded()
	
	// Verify that blocks progressed to local-safe and cross-safe properly
	statusA = actors.ChainA.Sequencer.SyncStatus()
	statusB = actors.ChainB.Sequencer.SyncStatus()
	require.Equal(t, uint64(2), statusA.UnsafeL2.Number)
	require.Equal(t, uint64(2), statusB.UnsafeL2.Number)
	require.Equal(t, uint64(2), statusA.LocalSafeL2.Number)
	require.Equal(t, uint64(2), statusB.LocalSafeL2.Number)
	require.Equal(t, uint64(2), statusA.SafeL2.Number)
	require.Equal(t, uint64(2), statusB.SafeL2.Number)
}