package apollo

import (
	"time"

	"github.com/urfave/cli/v2"
)

type CLIConfig struct {
	Enabled     bool
	Endpoint    string
	AppID       string
	Cluster     string
	Namespace   string
	Secret      string
	SyncTimeout time.Duration
}

func ReadCLIConfig(ctx *cli.Context) CLIConfig {
	return CLIConfig{
		Enabled:     ctx.Bool(ApolloEnabledFlagName),
		Endpoint:    ctx.String(ApolloEndpointFlagName),
		AppID:       ctx.String(ApolloAppIDFlagName),
		Cluster:     ctx.String(ApolloClusterFlagName),
		Namespace:   ctx.String(ApolloNamespaceFlagName),
		Secret:      ctx.String(ApolloSecretFlagName),
		SyncTimeout: ctx.Duration(ApolloSyncTimeoutFlagName),
	}
}
