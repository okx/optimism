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
		configManager.RegisterConfigHandler(flags.SubSafetyMarginFlag.Name, bs.handleSubSafetyMarginChange)

		bs.Log.Info("Apollo configuration handlers registered")
	} else {
		bs.Log.Info("Apollo client disabled")
	}

	return nil
}

// handleSubSafetyMarginChange processes changes to the sub-safety-margin configuration
// This function parses the new margin value and updates the channel configuration
func (bs *BatcherService) handleSubSafetyMarginChange(value string) error {
	// Parse the uint64 value
	newMargin, err := strconv.ParseUint(value, 10, 64)
	if err != nil {
		return fmt.Errorf("failed to parse sub-safety-margin value: %w", err)
	}

	// Handle different types of ChannelConfigProvider
	switch config := bs.ChannelConfig.(type) {
	case ChannelConfig:
		// Simple ChannelConfig - direct update
		if config.SubSafetyMargin == newMargin {
			return nil
		}

		oldMargin := config.SubSafetyMargin
		config.SubSafetyMargin = newMargin

		if err := config.Check(); err != nil {
			config.SubSafetyMargin = oldMargin
			return fmt.Errorf("invalid sub-safety-margin value: %w", err)
		}

		bs.ChannelConfig = config
		bs.Log.Info("Sub-safety-margin updated", "old_margin", oldMargin, "new_margin", newMargin)

		// Recreate channel manager to apply new configuration immediately
		if bs.driver != nil && bs.driver.channelMgr != nil {
			bs.driver.channelMgrMutex.Lock()
			bs.driver.channelMgr = NewChannelManager(bs.Log, bs.Metrics, bs.ChannelConfig, bs.RollupConfig)
			if bs.driver.ChannelOutFactory != nil {
				bs.driver.channelMgr.SetChannelOutFactory(bs.driver.ChannelOutFactory)
			}
			bs.driver.channelMgrMutex.Unlock()
			bs.Log.Info("Channel manager recreated with new sub-safety-margin")
		}

	case *DynamicEthChannelConfig:
		// DynamicEthChannelConfig - update both blob and calldata configs
		if config.blobConfig.SubSafetyMargin == newMargin && config.calldataConfig.SubSafetyMargin == newMargin {
			return nil
		}

		oldBlobMargin := config.blobConfig.SubSafetyMargin
		oldCalldataMargin := config.calldataConfig.SubSafetyMargin

		config.blobConfig.SubSafetyMargin = newMargin
		config.calldataConfig.SubSafetyMargin = newMargin

		if err := config.blobConfig.Check(); err != nil {
			config.blobConfig.SubSafetyMargin = oldBlobMargin
			return fmt.Errorf("invalid sub-safety-margin value for blob config: %w", err)
		}

		if err := config.calldataConfig.Check(); err != nil {
			config.blobConfig.SubSafetyMargin = oldBlobMargin
			config.calldataConfig.SubSafetyMargin = oldCalldataMargin
			return fmt.Errorf("invalid sub-safety-margin value for calldata config: %w", err)
		}

		bs.Log.Info("Sub-safety-margin updated", "old_blob_margin", oldBlobMargin,
			"old_calldata_margin", oldCalldataMargin, "new_margin", newMargin)

		// Recreate channel manager to apply new configuration immediately
		if bs.driver != nil && bs.driver.channelMgr != nil {
			bs.driver.channelMgrMutex.Lock()
			bs.driver.channelMgr = NewChannelManager(bs.Log, bs.Metrics, bs.ChannelConfig, bs.RollupConfig)
			if bs.driver.ChannelOutFactory != nil {
				bs.driver.channelMgr.SetChannelOutFactory(bs.driver.ChannelOutFactory)
			}
			bs.driver.channelMgrMutex.Unlock()
			bs.Log.Info("Channel manager recreated with new sub-safety-margin")
		}

	default:
		bs.Log.Warn("Channel configuration type not supported for dynamic updates",
			"type", fmt.Sprintf("%T", bs.ChannelConfig))
		return fmt.Errorf("channel configuration type %T not supported for dynamic updates", bs.ChannelConfig)
	}

	return nil
}
