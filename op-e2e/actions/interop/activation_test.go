package interop

import (
	"testing"
	"time"

	"github.com/ethereum-optimism/optimism/op-e2e/actions/helpers"
	"github.com/ethereum-optimism/optimism/op-e2e/actions/interop/dsl"
	"github.com/stretchr/testify/require"
)

func TestActivationBasics(gt *testing.T) {
	t := helpers.NewDefaultTesting(gt)

	activationOffset := uint64(60)
	is := dsl.SetupInterop(t, dsl.SetInteropOffsetForAllL2s(activationOffset))
	actors := is.CreateActors()

	actors.PrepareChainState(t)

	chainA := actors.ChainA
	chainB := actors.ChainB

	depSet := is.DepSet
	now := uint64(time.Now().Unix())

	// Check future activation status (well after our activation time)
	expectedActivationTime := now + activationOffset
	futureTime := expectedActivationTime + 120
	canInitiateA, err := depSet.CanInitiateAt(chainA.ChainID, futureTime)
	require.NoError(t, err, "should get activation status for chain A")
	canInitiateB, err := depSet.CanInitiateAt(chainB.ChainID, futureTime)
	require.NoError(t, err, "should get activation status for chain B")

	// Verify both chains should have the same deferred activation time
	require.True(t, canInitiateA, "Chain A should be active at future time")
	require.True(t, canInitiateB, "Chain B should be active at future time")

	// Sync the supervisor, handle initial events emitted by the nodes
	chainA.Sequencer.SyncSupervisor(t)
	chainB.Sequencer.SyncSupervisor(t)

	// Create empty blocks on both chains
	chainA.Sequencer.ActL2EmptyBlock(t)
	chainB.Sequencer.ActL2EmptyBlock(t)

	// Sync the supervisor
	chainA.Sequencer.SyncSupervisor(t)
	chainB.Sequencer.SyncSupervisor(t)
	actors.Supervisor.ProcessFull(t)

	// Verify chain status - blocks should be in unsafe but not cross-unsafe
	// because they're before activation time
	statusA := chainA.Sequencer.SyncStatus()
	statusB := chainB.Sequencer.SyncStatus()
	require.Equal(t, uint64(1), statusA.UnsafeL2.Number)
	require.Equal(t, uint64(1), statusB.UnsafeL2.Number)
	require.Equal(t, uint64(0), statusA.CrossUnsafeL2.Number, "Chain A block should not be cross-unsafe before activation")
	require.Equal(t, uint64(0), statusB.CrossUnsafeL2.Number, "Chain B block should not be cross-unsafe before activation")

	chainA.Sequencer.ActL2EmptyBlock(t)
	chainB.Sequencer.ActL2EmptyBlock(t)
	chainA.Sequencer.ActL2EmptyBlock(t)
	chainB.Sequencer.ActL2EmptyBlock(t)

	chainA.Sequencer.SyncSupervisor(t)
	chainB.Sequencer.SyncSupervisor(t)
	actors.Supervisor.ProcessFull(t)

	// Apply changes to the nodes
	chainA.Sequencer.ActL2PipelineFull(t)
	chainB.Sequencer.ActL2PipelineFull(t)

	// The blocks should all be in unsafe state
	statusA = chainA.Sequencer.SyncStatus()
	statusB = chainB.Sequencer.SyncStatus()
	require.Equal(t, uint64(3), statusA.UnsafeL2.Number)
	require.Equal(t, uint64(3), statusB.UnsafeL2.Number)
}

func TestActivationMessagePassing(gt *testing.T) {
	t := helpers.NewDefaultTesting(gt)
	activationOffset := uint64(10)
	is := dsl.SetupInterop(t, dsl.SetInteropOffsetForAllL2s(activationOffset))
	actors := is.CreateActors()
	actors.PrepareChainState(t)

	chainA := actors.ChainA
	chainB := actors.ChainB

	// Get the activation time for each chain
	depSet := is.DepSet

	// Verify the activation time has not passed yet
	now := uint64(time.Now().Unix())
	_, err := depSet.CanInitiateAt(chainA.ChainID, now)
	require.NoError(t, err, "Should be able to check activation state")
	_, err = depSet.CanInitiateAt(chainB.ChainID, now)
	require.NoError(t, err, "Should be able to check activation state")

	// First sync the supervisor to establish baseline
	chainA.Sequencer.SyncSupervisor(t)
	chainB.Sequencer.SyncSupervisor(t)
	actors.Supervisor.ProcessFull(t)

	// Create empty blocks on both chains
	chainA.Sequencer.ActL2EmptyBlock(t)
	chainB.Sequencer.ActL2EmptyBlock(t)

	// Sync the supervisor and process events
	chainA.Sequencer.SyncSupervisor(t)
	chainB.Sequencer.SyncSupervisor(t)
	actors.Supervisor.ProcessFull(t)

	// Process all changes in the node pipelines
	chainA.Sequencer.ActL2PipelineFull(t)
	chainB.Sequencer.ActL2PipelineFull(t)

	// Create more empty blocks to simulate transactions before activation
	chainA.Sequencer.ActL2EmptyBlock(t)
	chainB.Sequencer.ActL2EmptyBlock(t)

	// Sync and process again
	chainA.Sequencer.SyncSupervisor(t)
	chainB.Sequencer.SyncSupervisor(t)
	actors.Supervisor.ProcessFull(t)
	chainA.Sequencer.ActL2PipelineFull(t)
	chainB.Sequencer.ActL2PipelineFull(t)

	// Check status again - these blocks shouldn't be cross-unsafe yet
	statusA := chainA.Sequencer.SyncStatus()
	statusB := chainB.Sequencer.SyncStatus()

	// canInitiateA, err = depSet.CanInitiateAt(chainA.ChainID, now)
	// canInitiateB, err = depSet.CanInitiateAt(chainB.ChainID, now)

	// Make the activation time to pass
	chainA.Sequencer.ActL2EmptyBlock(t)
	chainB.Sequencer.ActL2EmptyBlock(t)

	// Verify the activation time has passed
	now = uint64(time.Now().Unix())
	canInitiateA, err := depSet.CanInitiateAt(chainA.ChainID, now)
	require.NoError(t, err, "Should be able to check activation state")
	canInitiateB, err := depSet.CanInitiateAt(chainB.ChainID, now)
	require.NoError(t, err, "Should be able to check activation state")
	require.True(t, canInitiateA, "Chain A should be active after waiting")
	require.True(t, canInitiateB, "Chain B should be active after waiting")

	// Create post-activation blocks
	chainA.Sequencer.ActL2EmptyBlock(t)
	chainB.Sequencer.ActL2EmptyBlock(t)

	// Sync and process all events
	chainA.Sequencer.SyncSupervisor(t)
	chainB.Sequencer.SyncSupervisor(t)
	actors.Supervisor.ProcessFull(t)
	chainA.Sequencer.ActL2PipelineFull(t)
	chainB.Sequencer.ActL2PipelineFull(t)

	// We should have at least some cross-unsafe blocks now
	require.Greater(t, statusA.CrossUnsafeL2.Number, uint64(0), "Chain A should have cross-unsafe blocks after activation")
	require.Greater(t, statusB.CrossUnsafeL2.Number, uint64(0), "Chain B should have cross-unsafe blocks after activation")
}

func TestActivationInvalidMessages(gt *testing.T) {
	t := helpers.NewDefaultTesting(gt)

	system := dsl.NewInteropDSL(t, dsl.SetInteropOffsetForAllL2s(60)) // 60 seconds in the future
	actors := system.Actors

	// Process events to ensure state is consistent
	actors.ChainA.Sequencer.SyncSupervisor(t)
	actors.ChainB.Sequencer.SyncSupervisor(t)
	actors.Supervisor.ProcessFull(t)

	// Create a user and deploy the emitter contracts with explicit WithL1BlockCrossUnsafe
	// to prevent the blocks from being considered cross-unsafe during setup
	aliceB := system.CreateUser()

	// Deploy emitter contracts with explicit cross-unsafe control
	emitter := system.DeployEmitterContracts()

	// Save state after contract deployment
	// Both chains should be at block 1 (deploy contract block)
	postDeployStatusA := actors.ChainA.Sequencer.SyncStatus()
	postDeployStatusB := actors.ChainB.Sequencer.SyncStatus()
	require.Equal(t, uint64(1), postDeployStatusA.UnsafeL2.Number, "Chain A unsafe head should be at block 1")
	require.Equal(t, uint64(1), postDeployStatusB.UnsafeL2.Number, "Chain B unsafe head should be at block 1")

	// Create a message on chain A
	validMsgA := "valid message from chain A"

	// For pre-activation messages, explicitly mark them as not cross-unsafe
	// and force pre-activation mode to ensure blocks don't advance cross-unsafe
	system.AddL2Block(actors.ChainA,
		dsl.WithL1BlockCrossUnsafe(),
		dsl.WithForcePreActivationBlock(),
		dsl.WithL2BlockTransactions(
			func(chain *dsl.Chain) *dsl.GeneratedTransaction {
				action := emitter.EmitMessage(system.CreateUser(), validMsgA)
				return action(chain)
			},
		),
	)
	system.AddL2Block(actors.ChainB,
		dsl.WithL1BlockCrossUnsafe(),
		dsl.WithForcePreActivationBlock(),
		dsl.WithL2BlockTransactions(
			func(chain *dsl.Chain) *dsl.GeneratedTransaction {
				execAction := system.InboxContract.Execute(
					aliceB,
					nil,
					dsl.WithPendingMessage(emitter, actors.ChainA, 2, 0, validMsgA), // Block 2 (1 for deploy, 1 for message)
					dsl.WithPayload([]byte("invalid payload")),
				)
				return execAction(chain)
			},
		),
	)

	// Verify post-action block status - blocks should be unsafe at block 2
	statusA := actors.ChainA.Sequencer.SyncStatus()
	statusB := actors.ChainB.Sequencer.SyncStatus()
	require.Equal(t, uint64(2), statusA.UnsafeL2.Number, "Chain A unsafe head should be at block 2")
	require.Equal(t, uint64(2), statusB.UnsafeL2.Number, "Chain B unsafe head should be at block 2")

	// Save the pre-activation state for comparison after activation
	preActivationCrossUnsafeA := statusA.CrossUnsafeL2.Number
	preActivationCrossUnsafeB := statusB.CrossUnsafeL2.Number

	for i := 0; i < 3; i++ {
		system.AdvanceL1()
	}

	// Create a second message on chain A, post-activation
	validMsgA2 := "valid message from chain A after activation"
	system.AddL2Block(actors.ChainA, dsl.WithL2BlockTransactions(
		func(chain *dsl.Chain) *dsl.GeneratedTransaction {
			action := emitter.EmitMessage(system.CreateUser(), validMsgA2)
			return action(chain)
		},
	))

	// Create an invalid execution of the second message on chain B
	// Again using WithPendingMessage to avoid needing the receipt
	system.AddL2Block(actors.ChainB, dsl.WithL2BlockTransactions(
		func(chain *dsl.Chain) *dsl.GeneratedTransaction {
			execAction := system.InboxContract.Execute(
				aliceB,
				nil, // Use nil for the tx since we're using WithPendingMessage
				dsl.WithPendingMessage(emitter, actors.ChainA, 3, 0, validMsgA2), // Block 3 (1 for deploy, + 2 message blocks)
				dsl.WithPayload([]byte("modified invalid payload")),
			)
			return execAction(chain)
		},
	))

	// Verify post-activation block status - blocks should be unsafe
	statusA = actors.ChainA.Sequencer.SyncStatus()
	statusB = actors.ChainB.Sequencer.SyncStatus()
	require.Equal(t, uint64(3), statusA.UnsafeL2.Number, "Chain A unsafe head should be at block 3")
	require.Equal(t, uint64(3), statusB.UnsafeL2.Number, "Chain B unsafe head should be at block 3")

	require.Greater(t, statusA.CrossUnsafeL2.Number, preActivationCrossUnsafeA,
		"Chain A cross-unsafe should advance after activation")
	require.Greater(t, statusB.CrossUnsafeL2.Number, preActivationCrossUnsafeB,
		"Chain B cross-unsafe should advance after activation")

	// Submit batch data to make blocks safe
	system.SubmitBatchData()

	// Verify block status after batch submission
	statusA = actors.ChainA.Sequencer.SyncStatus()
	statusB = actors.ChainB.Sequencer.SyncStatus()
	require.Equal(t, uint64(3), statusA.UnsafeL2.Number)
	require.Equal(t, uint64(3), statusB.UnsafeL2.Number)
	require.Equal(t, uint64(3), statusA.LocalSafeL2.Number)
	require.Equal(t, uint64(3), statusB.LocalSafeL2.Number)
	require.Equal(t, uint64(3), statusA.SafeL2.Number)
	require.Equal(t, uint64(3), statusB.SafeL2.Number)
}
