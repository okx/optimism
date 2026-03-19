package flags

import (
	"github.com/ethereum-optimism/optimism/op-challenger/config"
	gameTypes "github.com/ethereum-optimism/optimism/op-challenger/game/types"
	"github.com/urfave/cli/v2"
)

var (
	TeeProverRpcFlag = &cli.StringFlag{
		Name:    "tee-prover-rpc",
		Usage:   "HTTP provider URL for the TEE Prover service (tee game type only)",
		EnvVars: prefixEnvVars("TEE_PROVER_RPC"),
	}
	TeeProvePollIntervalFlag = &cli.DurationFlag{
		Name:    "tee-prove-poll-interval",
		Usage:   "Polling interval for TEE Prover task status (tee game type only)",
		EnvVars: prefixEnvVars("TEE_PROVE_POLL_INTERVAL"),
		Value:   config.DefaultTeeProvePollInterval,
	}
	TeeProveTimeoutFlag = &cli.DurationFlag{
		Name:    "tee-prove-timeout",
		Usage:   "Total timeout for a single game's prove attempt including retries (tee game type only)",
		EnvVars: prefixEnvVars("TEE_PROVE_TIMEOUT"),
		Value:   config.DefaultTeeProveTimeout,
	}
)

func init() {
	optionalFlags = append(optionalFlags, TeeProverRpcFlag, TeeProvePollIntervalFlag, TeeProveTimeoutFlag)
}

// onlyTeeGameTypes returns true if all enabled game types are TEE (which doesn't require L2 RPC).
func onlyTeeGameTypes(types []gameTypes.GameType) bool {
	for _, t := range types {
		if t != gameTypes.TeeGameType {
			return false
		}
	}
	return len(types) > 0
}
