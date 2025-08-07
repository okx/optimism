package game

import (
	"context"
	"fmt"
	"time"

	"github.com/ethereum-optimism/optimism/op-challenger/config"
	"github.com/ethereum-optimism/optimism/op-challenger/flags"
	"github.com/ethereum-optimism/optimism/op-service/apollo"
)

// initApollo initializes the Apollo client for dynamic configuration management
func (s *Service) initApollo(ctx context.Context, cfg *config.Config) error {
	// Initialize Apollo client
	apolloClient, err := apollo.NewClient(cfg.Apollo, s.logger)
	if err != nil {
		return fmt.Errorf("failed to initialize Apollo client: %w", err)
	}
	s.apolloClient = apolloClient

	if apolloClient.Enabled() {
		s.logger.Info("Apollo client initialized and enabled")

		// Create a configuration manager for this namespace
		configManager := apolloClient.CreateConfigManager(cfg.Apollo.Namespace)

		// Register handlers for specific configuration items
		configManager.RegisterConfigHandler(flags.GameWindowFlag.Name, s.handleGameWindowChange)

		s.logger.Info("Apollo configuration handlers registered")
	} else {
		s.logger.Info("Apollo client disabled")
	}

	return nil
}

// handleGameWindowChange processes changes to the game-window configuration
// This function parses the new window value and updates the monitor's game window
func (s *Service) handleGameWindowChange(value string) error {
	// Parse the new game window duration
	duration, err := time.ParseDuration(value)
	if err != nil {
		return fmt.Errorf("failed to parse game window duration '%s': %w", value, err)
	}

	// Validate the duration is reasonable (between 1 second and 30 days)
	if duration < time.Second {
		return fmt.Errorf("game window duration too short: %v (minimum 1s)", duration)
	}
	if duration > 30*24*time.Hour {
		return fmt.Errorf("game window duration too long: %v (maximum 30 days)", duration)
	}

	// Update the monitor's game window if it exists
	if s.monitor != nil {
		// Use the runState mutex to safely update the game window
		s.monitor.runState.Lock()
		oldWindow := s.monitor.gameWindow
		s.monitor.gameWindow = duration
		s.monitor.runState.Unlock()

		s.logger.Info("Updated game window via Apollo configuration",
			"oldWindow", oldWindow,
			"newWindow", duration,
			"newWindowSeconds", duration.Seconds())
	} else {
		s.logger.Warn("Cannot update game window: monitor not initialized")
	}

	return nil
}
