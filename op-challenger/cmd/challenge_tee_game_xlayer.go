package main

// For xlayer

import (
	"context"
	"fmt"

	contractMetrics "github.com/ethereum-optimism/optimism/op-challenger/game/fault/contracts/metrics"
	"github.com/ethereum-optimism/optimism/op-challenger/flags"
	"github.com/ethereum-optimism/optimism/op-challenger/game/fault/contracts"
	oplog "github.com/ethereum-optimism/optimism/op-service/log"
	"github.com/ethereum-optimism/optimism/op-service/sources/batching"
	"github.com/ethereum-optimism/optimism/op-service/txmgr"
	"github.com/ethereum/go-ethereum/common"
	"github.com/urfave/cli/v2"
)

func newTeeDisputeGameContract(_ context.Context, m contractMetrics.ContractMetricer, addr common.Address, caller *batching.MultiCaller) (contracts.TeeDisputeGameContract, error) {
	return contracts.NewTeeDisputeGameContract(m, addr, caller)
}

func ChallengeTeeGame(ctx *cli.Context) error {
	contract, txMgr, err := NewContractWithTxMgr[contracts.TeeDisputeGameContract](
		ctx,
		AddrFromFlag(GameAddressFlag.Name),
		newTeeDisputeGameContract,
	)
	if err != nil {
		return fmt.Errorf("failed to create tee dispute game bindings: %w", err)
	}

	tx, err := contract.ChallengeTx(context.Background())
	if err != nil {
		return fmt.Errorf("failed to create challenge tx: %w", err)
	}

	rct, err := txMgr.Send(context.Background(), tx)
	if err != nil {
		return fmt.Errorf("failed to send challenge tx: %w", err)
	}

	fmt.Printf("Sent challenge tx: hash=%s status=%v\n", rct.TxHash.String(), rct.Status)
	return nil
}

func challengeTeeGameFlags() []cli.Flag {
	cliFlags := []cli.Flag{
		flags.L1EthRpcFlag,
		GameAddressFlag,
	}
	cliFlags = append(cliFlags, txmgr.CLIFlagsWithDefaults(flags.EnvVarPrefix, txmgr.DefaultChallengerFlagValues)...)
	cliFlags = append(cliFlags, oplog.CLIFlags(flags.EnvVarPrefix)...)
	return cliFlags
}

var ChallengeTeeGameCommand = &cli.Command{
	Name:        "challenge-tee",
	Usage:       "Send a challenge() transaction to a TEE dispute game using XLayer remote signer",
	Description: "Calls challenge() on the specified TeeDisputeGame contract, paying the required challenger bond.",
	Action:      Interruptible(ChallengeTeeGame),
	Flags:       challengeTeeGameFlags(),
}
