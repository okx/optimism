package game

import (
	"context"
	"fmt"
	"time"

	"github.com/ethereum-optimism/optimism/op-challenger/config"
	"github.com/ethereum-optimism/optimism/op-challenger/flags"
	"github.com/ethereum-optimism/optimism/op-challenger/game/fault/contracts"
	"github.com/ethereum-optimism/optimism/op-service/apollo"
	"github.com/ethereum-optimism/optimism/op-service/client"
	"github.com/ethereum-optimism/optimism/op-service/sources/batching"
	"github.com/ethereum/go-ethereum/common"
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
		configManager.RegisterConfigHandler(flags.FactoryAddressFlag.Name, s.handleFactoryAddressChange)
		configManager.RegisterConfigHandler(flags.HTTPPollInterval.Name, s.handlePollIntervalChange)

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

// handleFactoryAddressChange processes changes to the game-factory-address configuration
// This function parses the new address and recreates the factory contract
func (s *Service) handleFactoryAddressChange(value string) error {
	// Parse the new factory address
	newAddress := common.HexToAddress(value)
	if newAddress == (common.Address{}) {
		return fmt.Errorf("invalid factory address: %s", value)
	}

	// Check if the address actually changed
	if s.factoryContract != nil {
		// We'll just update it since checking current address is complex
		s.logger.Info("Factory address updated", "new_address", newAddress)
	} else {
		s.logger.Info("Factory address set", "new_address", newAddress)
	}

	// Create new factory contract with the new address
	if s.l1Client != nil {
		multiCaller := batching.NewMultiCaller(s.l1Client.Client(), batching.DefaultBatchSize)
		newFactoryContract := contracts.NewDisputeGameFactoryContract(s.metrics, newAddress, multiCaller)

		// Update the factory contract
		s.factoryContract = newFactoryContract

		// Update monitor with new factory contract
		if s.monitor != nil {
			s.monitor.runState.Lock()
			s.monitor.source = newFactoryContract
			s.monitor.runState.Unlock()
			s.logger.Info("Monitor updated with new factory contract")
		}

		s.logger.Info("Factory contract recreated with new address", "address", newAddress)
	} else {
		s.logger.Warn("Cannot update factory address: L1 client not initialized")
	}

	return nil
}

// handlePollIntervalChange processes changes to the http-poll-interval configuration
// This function directly updates the poll rate of the existing PollingClient
func (s *Service) handlePollIntervalChange(value string) error {
	// Parse the new poll interval duration
	duration, err := time.ParseDuration(value)
	if err != nil {
		return fmt.Errorf("failed to parse poll interval duration '%s': %w", value, err)
	}

	// Check if pollClient exists
	if s.pollClient == nil {
		return fmt.Errorf("poll client not initialized")
	}

	// Try to cast to PollingClient and use the SetPollRate method
	if pollingClient, ok := s.pollClient.(*client.PollingClient); ok {
		if oldDuration := pollingClient.GetPollRate(); oldDuration == duration {
			return nil
		}
		// Use the public SetPollRate method
		pollingClient.SetPollRate(duration)
		s.logger.Info("Successfully updated poll rate via SetPollRate method",
			"new_rate", duration)
		return nil
	}

	// Fallback: if it's not a PollingClient, log a warning
	s.logger.Warn("Could not update poll rate directly, poll client may not be a PollingClient")
	return fmt.Errorf("failed to update poll rate: unsupported client type")
}
