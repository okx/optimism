package signer

import (
	opservice "github.com/ethereum-optimism/optimism/op-service"
	"github.com/urfave/cli/v2"
)

const (
	XLayerEnabledFlagName       = "xlayer-signer.enabled"
	XLayerEndpointFlagName      = "xlayer-signer.endpoint"
	XLayerAddressFlagName       = "xlayer-signer.address"
	XLayerUserIDFlagName        = "xlayer-signer.user-id"
	XLayerSymbolFlagName        = "xlayer-signer.symbol"
	XLayerProjectSymbolFlagName = "xlayer-signer.project-symbol"
	XLayerOperateSymbolFlagName = "xlayer-signer.operate-symbol"
	XLayerOperateAmountFlagName = "xlayer-signer.operate-amount"
	XLayerSysFromFlagName       = "xlayer-signer.sys-from"
	XLayerAccessKeyFlagName     = "xlayer-signer.access-key"
	XLayerSecretKeyFlagName     = "xlayer-signer.secret-key"
	XLayerTimeoutFlagName       = "xlayer-signer.timeout"
)

// XLayerCLIFlags returns CLI flags for XLayer remote signer configuration
func XLayerCLIFlags(envPrefix string, category string) []cli.Flag {
	envPrefix += "_XLAYER_SIGNER"
	return []cli.Flag{
		&cli.BoolFlag{
			Name:     XLayerEnabledFlagName,
			Usage:    "Enable XLayer remote signer",
			EnvVars:  opservice.PrefixEnvVar(envPrefix, "ENABLED"),
			Category: category,
			Value:    false,
		},
		&cli.StringFlag{
			Name:     XLayerEndpointFlagName,
			Usage:    "XLayer signer endpoint URL",
			EnvVars:  opservice.PrefixEnvVar(envPrefix, "ENDPOINT"),
			Category: category,
		},
		&cli.StringFlag{
			Name:     XLayerAddressFlagName,
			Usage:    "Address that XLayer signer will sign for",
			EnvVars:  opservice.PrefixEnvVar(envPrefix, "ADDRESS"),
			Category: category,
		},
		&cli.IntFlag{
			Name:     XLayerUserIDFlagName,
			Usage:    "XLayer user ID",
			EnvVars:  opservice.PrefixEnvVar(envPrefix, "USER_ID"),
			Category: category,
			Value:    0,
		},
		&cli.IntFlag{
			Name:     XLayerSymbolFlagName,
			Usage:    "XLayer symbol identifier",
			EnvVars:  opservice.PrefixEnvVar(envPrefix, "SYMBOL"),
			Category: category,
			Value:    2882, // Default for devnet
		},
		&cli.IntFlag{
			Name:     XLayerProjectSymbolFlagName,
			Usage:    "XLayer project symbol identifier",
			EnvVars:  opservice.PrefixEnvVar(envPrefix, "PROJECT_SYMBOL"),
			Category: category,
			Value:    3011,
		},
		&cli.IntFlag{
			Name:     XLayerOperateSymbolFlagName,
			Usage:    "XLayer operate symbol identifier",
			EnvVars:  opservice.PrefixEnvVar(envPrefix, "OPERATE_SYMBOL"),
			Category: category,
			Value:    2,
		},
		&cli.StringFlag{
			Name:     XLayerOperateAmountFlagName,
			Usage:    "XLayer operate amount (in ETH, supports decimals, e.g., '0.08', '1', '2.5')",
			EnvVars:  opservice.PrefixEnvVar(envPrefix, "OPERATE_AMOUNT"),
			Category: category,
			Value:    "0",
		},
		&cli.IntFlag{
			Name:     XLayerSysFromFlagName,
			Usage:    "XLayer system from identifier",
			EnvVars:  opservice.PrefixEnvVar(envPrefix, "SYS_FROM"),
			Category: category,
			Value:    3,
		},
		&cli.StringFlag{
			Name:     XLayerAccessKeyFlagName,
			Usage:    "XLayer access key for authentication",
			EnvVars:  opservice.PrefixEnvVar(envPrefix, "ACCESS_KEY"),
			Category: category,
		},
		&cli.StringFlag{
			Name:     XLayerSecretKeyFlagName,
			Usage:    "XLayer secret key for authentication",
			EnvVars:  opservice.PrefixEnvVar(envPrefix, "SECRET_KEY"),
			Category: category,
		},
		&cli.StringFlag{
			Name:     XLayerTimeoutFlagName,
			Usage:    "XLayer request timeout duration",
			EnvVars:  opservice.PrefixEnvVar(envPrefix, "TIMEOUT"),
			Category: category,
			Value:    "30s",
		},
	}
}

// ReadXLayerCLIConfig reads XLayer configuration from CLI context
func ReadXLayerCLIConfig(ctx *cli.Context) XLayerCLIConfig {
	return XLayerCLIConfig{
		Enabled:       ctx.Bool(XLayerEnabledFlagName),
		Endpoint:      ctx.String(XLayerEndpointFlagName),
		Address:       ctx.String(XLayerAddressFlagName),
		UserID:        ctx.Int(XLayerUserIDFlagName),
		Symbol:        ctx.Int(XLayerSymbolFlagName),
		ProjectSymbol: ctx.Int(XLayerProjectSymbolFlagName),
		OperateSymbol: ctx.Int(XLayerOperateSymbolFlagName),
		OperateAmount: ctx.String(XLayerOperateAmountFlagName),
		SysFrom:       ctx.Int(XLayerSysFromFlagName),
		AccessKey:     ctx.String(XLayerAccessKeyFlagName),
		SecretKey:     ctx.String(XLayerSecretKeyFlagName),
		Timeout:       ctx.String(XLayerTimeoutFlagName),
	}
}
