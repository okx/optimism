package apollo

import (
	"time"

	opservice "github.com/ethereum-optimism/optimism/op-service"
	"github.com/urfave/cli/v2"
)

const (
	ApolloEnabledFlagName     = "apollo.enabled"
	ApolloEndpointFlagName    = "apollo.endpoint"
	ApolloAppIDFlagName       = "apollo.app-id"
	ApolloClusterFlagName     = "apollo.cluster"
	ApolloNamespaceFlagName   = "apollo.namespace"
	ApolloSecretFlagName      = "apollo.secret"
	ApolloSyncTimeoutFlagName = "apollo.sync-timeout"
)

func CLIFlags(envPrefix string) []cli.Flag {
	return CLIFlagsWithCategory(envPrefix, "")
}

func CLIFlagsWithCategory(envPrefix string, category string) []cli.Flag {
	return []cli.Flag{
		&cli.BoolFlag{
			Name:     ApolloEnabledFlagName,
			Usage:    "Enable Apollo configuration management",
			EnvVars:  opservice.PrefixEnvVar(envPrefix, "APOLLO_ENABLED"),
			Category: category,
		},
		&cli.StringFlag{
			Name:     ApolloEndpointFlagName,
			Usage:    "Apollo configuration service endpoint",
			EnvVars:  opservice.PrefixEnvVar(envPrefix, "APOLLO_ENDPOINT"),
			Category: category,
		},
		&cli.StringFlag{
			Name:     ApolloAppIDFlagName,
			Usage:    "Apollo application ID",
			EnvVars:  opservice.PrefixEnvVar(envPrefix, "APOLLO_APP_ID"),
			Category: category,
		},
		&cli.StringFlag{
			Name:     ApolloClusterFlagName,
			Usage:    "Apollo cluster name",
			Value:    "default",
			EnvVars:  opservice.PrefixEnvVar(envPrefix, "APOLLO_CLUSTER"),
			Category: category,
		},
		&cli.StringFlag{
			Name:     ApolloNamespaceFlagName,
			Usage:    "Apollo namespace",
			Value:    "application",
			EnvVars:  opservice.PrefixEnvVar(envPrefix, "APOLLO_NAMESPACE"),
			Category: category,
		},
		&cli.StringFlag{
			Name:     ApolloSecretFlagName,
			Usage:    "Apollo access secret",
			EnvVars:  opservice.PrefixEnvVar(envPrefix, "APOLLO_SECRET"),
			Category: category,
		},
		&cli.DurationFlag{
			Name:     ApolloSyncTimeoutFlagName,
			Usage:    "Apollo configuration sync timeout",
			Value:    time.Second * 30,
			EnvVars:  opservice.PrefixEnvVar(envPrefix, "APOLLO_SYNC_TIMEOUT"),
			Category: category,
		},
	}
}
