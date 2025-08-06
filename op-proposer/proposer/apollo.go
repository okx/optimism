package proposer

import (
	"context"
	"fmt"

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
		// configManager := apolloClient.CreateConfigManager(cfg.Apollo.Namespace)

		// Register handlers for specific configuration items
	} else {
		ps.Log.Info("Apollo client disabled")
	}

	return nil
}
