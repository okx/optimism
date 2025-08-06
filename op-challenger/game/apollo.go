package game

import (
	"context"
	"fmt"

	"github.com/ethereum-optimism/optimism/op-challenger/config"
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
		// configManager := apolloClient.CreateConfigManager(cfg.Apollo.Namespace)

		// Register handlers for specific configuration items
	} else {
		s.logger.Info("Apollo client disabled")
	}

	return nil
}
