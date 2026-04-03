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
	TeeProverHTTPTimeoutFlag = &cli.DurationFlag{
		Name:    "tee-prover-http-timeout",
		Usage:   "HTTP request timeout for TEE Prover API calls. Increase for long-running proofs in production.",
		EnvVars: prefixEnvVars("TEE_PROVER_HTTP_TIMEOUT"),
		Value:   config.DefaultTeeProverHTTPTimeout,
	}
	L1RPCRateLimitFlag = &cli.Float64Flag{
		Name:    "l1-rpc-rate-limit",
		Usage:   "Self-imposed global rate-limit on L1 RPC requests, specified in requests / second. Disabled if set to 0.",
		EnvVars: prefixEnvVars("L1_RPC_RATE_LIMIT"),
		Value:   0,
	}
	L1RPCMaxBatchSizeFlag = &cli.IntFlag{
		Name:    "l1-rpc-max-batch-size",
		Usage:   "Maximum number of RPC requests to bundle in a single batch. Also used as burst size for rate limiter.",
		EnvVars: prefixEnvVars("L1_RPC_MAX_BATCH_SIZE"),
		Value:   20,
	}

	teeFlags = []cli.Flag{TeeProverRpcFlag, TeeProvePollIntervalFlag, TeeProveTimeoutFlag, TeeProverHTTPTimeoutFlag, L1RPCRateLimitFlag, L1RPCMaxBatchSizeFlag}
)

// onlyTeeGameTypes returns true if all enabled game types are TEE (which doesn't require L2 RPC).
func onlyTeeGameTypes(types []gameTypes.GameType) bool {
	for _, t := range types {
		if t != gameTypes.TeeGameType {
			return false
		}
	}
	return len(types) > 0
}
