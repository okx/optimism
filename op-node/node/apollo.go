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

		// Register handlers for specific configuration items
		configManager.RegisterConfigHandler(flags.L1EpochPollIntervalFlag.Name, n.handleL1EpochPollIntervalChange)

		// Add more configuration handlers here as needed
		// configManager.RegisterConfigHandler("param1", n.handleParam1Change)
		// configManager.RegisterConfigHandler("param2", n.handleParam2Change)

		n.log.Info("Apollo configuration handlers registered")
	} else {
		n.log.Info("Apollo client disabled")
	}

	return nil
}

// Configuration handlers - these match the apollo.ConfigItemHandler signature

// handleL1EpochPollIntervalChange processes changes to the L1EpochPollInterval configuration
// This function parses the new interval value, updates the configuration, and restarts the polling subscriptions.
func (n *OpNode) handleL1EpochPollIntervalChange(value string) error {
	// Parse the duration string
	newInterval, err := time.ParseDuration(value)
	if err != nil {
		// Try parsing as seconds if direct duration parsing fails
		if seconds, parseErr := strconv.ParseFloat(value, 64); parseErr == nil {
			newInterval = time.Duration(seconds * float64(time.Second))
		} else {
			return fmt.Errorf("failed to parse L1 epoch poll interval: %w", err)
		}
	}

	// Validate the interval (must be positive or 0 to disable)
	if newInterval < 0 {
		return fmt.Errorf("L1 epoch poll interval cannot be negative: %v", newInterval)
	}

	// Check if the interval actually changed
	if n.cfg.L1EpochPollInterval == newInterval {
		n.log.Debug("L1 epoch poll interval unchanged, skipping update", "interval", newInterval)
		return nil
	}

	n.log.Info("Applying L1 epoch poll interval change",
		"old_interval", n.cfg.L1EpochPollInterval,
		"new_interval", newInterval)

	// Update the configuration value
	oldInterval := n.cfg.L1EpochPollInterval
	n.cfg.L1EpochPollInterval = newInterval

	// Restart the polling subscriptions to apply the new interval
	if err := n.restartL1EpochPolling(); err != nil {
		// Rollback the configuration change if restart fails
		n.cfg.L1EpochPollInterval = oldInterval
		return fmt.Errorf("failed to restart L1 epoch polling: %w", err)
	}

	n.log.Info("L1 epoch poll interval updated and applied successfully",
		"new_interval", newInterval)

	return nil
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
