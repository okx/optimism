package apollo

import (
	"strings"
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
	NodeCfg *node.Config
}

// Global Apollo configuration instance
var globalApolloConfig *ApolloConfigImpl

// TryUnsafeGetApolloConfig returns the global Apollo configuration
// This is unsafe and should be used carefully
func TryUnsafeGetApolloConfig() *ApolloConfigImpl {
	return globalApolloConfig
}

// SetApolloConfig sets the global Apollo configuration
func SetApolloConfig(nodeCfg *node.Config) {
	if globalApolloConfig == nil {
		globalApolloConfig = &ApolloConfigImpl{}
	}

	globalApolloConfig.NodeCfg = nodeCfg
}

// IsApolloConfigSet checks if Apollo configuration has been set
func IsApolloConfigSet() bool {
	return globalApolloConfig != nil
}

// OpNodeConfigHandler implements geth-specific configuration change logic
type OpNodeConfigHandler struct{}

// HandleConfigChange implements geth-specific configuration change logic
func (g *OpNodeConfigHandler) HandleConfigChange(prefix string, ctx *cli.Context, key string, value *storage.ConfigChange) {
	component := getComponentFromNamespace(prefix)

	// Validate that this is for op-node component
	if component != apollo.OpNodeComponent {
		log.Warn("OpNode received config load request for non-opnode namespace, ignoring", "component", component, "prefix", prefix)
		return
	}
	switch prefix {
	// TODO: update when configs to be updated by apollo is decided
	case apollo.JsonRPC:
	case apollo.Sequencer:
	case apollo.L2GasPricer:
	case apollo.Pool:
	case apollo.Halt:
	default:
		log.Info("unknown config prefix", "prefix", prefix, "key", key, "value", value.NewValue)
	}
}

// LoadConfig implements op-node-specific configuration loading logic
func (g *OpNodeConfigHandler) LoadConfig(prefix string, ctx *cli.Context) {
	component := getComponentFromNamespace(prefix)
	// Validate that this is for op-node component
	if component != apollo.OpNodeComponent {
		log.Warn("OpNode received config load request for non-opnode namespace, ignoring", "component", component, "prefix", prefix)
		return
	}

	switch prefix {
	// TODO: update when configs to be updated by apollo is decided
	case apollo.JsonRPC:
	case apollo.Sequencer:
	case apollo.L2GasPricer:
	case apollo.Pool:
	case apollo.Halt:
	default:
		log.Info("OpNode unknown config prefix for loading", "prefix", prefix)
	}
}

// getComponentFromNamespace extracts the component prefix from namespace
// e.g. "opnode_sequencer" -> "opnode"
func getComponentFromNamespace(namespace string) string {
	parts := strings.Split(namespace, "_")
	if len(parts) > 0 {
		return parts[0]
	}
	return ""
}

// NewOpNodeConfigHandler creates a new geth-specific config handler
func NewOpNodeConfigHandler() *OpNodeConfigHandler {
	return &OpNodeConfigHandler{}
}
