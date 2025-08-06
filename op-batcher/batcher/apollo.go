package batcher

import (
	"context"
	"fmt"
	"strconv"

	"github.com/ethereum-optimism/optimism/op-batcher/flags"
	"github.com/ethereum-optimism/optimism/op-service/apollo"
)

// initApollo initializes the Apollo client for dynamic configuration management
func (bs *BatcherService) initApollo(ctx context.Context, cfg *CLIConfig) error {
	// Initialize Apollo client
	apolloClient, err := apollo.NewClient(cfg.Apollo, bs.Log)
	if err != nil {
		return fmt.Errorf("failed to initialize Apollo client: %w", err)
	}
	bs.apolloClient = apolloClient

	if apolloClient.Enabled() {
		bs.Log.Info("Apollo client initialized and enabled")

		// Create a configuration manager for this namespace
		configManager := apolloClient.CreateConfigManager(cfg.Apollo.Namespace)

		// Register handlers for specific configuration items
		configManager.RegisterConfigHandler(flags.MaxChannelDurationFlag.Name, bs.handleMaxChannelDurationChange)

		bs.Log.Info("Apollo configuration handlers registered")
	} else {
		bs.Log.Info("Apollo client disabled")
	}

	return nil
}

// handleMaxChannelDurationChange processes changes to the MaxChannelDuration configuration
func (bs *BatcherService) handleMaxChannelDurationChange(value string) error {
	newDuration, err := strconv.ParseUint(value, 10, 64)
	if err != nil {
		return fmt.Errorf("failed to parse max channel duration: %w", err)
	}

	bs.Log.Info("Applying max channel duration change",
		"old_duration", bs.ChannelConfig.ChannelConfig(false).MaxChannelDuration,
		"new_duration", newDuration)

	var cfgProvider ChannelConfigProvider
	// Try to update the channel configuration using type assertion
	if cc, ok := bs.ChannelConfig.(ChannelConfig); ok {
		// For value type, we need to create a new config and replace it
		cc.MaxChannelDuration = newDuration
		bs.ChannelConfig = cc
		cfgProvider = cc
		bs.Log.Info("Max channel duration updated successfully", "new_duration", newDuration)
	} else if dynamicConfig, ok := bs.ChannelConfig.(*DynamicEthChannelConfig); ok {
		// Update both blob and calldata configs in dynamic config
		dynamicConfig.blobConfig.MaxChannelDuration = newDuration
		dynamicConfig.calldataConfig.MaxChannelDuration = newDuration
		cfgProvider = dynamicConfig
		bs.Log.Info("Max channel duration updated successfully in dynamic config", "new_duration", newDuration)
	} else {
		// If not a supported type, we can't update it
		bs.Log.Warn("Channel config type not supported for dynamic updates", "type", fmt.Sprintf("%T", bs.ChannelConfig))
		return fmt.Errorf("channel config type %T does not support dynamic updates", bs.ChannelConfig)
	}

	// Force close current channel to apply new configuration
	if bs.driver != nil {
		bs.Log.Info("Forcing current channel to close to apply new max channel duration")
		bs.driver.channelMgr.cfgProvider = cfgProvider
		// Clear the channel manager state to force creation of a new channel with updated config
		// This will cause the current channel to be closed and a new one created with the updated MaxChannelDuration
		bs.driver.clearState(context.Background())
		bs.Log.Info("Channel state cleared, new channel will be created with updated config")
	}

	return nil
}
