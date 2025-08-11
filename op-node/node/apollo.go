package node

import (
	"context"
	"fmt"
	"strconv"
	"time"

	"github.com/ethereum-optimism/optimism/op-node/flags"
	"github.com/ethereum-optimism/optimism/op-node/rollup/finality"
	"github.com/ethereum-optimism/optimism/op-node/rollup/status"
	"github.com/ethereum-optimism/optimism/op-service/apollo"
	"github.com/ethereum-optimism/optimism/op-service/eth"
	"github.com/ethereum-optimism/optimism/op-service/sources"
)

const (
	SeqNamespace = "op-seq.txt"
	RpcNamespace = "op-rpc.txt"
)

func (n *OpNode) initApollo(ctx context.Context, cfg *Config) error {
	// Initialize Apollo client
	apolloClient, err := apollo.NewClient(cfg.Apollo, n.log)
	if err != nil {
		return fmt.Errorf("failed to initialize Apollo client: %w", err)
	}
	n.apolloClient = apolloClient

	if apolloClient.Enabled() {
		n.log.Info("Apollo client initialized and enabled")

		// Create a configuration manager for this namespace
		configManager := apolloClient.CreateConfigManager(cfg.Apollo.Namespace)

		configManager.RegisterConfigHandler(flags.L1HTTPPollInterval.Name, n.handleL1HTTPPollIntervalChange)
		configManager.RegisterConfigHandler(flags.L1EpochPollIntervalFlag.Name, n.handleL1EpochPollIntervalChange)
		switch cfg.Apollo.Namespace {
		case SeqNamespace:
			// Register handlers for specific configuration items
			configManager.RegisterConfigHandler(flags.SequencerMaxSafeLagFlag.Name, n.handleSequencerMaxSafeLagChange)
			configManager.RegisterConfigHandler(flags.SequencerRecoverMode.Name, n.handleSequencerRecoverModeChange)
			configManager.RegisterConfigHandler(flags.SequencerL1Confs.Name, n.handleSequencerL1ConfsChange)
		case RpcNamespace:
			configManager.RegisterConfigHandler(flags.VerifierL1Confs.Name, n.handleVerifierL1ConfsChange)
		}

		n.log.Info("Apollo configuration handlers registered")
	} else {
		n.log.Info("Apollo client disabled")
	}

	return nil
}

// handleL1EpochPollIntervalChange processes changes to the L1EpochPollInterval configuration
func (n *OpNode) handleL1EpochPollIntervalChange(value string) error {
	newInterval, err := n.parseDuration(value)
	if err != nil {
		return fmt.Errorf("failed to parse L1 epoch poll interval: %w", err)
	}

	if newInterval < 0 {
		return fmt.Errorf("L1 epoch poll interval cannot be negative: %v", newInterval)
	}

	if n.cfg.L1EpochPollInterval == newInterval {
		return nil
	}

	n.log.Info("L1 epoch poll interval updated", "old", n.cfg.L1EpochPollInterval, "new", newInterval)
	n.cfg.L1EpochPollInterval = newInterval

	// Restart polling subscriptions with new interval
	n.restartL1EpochPolling()
	return nil
}

// handleSequencerMaxSafeLagChange processes changes to the sequencer max safe lag configuration
func (n *OpNode) handleSequencerMaxSafeLagChange(value string) error {
	newLag, err := strconv.ParseUint(value, 10, 64)
	if err != nil {
		return fmt.Errorf("failed to parse sequencer max safe lag: %w", err)
	}

	if n.cfg.Driver.SequencerMaxSafeLag == newLag {
		return nil
	}

	if !n.cfg.Driver.SequencerEnabled {
		return nil
	}

	n.log.Info("Sequencer max safe lag updated", "old", n.cfg.Driver.SequencerMaxSafeLag, "new", newLag)
	n.cfg.Driver.SequencerMaxSafeLag = newLag

	// Apply the new setting to the running sequencer immediately
	if n.l2Driver != nil {
		ctx := context.Background()
		if err := n.l2Driver.SetMaxSafeLag(ctx, newLag); err != nil {
			n.log.Warn("Failed to call SetMaxSafeLag", "error", err)
		} else {
			n.log.Info("Successfully updated sequencer max safe lag")
		}
	}

	return nil
}

func (n *OpNode) handleSequencerRecoverModeChange(value string) error {
	newRecoverMode, err := strconv.ParseBool(value)
	if err != nil {
		return fmt.Errorf("failed to parse sequencer recover mode: %w", err)
	}

	if n.cfg.Driver.RecoverMode == newRecoverMode {
		return nil
	}

	if !n.cfg.Driver.SequencerEnabled {
		return nil
	}

	n.log.Info("Sequencer recover mode updated", "old", n.cfg.Driver.RecoverMode, "new", newRecoverMode)
	n.cfg.Driver.RecoverMode = newRecoverMode

	// Apply the new setting to the running driver if driver is active
	if n.l2Driver != nil {
		// Direct call to driver's public SetRecoverMode method
		ctx := context.Background()
		if err := n.l2Driver.SetRecoverMode(ctx, newRecoverMode); err != nil {
			n.log.Warn("Failed to call SetRecoverMode", "error", err)
		} else {
			n.log.Info("Successfully updated sequencer recover mode")
		}
	}

	return nil
}

// handleSequencerL1ConfsChange processes changes to the sequencer L1 confirmations configuration
func (n *OpNode) handleSequencerL1ConfsChange(value string) error {
	newConfs, err := strconv.ParseUint(value, 10, 64)
	if err != nil {
		return fmt.Errorf("failed to parse sequencer L1 confs: %w", err)
	}

	if n.cfg.Driver.SequencerConfDepth == newConfs {
		return nil
	}

	if !n.cfg.Driver.SequencerEnabled {
		n.log.Debug("Sequencer not enabled, skipping L1 confs update")
		return nil
	}

	n.log.Info("Sequencer L1 confs updated", "old", n.cfg.Driver.SequencerConfDepth, "new", newConfs)
	n.cfg.Driver.SequencerConfDepth = newConfs

	// Apply the new setting to the running driver if driver is active
	if n.l2Driver != nil {
		ctx := context.Background()
		if err := n.l2Driver.SetSequencerConfDepth(ctx, newConfs); err != nil {
			n.log.Warn("Failed to call SetSequencerConfDepth", "error", err)
		} else {
			n.log.Info("Successfully updated sequencer L1 confirmation depth")
		}
	}

	return nil
}

// parseDuration parses duration string with fallback to seconds
func (n *OpNode) parseDuration(value string) (time.Duration, error) {
	if duration, err := time.ParseDuration(value); err == nil {
		return duration, nil
	}

	if seconds, err := strconv.ParseFloat(value, 64); err == nil {
		return time.Duration(seconds * float64(time.Second)), nil
	}

	return 0, fmt.Errorf("invalid duration format: %s", value)
}

// restartL1EpochPolling restarts the L1 safe and finalized block polling subscriptions
// with the current configuration interval. This applies dynamic configuration changes.
// The function ensures atomic replacement: new subscriptions are created first, and old ones
// are only stopped after the new ones are successfully established.
func (n *OpNode) restartL1EpochPolling() error {
	// Only restart if we have the necessary components initialized
	if n.l1Source == nil {
		n.log.Debug("L1 polling components not ready, skipping restart")
		return nil
	}

	n.log.Debug("Restarting L1 epoch polling subscriptions")

	// Get current configuration interval
	pollInterval := n.cfg.L1EpochPollInterval

	// Store references to old subscriptions before creating new ones
	oldL1SafeSub := n.l1SafeSub
	oldL1FinalizedSub := n.l1FinalizedSub

	// First unregister the existing emitter to avoid panic, then re-register
	n.eventSys.Unregister("l1-signals")
	emitter := n.eventSys.Register("l1-signals", nil)
	onL1Safe := func(ctx context.Context, sig eth.L1BlockRef) {
		emitter.Emit(status.L1SafeEvent{L1Safe: sig})
	}
	onL1Finalized := func(ctx context.Context, sig eth.L1BlockRef) {
		emitter.Emit(finality.FinalizeL1Event{FinalizedL1: sig})
	}

	// Create new subscriptions first - this ensures continuity if creation fails
	n.log.Debug("Creating new L1 polling subscriptions", "interval", pollInterval)

	newL1SafeSub := eth.PollBlockChanges(n.log, n.l1Source, onL1Safe, eth.Safe,
		pollInterval, time.Second*10)
	newL1FinalizedSub := eth.PollBlockChanges(n.log, n.l1Source, onL1Finalized, eth.Finalized,
		pollInterval, time.Second*10)

	// Verify that new subscriptions were created successfully
	if newL1SafeSub == nil || newL1FinalizedSub == nil {
		// Clean up any partially created subscriptions
		if newL1SafeSub != nil {
			newL1SafeSub.Unsubscribe()
		}
		if newL1FinalizedSub != nil {
			newL1FinalizedSub.Unsubscribe()
		}
		return fmt.Errorf("failed to create new L1 polling subscriptions")
	}

	// Atomically replace the subscriptions
	n.l1SafeSub = newL1SafeSub
	n.l1FinalizedSub = newL1FinalizedSub

	// Now safely stop the old subscriptions
	if oldL1SafeSub != nil {
		n.log.Debug("Stopping old L1 safe subscription")
		oldL1SafeSub.Unsubscribe()
	}
	if oldL1FinalizedSub != nil {
		n.log.Debug("Stopping old L1 finalized subscription")
		oldL1FinalizedSub.Unsubscribe()
	}

	n.log.Info("L1 epoch polling subscriptions restarted successfully", "interval", pollInterval)
	return nil
}

// handleL1HTTPPollIntervalChange processes changes to the L1 HTTP poll interval configuration
// This function directly updates the poll rate of the existing PollingClient
func (n *OpNode) handleL1HTTPPollIntervalChange(value string) error {
	// Parse the new HTTP poll interval duration
	duration, err := time.ParseDuration(value)
	if err != nil {
		return fmt.Errorf("failed to parse L1 HTTP poll interval duration '%s': %w", value, err)
	}

	// Check if l1Source exists
	if n.l1Source == nil {
		return fmt.Errorf("L1 source not initialized")
	}

	// Try to cast to PollRateConfigurable interface
	if configurable, ok := n.l1Source.Client().(sources.PollRateConfigurable); ok {
		configurable.SetPollRate(duration)
		n.log.Info("Successfully updated L1 HTTP poll rate via PollRateConfigurable interface",
			"new_rate", duration,
			"client_type", fmt.Sprintf("%T", n.l1Source.Client()))
		return nil
	}

	// Final fallback: log error with actual type
	actualType := fmt.Sprintf("%T", n.l1Source.Client())
	n.log.Warn("Could not update L1 HTTP poll rate: client does not implement PollRateConfigurable interface",
		"actual_type", actualType)
	return fmt.Errorf("failed to update L1 HTTP poll rate: unsupported client type %s", actualType)
}

func (n *OpNode) handleVerifierL1ConfsChange(value string) error {
	newConfs, err := strconv.ParseUint(value, 10, 64)
	if err != nil {
		return fmt.Errorf("failed to parse verifier L1 confs: %w", err)
	}

	if n.cfg.Driver.VerifierConfDepth == newConfs {
		return nil
	}

	if n.cfg.Driver.SequencerEnabled {
		n.log.Warn("Verifier L1 confs cannot be updated when sequencer is enabled")
		return nil
	}

	n.log.Info("Verifier L1 confs updated", "old", n.cfg.Driver.VerifierConfDepth, "new", newConfs)
	n.cfg.Driver.VerifierConfDepth = newConfs

	// Apply the new setting to the running driver if driver is active
	if n.l2Driver != nil {
		ctx := context.Background()
		if err := n.l2Driver.SetVerifierConfDepth(ctx, newConfs); err != nil {
			n.log.Warn("Failed to call SetVerifierConfDepth", "error", err)
		} else {
			n.log.Info("Successfully updated verifier L1 confirmation depth")
		}
	}

	return nil
}
