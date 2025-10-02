package apollo

import (
	"sync"

	"github.com/apolloconfig/agollo/v4/storage"
	"github.com/ethereum-optimism/optimism/op-node/node"
	"github.com/ethereum/go-ethereum/log"
	"github.com/ethereum/go-ethereum/xlayer/apollo"
	"github.com/urfave/cli/v2"
)

// ApolloConfig holds the global Apollo configuration state
type ApolloConfigImpl struct {
	sync.RWMutex
	NodeCfg node.Config
}

// Global Apollo configuration instance
var globalApolloConfig *ApolloConfigImpl
var configMutex sync.RWMutex

// UnsafeGetApolloConfig returns the global Apollo configuration
// This is unsafe and should be used carefully
func UnsafeGetApolloConfig() *ApolloConfigImpl {
	configMutex.RLock()
	defer configMutex.RUnlock()
	return globalApolloConfig
}

// SetApolloConfig sets the global Apollo configuration
func SetApolloConfig(nodeCfg node.Config) {
	configMutex.Lock()
	defer configMutex.Unlock()

	if globalApolloConfig == nil {
		globalApolloConfig = &ApolloConfigImpl{}
	}

	globalApolloConfig.NodeCfg = nodeCfg
}

// IsApolloConfigSet checks if Apollo configuration has been set
func IsApolloConfigSet() bool {
	configMutex.RLock()
	defer configMutex.RUnlock()
	return globalApolloConfig != nil
}

// OpNodeConfigHandler implements geth-specific configuration change logic
type OpNodeConfigHandler struct{}

// HandleConfigChange implements geth-specific configuration change logic
func (g *OpNodeConfigHandler) HandleConfigChange(prefix string, ctx *cli.Context, key string, value *storage.ConfigChange) {
	switch prefix {
	case apollo.Sequencer:
		log.Info("apollo sequencer", "key", key, "value", value.NewValue)
		// TODO: update when configs to be updated by apollo is decided
		fireTest(ctx, value)
	default:
		log.Info("unknown config prefix", "prefix", prefix, "key", key, "value", value.NewValue)
	}
}

// LoadConfig implements geth-specific configuration loading logic
func (g *OpNodeConfigHandler) LoadConfig(prefix string, ctx *cli.Context) {
	switch prefix {
	// TODO: update when configs to be updated by apollo is decided
	case apollo.L2GasPricer:
		g.loadTest(ctx)
	default:
		log.Info("OpNode unknown config prefix for loading", "prefix", prefix)
	}
}

// NewOpNodeConfigHandler creates a new geth-specific config handler
func NewOpNodeConfigHandler() *OpNodeConfigHandler {
	return &OpNodeConfigHandler{}
}
