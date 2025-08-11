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
		configManager.RegisterConfigHandler(flags.ProposalIntervalFlag.Name, ps.handleProposalIntervalChange)
		configManager.RegisterConfigHandler(flags.AllowNonFinalizedFlag.Name, ps.handleAllowNonFinalizedChange)
		configManager.RegisterConfigHandler(flags.DisputeGameTypeFlag.Name, ps.handleDisputeGameTypeChange)

		ps.Log.Info("Apollo configuration handlers registered")
	} else {
		ps.Log.Info("Apollo client disabled")
	}

	return nil
}

// handleProposalIntervalChange processes changes to the proposal-interval configuration
// This function parses the new interval value and updates the proposer configuration
func (ps *ProposerService) handleProposalIntervalChange(value string) error {
	// Parse the duration string
	newInterval, err := time.ParseDuration(value)
	if err != nil {
		// Try parsing as seconds if direct duration parsing fails
		if seconds, parseErr := strconv.ParseFloat(value, 64); parseErr == nil {
			newInterval = time.Duration(seconds * float64(time.Second))
		} else {
			return fmt.Errorf("failed to parse proposal-interval value: %w", err)
		}
	}

	// Validate the interval (must be positive)
	if newInterval <= 0 {
		return fmt.Errorf("proposal-interval must be positive: %v", newInterval)
	}

	// Check if the interval actually changed
	if ps.ProposalInterval == newInterval {
		return nil
	}

	ps.Log.Info("Proposal-interval updated", "old_interval", ps.ProposalInterval, "new_interval", newInterval)

	// Update the configuration values
	ps.ProposalInterval = newInterval
	if ps.driver != nil {
		ps.driver.Cfg.ProposalInterval = newInterval
	}

	return nil
}

func (ps *ProposerService) handleAllowNonFinalizedChange(value string) error {
	allowNonFinalized, err := strconv.ParseBool(value)
	if err != nil {
		return fmt.Errorf("failed to parse allow-non-finalized value: %w", err)
	}

	if ps.AllowNonFinalized == allowNonFinalized {
		return nil
	}

	ps.Log.Info("Allow-non-finalized updated", "old_allow_non_finalized", ps.AllowNonFinalized, "new_allow_non_finalized", allowNonFinalized)

	// Update the configuration values
	ps.AllowNonFinalized = allowNonFinalized
	if ps.driver != nil {
		ps.driver.Cfg.AllowNonFinalized = allowNonFinalized
	}

	return nil
}

func (ps *ProposerService) handleDisputeGameTypeChange(value string) error {
	disputeGameType, err := strconv.ParseUint(value, 10, 32)
	if err != nil {
		return fmt.Errorf("failed to parse dispute-game-type value: %w", err)
	}

	if uint64(ps.DisputeGameType) == disputeGameType {
		return nil
	}

	ps.Log.Info("Dispute-game-type updated", "old_dispute_game_type", ps.DisputeGameType, "new_dispute_game_type", disputeGameType)

	// Update the configuration values
	ps.DisputeGameType = uint32(disputeGameType)
	if ps.driver != nil {
		ps.driver.Cfg.DisputeGameType = uint32(disputeGameType)
	}

	return nil
}
