package apollo

import (
	"fmt"

	"github.com/apolloconfig/agollo/v4/storage"
	"github.com/ethereum-optimism/optimism/op-node/node"
	"github.com/ethereum/go-ethereum/log"
	"github.com/urfave/cli/v2"
)

// TODO: update when configs to be updated by apollo is decided
func (g *OpNodeConfigHandler) loadTest(ctx *cli.Context) {
	loadTestConfig(ctx)
	log.Info(fmt.Sprintf("loaded from apollo config"))
}

func fireTest(ctx *cli.Context, value *storage.ConfigChange) {
	loadTestConfig(ctx)
	log.Info(fmt.Sprintf("apollo old config : %+v", value.OldValue.(string)))
	log.Info(fmt.Sprintf("apollo config changed: %+v", value.NewValue.(string)))
}

func loadTestConfig(ctx *cli.Context) {
	UnsafeGetApolloConfig().Lock()
	defer UnsafeGetApolloConfig().Unlock()

	config := UnsafeGetApolloConfig()
	if config == nil {
		log.Warn("Apollo config is nil, skipping L2GasPricer config load")
		return
	}
	loadNodeTestConfig(ctx, UnsafeGetApolloConfig().NodeCfg)
}

func loadNodeTestConfig(ctx *cli.Context, nodeCfg *node.Config) {
}
