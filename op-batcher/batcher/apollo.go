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
		configManager.RegisterConfigHandler(flags.MaxChannelDurationFlag.Name, bs.handleMaxChannelDurationChange)
		configManager.RegisterConfigHandler(flags.MaxL1TxSizeBytesFlag.Name, bs.handleMaxL1TxSizeBytesChange)

		bs.Log.Info("Apollo configuration handlers registered")
	} else {
		bs.Log.Info("Apollo client disabled")
	}

	return nil
}

// updateChannelConfigField is a generic helper for updating uint64 channel configuration fields
func (bs *BatcherService) updateChannelConfigField(value, fieldName string,
	updateSimpleConfig func(*ChannelConfig, uint64) (uint64, error),
	updateDynamicConfig func(*DynamicEthChannelConfig, uint64) (uint64, uint64, error)) error {

	// Parse the uint64 value
	newValue, err := strconv.ParseUint(value, 10, 64)
	if err != nil {
		return fmt.Errorf("failed to parse %s value: %w", fieldName, err)
	}

	// Handle different types of ChannelConfigProvider
	switch config := bs.ChannelConfig.(type) {
	case ChannelConfig:
		// updateSimpleConfig should validate first, then update if valid
		oldValue, err := updateSimpleConfig(&config, newValue)
		if err != nil {
			return fmt.Errorf("failed to update %s: %w", fieldName, err)
		}
		if oldValue == newValue {
			return nil // No change needed
		}

		bs.ChannelConfig = config
		bs.Log.Info(fmt.Sprintf("%s updated", fieldName), "old_value", oldValue, "new_value", newValue)

	case *DynamicEthChannelConfig:
		// updateDynamicConfig should validate first, then update if valid
		oldBlobValue, oldCalldataValue, err := updateDynamicConfig(config, newValue)
		if err != nil {
			return fmt.Errorf("failed to update %s: %w", fieldName, err)
		}
		if oldBlobValue == newValue && oldCalldataValue == newValue {
			return nil // No change needed
		}

		bs.Log.Info(fmt.Sprintf("%s updated", fieldName),
			"old_blob_value", oldBlobValue, "old_calldata_value", oldCalldataValue, "new_value", newValue)

	default:
		bs.Log.Warn("Channel configuration type not supported for dynamic updates",
			"type", fmt.Sprintf("%T", bs.ChannelConfig))
		return fmt.Errorf("channel configuration type %T not supported for dynamic updates", bs.ChannelConfig)
	}

	// Recreate channel manager to apply new configuration immediately
	bs.refreshChannelManager(fieldName, newValue)
	return nil
}

// recreateChannelManager forces current channel to be marked as full so new channels use updated config
func (bs *BatcherService) refreshChannelManager(configName string, newValue uint64) {
	if bs.driver != nil && bs.driver.channelMgr != nil {
		bs.driver.channelMgrMutex.Lock()
		defer bs.driver.channelMgrMutex.Unlock()

		// Mark current channel as full so it gets finalized and a new one is created
		// This is safer than setting it to nil which could cause panic
		if bs.driver.channelMgr.currentChannel != nil {
			// Force the channel to be full by setting a "configuration changed" error
			bs.driver.channelMgr.currentChannel.channelBuilder.setFullErr(fmt.Errorf("channel closed due to configuration change: %s", configName))
		}

		// Update defaultCfg so new channels use the updated configuration
		// This is the key fix - ensuring new channels pick up the new config
		bs.driver.channelMgr.defaultCfg = bs.ChannelConfig.ChannelConfig(false)

		bs.Log.Info("Current channel marked as full due to config change, new config will take effect on next channel",
			"updated_config", configName, "new_value", newValue)
	}
}

// validateAndUpdateSimpleConfig validates and updates a simple ChannelConfig field
func validateAndUpdateSimpleConfig(config *ChannelConfig, newValue uint64,
	getOldValue func(*ChannelConfig) uint64,
	setNewValue func(*ChannelConfig, uint64)) (uint64, error) {

	oldValue := getOldValue(config)

	// Create a copy to test validation before updating
	testConfig := *config
	setNewValue(&testConfig, newValue)
	if err := testConfig.Check(); err != nil {
		return 0, err
	}

	// Update the actual config if validation passes
	setNewValue(config, newValue)
	return oldValue, nil
}

// validateAndUpdateDynamicConfig validates and updates a DynamicEthChannelConfig field
func validateAndUpdateDynamicConfig(config *DynamicEthChannelConfig, newValue uint64,
	getOldBlobValue func(*ChannelConfig) uint64,
	getOldCalldataValue func(*ChannelConfig) uint64,
	setNewValue func(*ChannelConfig, uint64)) (uint64, uint64, error) {

	oldBlobValue := getOldBlobValue(&config.blobConfig)
	oldCalldataValue := getOldCalldataValue(&config.calldataConfig)

	// Create copies to test validation before updating
	testBlobConfig := config.blobConfig
	testCalldataConfig := config.calldataConfig
	setNewValue(&testBlobConfig, newValue)
	setNewValue(&testCalldataConfig, newValue)

	if err := testBlobConfig.Check(); err != nil {
		return 0, 0, fmt.Errorf("invalid value for blob config: %w", err)
	}
	if err := testCalldataConfig.Check(); err != nil {
		return 0, 0, fmt.Errorf("invalid value for calldata config: %w", err)
	}

	// Update the actual configs if validation passes
	setNewValue(&config.blobConfig, newValue)
	setNewValue(&config.calldataConfig, newValue)
	return oldBlobValue, oldCalldataValue, nil
}

// handleSubSafetyMarginChange processes changes to the sub-safety-margin configuration
func (bs *BatcherService) handleSubSafetyMarginChange(value string) error {
	return bs.updateChannelConfigField(value, "sub-safety-margin",
		func(config *ChannelConfig, newValue uint64) (uint64, error) {
			return validateAndUpdateSimpleConfig(config, newValue,
				func(c *ChannelConfig) uint64 { return c.SubSafetyMargin },
				func(c *ChannelConfig, v uint64) { c.SubSafetyMargin = v })
		},
		func(config *DynamicEthChannelConfig, newValue uint64) (uint64, uint64, error) {
			return validateAndUpdateDynamicConfig(config, newValue,
				func(c *ChannelConfig) uint64 { return c.SubSafetyMargin },
				func(c *ChannelConfig) uint64 { return c.SubSafetyMargin },
				func(c *ChannelConfig, v uint64) { c.SubSafetyMargin = v })
		})
}

// handleMaxChannelDurationChange processes changes to the max-channel-duration configuration
func (bs *BatcherService) handleMaxChannelDurationChange(value string) error {
	return bs.updateChannelConfigField(value, "max-channel-duration",
		func(config *ChannelConfig, newValue uint64) (uint64, error) {
			return validateAndUpdateSimpleConfig(config, newValue,
				func(c *ChannelConfig) uint64 { return c.MaxChannelDuration },
				func(c *ChannelConfig, v uint64) { c.MaxChannelDuration = v })
		},
		func(config *DynamicEthChannelConfig, newValue uint64) (uint64, uint64, error) {
			return validateAndUpdateDynamicConfig(config, newValue,
				func(c *ChannelConfig) uint64 { return c.MaxChannelDuration },
				func(c *ChannelConfig) uint64 { return c.MaxChannelDuration },
				func(c *ChannelConfig, v uint64) { c.MaxChannelDuration = v })
		})
}

func (bs *BatcherService) handleMaxL1TxSizeBytesChange(value string) error {
	return bs.updateChannelConfigField(value, "max-l1-tx-size-bytes",
		func(config *ChannelConfig, newValue uint64) (uint64, error) {
			return validateAndUpdateSimpleConfig(config, newValue,
				func(c *ChannelConfig) uint64 { return c.MaxFrameSize },
				func(c *ChannelConfig, v uint64) { c.MaxFrameSize = v - 1 })
		},
		func(config *DynamicEthChannelConfig, newValue uint64) (uint64, uint64, error) {
			return validateAndUpdateDynamicConfig(config, newValue,
				func(c *ChannelConfig) uint64 { return c.MaxFrameSize },
				func(c *ChannelConfig) uint64 { return c.MaxFrameSize },
				func(c *ChannelConfig, v uint64) { c.MaxFrameSize = v - 1 })
		})
}
