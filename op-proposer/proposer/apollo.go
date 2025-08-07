package proposer

import (
	"context"
	"fmt"
	"strconv"
	"time"

	"github.com/ethereum-optimism/optimism/op-proposer/flags"
	"github.com/ethereum-optimism/optimism/op-service/apollo"
)

// initApollo initializes the Apollo client for dynamic configuration management
func (ps *ProposerService) initApollo(ctx context.Context, cfg *CLIConfig) error {
	// Initialize Apollo client
	apolloClient, err := apollo.NewClient(cfg.Apollo, ps.Log)
	if err != nil {
		return fmt.Errorf("failed to initialize Apollo client: %w", err)
	}
	ps.apolloClient = apolloClient

	if apolloClient.Enabled() {
		ps.Log.Info("Apollo client initialized and enabled")

		// Create a configuration manager for this namespace
		configManager := apolloClient.CreateConfigManager(cfg.Apollo.Namespace)

		// Register handlers for specific configuration items
		configManager.RegisterConfigHandler(flags.PollIntervalFlag.Name, ps.handlePollIntervalChange)

		ps.Log.Info("Apollo configuration handlers registered")
	} else {
		ps.Log.Info("Apollo client disabled")
	}

	return nil
}

// handlePollIntervalChange processes changes to the poll-interval configuration
// This function parses the new interval value and updates the proposer configuration
func (ps *ProposerService) handlePollIntervalChange(value string) error {
	// Parse the duration string
	newInterval, err := time.ParseDuration(value)
	if err != nil {
		// Try parsing as seconds if direct duration parsing fails
		if seconds, parseErr := strconv.ParseFloat(value, 64); parseErr == nil {
			newInterval = time.Duration(seconds * float64(time.Second))
		} else {
			return fmt.Errorf("failed to parse poll-interval value: %w", err)
		}
	}

	// Validate the interval (must be positive)
	if newInterval <= 0 {
		return fmt.Errorf("poll-interval must be positive: %v", newInterval)
	}

	// Check if the interval actually changed
	if ps.PollInterval == newInterval {
		return nil
	}

	ps.Log.Info("Poll-interval updated", "old_interval", ps.PollInterval, "new_interval", newInterval)

	// Update the configuration value
	ps.PollInterval = newInterval

	// Update driver configuration and recreate driver with new context
	if ps.driver != nil {
		ps.driver.Cfg.PollInterval = newInterval

		if ps.driver.running.Load() {
			ps.Log.Info("Recreating driver to apply new poll interval")

			// Stop current driver
			if err := ps.driver.StopL2OutputSubmittingIfRunning(); err != nil {
				ps.Log.Error("Failed to stop driver", "error", err)
				return fmt.Errorf("failed to stop driver: %w", err)
			}

			// Recreate driver with updated config
			if err := ps.initDriver(); err != nil {
				ps.Log.Error("Failed to recreate driver", "error", err)
				return fmt.Errorf("failed to recreate driver: %w", err)
			}

			// Start new driver
			if err := ps.driver.StartL2OutputSubmitting(); err != nil {
				ps.Log.Error("Failed to start new driver", "error", err)
				return fmt.Errorf("failed to start new driver: %w", err)
			}

			ps.Log.Info("Driver recreated successfully with new poll interval")
		}
	}

	return nil
}
