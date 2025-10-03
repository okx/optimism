package main

import (
	"context"
	"fmt"
	"os"

	"github.com/apolloconfig/agollo/v4/env/config"
	"github.com/urfave/cli/v2"

	"github.com/ethereum/go-ethereum/log"

	opnode "github.com/ethereum-optimism/optimism/op-node"
	"github.com/ethereum-optimism/optimism/op-node/chaincfg"
	opnodeApollo "github.com/ethereum-optimism/optimism/op-node/cmd/apollo"
	"github.com/ethereum-optimism/optimism/op-node/cmd/genesis"
	"github.com/ethereum-optimism/optimism/op-node/cmd/interop"
	"github.com/ethereum-optimism/optimism/op-node/cmd/networks"
	"github.com/ethereum-optimism/optimism/op-node/cmd/p2p"
	"github.com/ethereum-optimism/optimism/op-node/flags"
	"github.com/ethereum-optimism/optimism/op-node/metrics"
	"github.com/ethereum-optimism/optimism/op-node/node"
	"github.com/ethereum-optimism/optimism/op-node/version"
	opservice "github.com/ethereum-optimism/optimism/op-service"
	"github.com/ethereum-optimism/optimism/op-service/cliapp"
	"github.com/ethereum-optimism/optimism/op-service/ctxinterrupt"
	oplog "github.com/ethereum-optimism/optimism/op-service/log"
	"github.com/ethereum-optimism/optimism/op-service/metrics/doc"
	"github.com/ethereum/go-ethereum/xlayer/apollo"
)

var (
	GitCommit = ""
	GitDate   = ""
)

// VersionWithMeta holds the textual version string including the metadata.
var VersionWithMeta = opservice.FormatVersion(version.Version, GitCommit, GitDate, version.Meta)

func main() {
	// Set up logger with a default INFO level in case we fail to parse flags,
	// otherwise the final critical log won't show what the parsing error was.
	oplog.SetupDefaults()

	app := cli.NewApp()
	app.Version = VersionWithMeta
	app.Flags = cliapp.ProtectFlags(flags.Flags)
	app.Name = "op-node"
	app.Usage = "Optimism Rollup Node"
	app.Description = "The Optimism Rollup Node derives L2 block inputs from L1 data and drives an external L2 Execution Engine to build a L2 chain."
	app.Action = cliapp.LifecycleCmd(RollupNodeMain)
	app.Commands = []*cli.Command{
		{
			Name:        "p2p",
			Subcommands: p2p.Subcommands,
		},
		{
			Name:        "genesis",
			Subcommands: genesis.Subcommands,
		},
		{
			Name:        "doc",
			Subcommands: doc.NewSubcommands(metrics.NewMetrics("default")),
		},
		{
			Name:        "networks",
			Subcommands: networks.Subcommands,
		},
		interop.InteropCmd,
	}

	ctx := ctxinterrupt.WithSignalWaiterMain(context.Background())
	err := app.RunContext(ctx, os.Args)
	if err != nil {
		log.Crit("Application failed", "message", err)
	}
}

func RollupNodeMain(ctx *cli.Context, closeApp context.CancelCauseFunc) (cliapp.Lifecycle, error) {
	logCfg := oplog.ReadCLIConfig(ctx)
	log := oplog.NewLogger(oplog.AppOut(ctx), logCfg)
	oplog.SetGlobalLogHandler(log.Handler())
	opservice.ValidateEnvVars(flags.EnvVarPrefix, flags.Flags, log)
	opservice.WarnOnDeprecatedFlags(ctx, flags.DeprecatedFlags, log)
	m := metrics.NewMetrics("default")

	cfg, err := opnode.NewConfig(ctx, log)
	if err != nil {
		return nil, fmt.Errorf("unable to create the rollup node config: %w", err)
	}
	cfg.Cancel = closeApp

	if cfg.Apollo.Enable && cfg != nil {
		opnodeApollo.SetApolloConfig(cfg)
		// Create geth-specific config handler
		handler := opnodeApollo.NewOpNodeConfigHandler()

		// Create flags for Apollo config context (exclude Apollo connection flags)
		flags := append(flags.Flags)

		client, err := apollo.GetInstance(&config.AppConfig{
			AppID:         cfg.Apollo.AppID,
			IP:            cfg.Apollo.IP,
			Cluster:       cfg.Apollo.Cluster,
			NamespaceName: cfg.Apollo.Namespace,
		}, flags)

		log.Info("Apollo client initialized, apollo config: %+v", cfg.Apollo)

		client.AddHandler(handler)

		if err != nil {
			log.Error("Failed to initialize Apollo configuration: %v", err)
		} else {
			log.Info("Apollo client initialized")
		}
		client.LoadConfig()
	}

	// Only pretty-print the banner if it is a terminal log. Otherwise log it as key-value pairs.
	if logCfg.Format == "terminal" {
		log.Info("rollup config:\n" + cfg.Rollup.Description(chaincfg.L2ChainIDToNetworkDisplayName))
	} else {
		cfg.Rollup.LogDescription(log, chaincfg.L2ChainIDToNetworkDisplayName)
	}

	n, err := node.New(ctx.Context, cfg, log, VersionWithMeta, m)
	if err != nil {
		return nil, fmt.Errorf("unable to create the rollup node: %w", err)
	}

	return n, nil
}
