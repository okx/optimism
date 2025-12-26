package engine

import (
	"sync"
	"time"

	"github.com/ethereum/go-ethereum/log"
)

// stallConfig holds the test stall configuration for X Layer.
// This is a package-level variable that can be configured at startup.
type stallConfig struct {
	mu       sync.RWMutex
	height   uint64
	duration time.Duration
	stalled  bool // tracks if we've already stalled to avoid repeated stalls
}

var globalStallConfig = &stallConfig{}

// SetStallConfig sets the stall configuration.
// This should be called during node initialization.
func SetStallConfig(height uint64, duration time.Duration) {
	globalStallConfig.mu.Lock()
	defer globalStallConfig.mu.Unlock()
	globalStallConfig.height = height
	globalStallConfig.duration = duration
	globalStallConfig.stalled = false
}

// GetStallConfig returns the current stall configuration.
func GetStallConfig() (height uint64, duration time.Duration) {
	globalStallConfig.mu.RLock()
	defer globalStallConfig.mu.RUnlock()
	return globalStallConfig.height, globalStallConfig.duration
}

// stallEnabled returns true if stall is enabled and configured.
func stallEnabled() bool {
	globalStallConfig.mu.RLock()
	defer globalStallConfig.mu.RUnlock()
	return globalStallConfig.height > 0 && globalStallConfig.duration > 0
}

// checkAndStall checks if the current block height matches the stall height,
// and if so, sleeps for the configured duration.
// Returns true if stall occurred, false otherwise.
func checkAndStall(logger log.Logger, blockNumber uint64) bool {
	if !stallEnabled() {
		return false
	}

	globalStallConfig.mu.Lock()
	defer globalStallConfig.mu.Unlock()

	// Check if we should stall
	if blockNumber != globalStallConfig.height {
		return false
	}

	// Check if we've already stalled for this height
	if globalStallConfig.stalled {
		return false
	}

	// Mark as stalled and perform the stall
	globalStallConfig.stalled = true

	logger.Warn("[TEST] Node stalling at configured block height",
		"height", blockNumber,
		"duration", globalStallConfig.duration,
	)

	// Sleep for the configured duration
	time.Sleep(globalStallConfig.duration)

	logger.Warn("[TEST] Node stall completed, resuming normal operation",
		"height", blockNumber,
	)

	return true
}
