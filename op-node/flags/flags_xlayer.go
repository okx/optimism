package flags

import (
	"github.com/urfave/cli/v2"
)

var (
	ApolloEnabledFlag = &cli.BoolFlag{
		Name:  "apollo.enabled",
		Usage: "Enable Apollo configuration service",
		Value: false,
	}
	ApolloAppIDFlag = &cli.StringFlag{
		Name:  "apollo.app-id",
		Usage: "Apollo app ID",
		Value: "",
	}
	ApolloIPFlag = &cli.StringFlag{
		Name:  "apollo.ip",
		Usage: "Apollo IP",
		Value: "",
	}
	ApolloClusterFlag = &cli.StringFlag{
		Name:  "apollo.cluster",
		Usage: "Apollo cluster name",
		Value: "default",
	}
	ApolloNamespaceFlag = &cli.StringFlag{
		Name:  "apollo.namespace",
		Usage: "Apollo namespace",
		Value: "application",
	}

	XLayerFlags = []cli.Flag{
		ApolloEnabledFlag,
		ApolloAppIDFlag,
		ApolloIPFlag,
		ApolloClusterFlag,
		ApolloNamespaceFlag,
	}
)
