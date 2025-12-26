package flags

import (
	"time"

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

	// Test stall flags - used for testing node stalling behavior
	TestStallHeightFlag = &cli.Uint64Flag{
		Name:  "test.stall.height",
		Usage: "[TEST ONLY] Block height at which the node will stall (sleep). Set to 0 to disable.",
		Value: 0,
	}
	TestStallDurationFlag = &cli.DurationFlag{
		Name:  "test.stall.duration",
		Usage: "[TEST ONLY] Duration to stall (sleep) when reaching the specified block height.",
		Value: 0 * time.Second,
	}

	XLayerFlags = []cli.Flag{
		ApolloEnabledFlag,
		ApolloAppIDFlag,
		ApolloIPFlag,
		ApolloClusterFlag,
		ApolloNamespaceFlag,
		TestStallHeightFlag,
		TestStallDurationFlag,
	}
)
