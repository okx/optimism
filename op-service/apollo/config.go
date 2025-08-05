package apollo

import (
	"errors"
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

func NewCLIConfig() CLIConfig {
	return CLIConfig{
		Enabled:     false,
		Cluster:     "default",
		Namespace:   "application",
		SyncTimeout: time.Second * 30,
	}
}

func (c CLIConfig) Check() error {
	if !c.Enabled {
		return nil
	}
	if c.Endpoint == "" {
		return errors.New("apollo endpoint is required when apollo is enabled")
	}
	if c.AppID == "" {
		return errors.New("apollo app-id is required when apollo is enabled")
	}
	return nil
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
