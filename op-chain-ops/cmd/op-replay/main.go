package main

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"math/big"
	"os"
	"time"

	"github.com/urfave/cli/v2"

	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/common/hexutil"
	"github.com/ethereum/go-ethereum/consensus"
	"github.com/ethereum/go-ethereum/consensus/beacon"
	"github.com/ethereum/go-ethereum/consensus/misc"
	"github.com/ethereum/go-ethereum/core"
	gstate "github.com/ethereum/go-ethereum/core/state"
	"github.com/ethereum/go-ethereum/core/stateless"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/core/vm"
	logger2 "github.com/ethereum/go-ethereum/eth/tracers/logger"
	"github.com/ethereum/go-ethereum/ethclient"
	"github.com/ethereum/go-ethereum/log"
	"github.com/ethereum/go-ethereum/params"
	"github.com/ethereum/go-ethereum/rpc"
	"github.com/ethereum/go-ethereum/triedb"

	op_service "github.com/ethereum-optimism/optimism/op-service"
	"github.com/ethereum-optimism/optimism/op-service/cliapp"
	"github.com/ethereum-optimism/optimism/op-service/ctxinterrupt"
	"github.com/ethereum-optimism/optimism/op-service/eth"
	oplog "github.com/ethereum-optimism/optimism/op-service/log"
	"github.com/ethereum-optimism/optimism/op-service/retry"
	"github.com/ethereum-optimism/optimism/op-service/sources"
)

// =============================================================================
// Constants and Global Variables
// =============================================================================

const EnvPrefix = "OP_REPLAY"

var (
	RPCFlag = &cli.StringFlag{
		Name:     "rpc",
		Usage:    "RPC endpoint to fetch data from",
		EnvVars:  op_service.PrefixEnvVar(EnvPrefix, "RPC"),
		Required: true,
	}
	StartFlag = &cli.IntFlag{
		Name:     "start",
		Usage:    "Start block number",
		EnvVars:  op_service.PrefixEnvVar(EnvPrefix, "START"),
		Required: true,
	}
	EndFlag = &cli.IntFlag{
		Name:     "end",
		Usage:    "End block number",
		EnvVars:  op_service.PrefixEnvVar(EnvPrefix, "END"),
		Required: true,
	}
	OutPathFlag = &cli.PathFlag{
		Name:    "out",
		Usage:   "Path to directory to write trace data files (optional, if not specified no files will be written)",
		EnvVars: op_service.PrefixEnvVar(EnvPrefix, "OUT"),
		Value:   "",
	}
)

// =============================================================================
// Core Data Structures
// =============================================================================

// remoteChainCtx provides access to block-headers, for usage by the state-transition,
// such as basefee computation (based on prior block) and EVM block-hash opcode.
type remoteChainCtx struct {
	consensusEng consensus.Engine
	hdr          *types.Header
	cfg          *params.ChainConfig
	cl           *ethclient.Client
	logger       log.Logger
	currentState *gstate.StateDB // Added to support state access in replay mode
}

// =============================================================================
// Interface Implementation
// =============================================================================

var _ core.ChainContext = (*remoteChainCtx)(nil)
var _ consensus.ChainHeaderReader = (*remoteChainCtx)(nil)

// Config is part of consensus.ChainHeaderReader
func (r *remoteChainCtx) Config() *params.ChainConfig {
	return r.cfg
}

// CurrentHeader is part of consensus.ChainHeaderReader
func (r remoteChainCtx) CurrentHeader() *types.Header {
	return r.hdr
}

// GetHeaderByNumber is part of consensus.ChainHeaderReader
func (r remoteChainCtx) GetHeaderByNumber(u uint64) *types.Header {
	if r.hdr.Number.Uint64() == u {
		return r.hdr
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Second*10)
	defer cancel()
	hdr, err := retry.Do[*types.Header](ctx, 10, retry.Exponential(), func() (*types.Header, error) {
		r.logger.Info("fetching block header", "num", u)
		return r.cl.HeaderByNumber(ctx, new(big.Int).SetUint64(u))
	})
	if err != nil {
		r.logger.Error("failed to get block header", "err", err, "num", u)
		return nil
	}
	if hdr == nil {
		r.logger.Warn("header not found", "num", u)
	}
	return hdr
}

// GetHeaderByHash is part of consensus.ChainHeaderReader
func (r remoteChainCtx) GetHeaderByHash(hash common.Hash) *types.Header {
	if r.hdr.Hash() == hash {
		return r.hdr
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Second*10)
	defer cancel()
	hdr, err := retry.Do[*types.Header](ctx, 10, retry.Exponential(), func() (*types.Header, error) {
		r.logger.Info("fetching block header", "hash", hash)
		return r.cl.HeaderByHash(ctx, hash)
	})
	if err != nil {
		r.logger.Error("failed to get block header", "err", err, "hash", hash)
		return nil
	}
	if hdr == nil {
		r.logger.Warn("header not found", "hash", hash)
	}
	return hdr
}

// GetTd is part of consensus.ChainHeaderReader
func (r remoteChainCtx) GetTd(hash common.Hash, number uint64) *big.Int {
	return big.NewInt(1)
}

// Engine is part of core.ChainContext
func (r remoteChainCtx) Engine() consensus.Engine {
	return r.consensusEng
}

// GetHeader is part of both consensus.ChainHeaderReader and core.ChainContext
func (r remoteChainCtx) GetHeader(hash common.Hash, u uint64) *types.Header {
	if r.hdr.Hash() == hash {
		return r.hdr
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Second*10)
	defer cancel()
	hdr, err := retry.Do[*types.Header](ctx, 10, retry.Exponential(), func() (*types.Header, error) {
		r.logger.Info("fetching block header", "hash", hash, "num", u)
		return r.cl.HeaderByNumber(ctx, new(big.Int).SetUint64(u))
	})
	if err != nil {
		r.logger.Error("failed to get block header", "err", err, "hash", hash, "num", u)
		return nil
	}
	if hdr == nil {
		r.logger.Warn("header not found", "hash", hash, "num", u)
	}
	if got := hdr.Hash(); got != hash {
		r.logger.Error("fetched incompatible header", "expectedHash", hash, "fetchedHash", got, "num", u)
	}
	return hdr
}

// =============================================================================
// Main Application Entry Point
// =============================================================================

func main() {
	flags := []cli.Flag{
		RPCFlag, StartFlag, EndFlag, OutPathFlag,
	}
	flags = append(flags, oplog.CLIFlags(EnvPrefix)...)

	app := cli.NewApp()
	app.Name = "op-replay"
	app.Usage = "Replay a range of blocks locally."
	app.Description = "Replay a range of blocks locally and verify state roots."
	app.Flags = cliapp.ProtectFlags(flags)
	app.Action = mainAction
	app.Writer = os.Stdout
	app.ErrWriter = os.Stderr
	err := app.Run(os.Args)
	if err != nil {
		_, _ = fmt.Fprintf(os.Stderr, "Application failed: %v", err)
		os.Exit(1)
	}
}

// =============================================================================
// Core Business Logic
// =============================================================================

// mainAction is the main application logic that orchestrates the block replay process
func mainAction(c *cli.Context) error {
	ctx := ctxinterrupt.WithCancelOnInterrupt(c.Context)
	logCfg := oplog.ReadCLIConfig(c)
	logger := oplog.NewLogger(c.App.Writer, logCfg)

	rpcEndpoint := c.String(RPCFlag.Name)
	start := c.Int(StartFlag.Name)
	end := c.Int(EndFlag.Name)
	outDir := c.Path(OutPathFlag.Name)

	if start > end {
		return fmt.Errorf("start block (%d) must be less than or equal to end block (%d)", start, end)
	}

	// Create output directory if it doesn't exist
	if outDir != "" {
		if err := os.MkdirAll(outDir, 0755); err != nil {
			return fmt.Errorf("failed to create output directory: %w", err)
		}
	}

	cl, err := rpc.DialContext(ctx, rpcEndpoint)
	if err != nil {
		return fmt.Errorf("failed to dial RPC: %w", err)
	}
	defer cl.Close()

	ethCl := ethclient.NewClient(cl)
	db := NewHybridRemoteDB(cl)

	var config *params.ChainConfig
	if err := cl.CallContext(ctx, &config, "debug_chainConfig"); err != nil {
		return fmt.Errorf("failed to fetch chain config: %w", err)
	}

	// Print chain configuration details
	logger.Info("Chain configuration loaded",
		"chain_id", config.ChainID,
		"homestead_block", config.HomesteadBlock,
		"dao_fork_block", config.DAOForkBlock,
		"dao_fork_support", config.DAOForkSupport,
		"eip150_block", config.EIP150Block,
		"eip155_block", config.EIP155Block,
		"eip158_block", config.EIP158Block,
		"byzantium_block", config.ByzantiumBlock,
		"constantinople_block", config.ConstantinopleBlock,
		"petersburg_block", config.PetersburgBlock,
		"istanbul_block", config.IstanbulBlock,
		"muir_glacier_block", config.MuirGlacierBlock,
		"berlin_block", config.BerlinBlock,
		"london_block", config.LondonBlock,
		"arrow_glacier_block", config.ArrowGlacierBlock,
		"gray_glacier_block", config.GrayGlacierBlock,
		"shanghai_time", config.ShanghaiTime,
		"cancun_time", config.CancunTime,
		"prague_time", config.PragueTime,
		"terminal_total_difficulty", config.TerminalTotalDifficulty,
		"optimism_config", config.Optimism != nil,
		"clique_config", config.Clique != nil,
	)

	// Print detailed Optimism configuration if available
	if config.Optimism != nil {
		logger.Info("Optimism configuration",
			"eip1559_elasticity", config.Optimism.EIP1559Elasticity,
			"eip1559_denominator", config.Optimism.EIP1559Denominator,
			"eip1559_denominator_canyon", config.Optimism.EIP1559DenominatorCanyon,
		)
	}

	// Print detailed Clique configuration if available
	if config.Clique != nil {
		logger.Info("Clique configuration",
			"epoch", config.Clique.Epoch,
			"period", config.Clique.Period,
		)
	}

	// Print additional hardfork timestamps if available
	if config.RegolithTime != nil {
		logger.Info("Regolith hardfork", "time", *config.RegolithTime)
	}
	if config.CanyonTime != nil {
		logger.Info("Canyon hardfork", "time", *config.CanyonTime)
	}
	if config.EcotoneTime != nil {
		logger.Info("Ecotone hardfork", "time", *config.EcotoneTime)
	}
	if config.FjordTime != nil {
		logger.Info("Fjord hardfork", "time", *config.FjordTime)
	}
	if config.InteropTime != nil {
		logger.Info("Interop hardfork", "time", *config.InteropTime)
	}

	logger.Info("Starting block range replay using RPC logic", "start", start, "end", end)

	// Get parent block of the start block
	parentBlock, err := ethCl.HeaderByNumber(ctx, big.NewInt(int64(start-1)))
	if err != nil {
		return fmt.Errorf("failed to fetch parent block %d: %w", start-1, err)
	}

	stateDB := gstate.NewDatabase(triedb.NewDatabase(db, &triedb.Config{
		Preimages: true,
	}), nil)

	currentState, err := gstate.New(parentBlock.Root, stateDB)
	if err != nil {
		return fmt.Errorf("failed to create initial state from block %d: %w", start-1, err)
	}
	logger.Info("Initialized state from parent block", "parent_number", start-1, "parent_root", parentBlock.Root)

	// Create remoteChainCtx once, outside the block processing loop
	// This matches the RPC pattern where these objects are created once and reused
	consensusEng := beacon.New(&beacon.OpLegacy{})
	chCtx := &remoteChainCtx{
		consensusEng: consensusEng,
		hdr:          parentBlock, // Will be updated for each block
		cfg:          config,
		cl:           ethCl,
		logger:       logger,
		currentState: currentState, // Will be updated for each block
	}

	// Stats tracking
	var (
		totalBlocks    = 0
		successBlocks  = 0
		mismatchBlocks = 0
		prevBlockHash  common.Hash // Track previous block hash for validation
	)

	// Process each block in the range
	for i := start; i <= end; i++ {
		logger.Info("Processing block", "number", i)

		// Fetch the block
		block, err := ethCl.BlockByNumber(ctx, big.NewInt(int64(i)))
		if err != nil {
			return fmt.Errorf("failed to fetch block %d: %w", i, err)
		}

		// Basic block validation
		if block.NumberU64() != uint64(i) {
			return fmt.Errorf("block number mismatch: expected %d, got %d", i, block.NumberU64())
		}

		// Verify parent block connection (except for the first block)
		if i > start {
			expectedParentHash := prevBlockHash
			if block.ParentHash() != expectedParentHash {
				return fmt.Errorf("parent hash mismatch at block %d: expected %s, got %s",
					i, expectedParentHash, block.ParentHash())
			}
		}

		// Create output file for this block's trace
		var outW io.Writer = io.Discard // Default to discard if no output directory
		if outDir != "" {
			outPath := fmt.Sprintf("%s/block_%d_trace.txt", outDir, i)
			file, err := os.OpenFile(outPath, os.O_CREATE|os.O_WRONLY|os.O_TRUNC, 0644)
			if err != nil {
				return fmt.Errorf("failed to create trace file for block %d: %w", i, err)
			}
			outW = file
		}

		// Convert types.Block to sources.RPCBlock for processing
		rpcBlock, err := convertToRPCBlock(block)
		if err != nil {
			return fmt.Errorf("failed to convert block %d to RPC format: %w", i, err)
		}

		// Process the block using the same logic as RPC node
		// This mimics the call chain: processBlockWithRpcLogic -> insertChain -> processBlock -> Process
		result, witness, newStateRoot, err := processBlockWithRpcLogic(logger, config, chCtx, rpcBlock, outW, outDir != "")

		// Export witness data for debugging if output directory is specified
		if outDir != "" && witness != nil {
			witnessDump := witness.ToExecutionWitness()
			out, err := json.MarshalIndent(witnessDump, "", "  ")
			if err != nil {
				logger.Error("failed to encode witness", "err", err)
			} else {
				witnessPath := fmt.Sprintf("%s/block_%d_witness.json", outDir, i)
				if err := os.WriteFile(witnessPath, out, 0644); err != nil {
					logger.Debug("Failed to write witness", "err", err, "path", witnessPath)
				} else {
					logger.Info("Witness data exported", "path", witnessPath)
				}
			}
		}

		// Close the file immediately after processing the block
		if outDir != "" {
			if file, ok := outW.(*os.File); ok {
				file.Close()
			}
		}

		if err != nil {
			logger.Error("Failed to process block", "number", i, "error", err)
			return fmt.Errorf("failed to process block %d: %w", i, err)
		}

		totalBlocks++

		// Compare state roots
		expectedRoot := block.Root()
		if newStateRoot != expectedRoot {
			mismatchBlocks++
			logger.Error("State root mismatch",
				"block_number", i,
				"expected_root", expectedRoot,
				"computed_root", newStateRoot,
				"gas_used", result.GasUsed)
		} else {
			successBlocks++
			logger.Info("Block processed successfully",
				"block_number", i,
				"state_root", newStateRoot,
				"gas_used", result.GasUsed,
				"tx_count", len(block.Transactions()))
		}

		// Update current state to the new state for next block
		// Note: We need to create a new StateDB instance because the current one is already committed
		currentState, err = gstate.New(newStateRoot, stateDB)
		if err != nil {
			return fmt.Errorf("failed to create state for next block: %w", err)
		}

		// Update the remoteChainCtx for the next block processing
		chCtx.currentState = currentState // Update state for next block
		chCtx.hdr = block.Header()        // Update header to current block (will be parent for next block)

		// Update previous block hash for next iteration
		prevBlockHash = block.Hash()
	}

	// Print summary
	logger.Info("Replay completed",
		"total_blocks", totalBlocks,
		"success_blocks", successBlocks,
		"mismatch_blocks", mismatchBlocks,
		"success_rate", fmt.Sprintf("%.2f%%", float64(successBlocks)/float64(totalBlocks)*100))

	if mismatchBlocks > 0 {
		return fmt.Errorf("state root mismatches detected in %d out of %d blocks", mismatchBlocks, totalBlocks)
	}

	return nil
}

// =============================================================================
// Block Processing Methods (RPC Logic Simulation)
// =============================================================================

// processBlockWithRpcLogic mimics ConsensusAPI.newPayload - the entry point for processing new blocks
// DIFFERENCES FROM RPC:
// - No versionedHashes, beaconRoot, requests parameters (not needed for replay)
// - Witness generation is handled in insertChain method (not directly here)
// - Directly processes the block instead of going through consensus API
func processBlockWithRpcLogic(logger log.Logger, config *params.ChainConfig, chCtx *remoteChainCtx, block *sources.RPCBlock,
	outW io.Writer, witness bool) (*core.ProcessResult, *stateless.Witness, common.Hash, error) {

	header := block.CreateGethHeader()

	vmCfg := vm.Config{Tracer: nil}

	// Set up tracing only if output is requested
	if outW != nil && outW != io.Discard {
		vmCfg.Tracer = logger2.NewJSONLogger(&logger2.Config{
			EnableMemory:     false,
			DisableStack:     false,
			DisableStorage:   false,
			EnableReturnData: false,
			Limit:            0,
			Overrides:        nil,
		}, outW)
	}

	// Witness creation and prefetcher management is now handled in insertChain method
	// to align with RPC logic and avoid duplication

	// insertChain mimics BlockChain.insertChain
	// DIFFERENCES FROM RPC:
	// - Witness generation is conditional based on the witness parameter
	// - Directly processes the block without blockchain validation
	result, witnessData, err := insertChain(logger, config, block, chCtx.currentState, vmCfg, chCtx, outW, witness)
	if err != nil {
		return nil, nil, common.Hash{}, err
	}

	// Commit the state changes and get the new state root
	newStateRoot, err := chCtx.currentState.Commit(uint64(block.Number), config.IsEIP158(header.Number), config.IsCancun(header.Number, header.Time))
	if err != nil {
		return nil, nil, common.Hash{}, fmt.Errorf("failed to commit state: %w", err)
	}

	return result, witnessData, newStateRoot, nil
}

// insertChain mimics BlockChain.insertChain - inserts a block into the chain
// DIFFERENCES FROM RPC:
// - Witness generation is conditional based on the witness parameter
// - Directly processes the block without blockchain validation
func insertChain(logger log.Logger, config *params.ChainConfig,
	block *sources.RPCBlock,
	statedb *gstate.StateDB, cfg vm.Config,
	chainCtx *remoteChainCtx, outW io.Writer, witness bool) (*core.ProcessResult, *stateless.Witness, error) {

	// Start a parallel signature recovery (matching RPC insertChain lines 1685-1686)
	// This is important for transaction signature validation
	gethBlock, err := convertRPCBlockToGethBlock(block)
	if err != nil {
		logger.Error("failed to convert RPC block to geth block", "err", err)
	} else {
		core.SenderCacher().RecoverFromBlocks(types.MakeSigner(config, new(big.Int).SetUint64(uint64(block.Number)), uint64(block.Time)), []*types.Block{gethBlock})
	}

	// Witness creation and prefetcher management (matching RPC insertChain lines 1850-1860)
	// DIFFERENCES FROM RPC:
	// - Witness generation is conditional and simplified for replay purposes
	// - Simplified prefetcher management
	var createdWitness *stateless.Witness
	if witness {
		// Create witness for debugging purposes
		header := block.CreateGethHeader()
		var err error
		createdWitness, err = stateless.NewWitness(header, chainCtx)
		if err != nil {
			logger.Error("failed to prepare witness data collector", "err", err)
		} else {
			// Start prefetcher for trie node path optimization
			statedb.StartPrefetcher("replay", createdWitness)
		}
	} else if config.IsByzantium(new(big.Int).SetUint64(uint64(block.Number))) {
		// Start prefetcher without witness for trie node path optimization
		statedb.StartPrefetcher("replay", nil)
	}

	// processBlock mimics BlockChain.processBlock - processes the block using StateProcessor.Process
	// DIFFERENCES FROM RPC:
	// - No block validation (already validated in replay context)
	// - Witness generation is handled in insertChain, not here
	// - Directly processes the block without blockchain context
	result, err := processBlock(logger, config, block, statedb, cfg, chainCtx, outW)
	if err != nil {
		return nil, nil, err
	}

	logger.Info("Completed block processing")
	if outW != nil {
		_, _ = fmt.Fprintf(outW, "# Completed block processing\n")
	}

	return result, createdWitness, nil
}

// processBlock mimics BlockChain.processBlock - processes the block using StateProcessor.Process
// DIFFERENCES FROM RPC:
// - No logging hooks (OnBlockStart/OnBlockEnd not needed in replay)
// - No stateless self-validation (not needed in replay)
// - No metrics collection (not needed in replay)
// - No database writes (not needed in replay)
// - Focus only on core processing and validation
func processBlock(logger log.Logger, config *params.ChainConfig, block *sources.RPCBlock, statedb *gstate.StateDB, cfg vm.Config, chainCtx *remoteChainCtx, outW io.Writer) (*core.ProcessResult, error) {

	// Process mimics StateProcessor.Process - processes all transactions and finalizes the block
	// DIFFERENCES FROM RPC:
	// - Witness generation is handled in insertChain, not here
	// - Directly processes consensus requests without blockchain context
	result, err := Process(logger, config, block, statedb, cfg, chainCtx)
	if err != nil {
		return nil, err
	}

	// SKIPPED: validateState logic (not needed for replay)
	// REASON: Replay tool's purpose is to replay blocks and verify state roots,
	// not to validate other block properties like gas usage, bloom filters, etc.

	return result, nil
}

// Process mimics StateProcessor.Process - processes all transactions and finalizes the block
// DIFFERENCES FROM RPC:
// - Witness generation is handled in insertChain, not here
// - Directly processes consensus requests without blockchain context
func Process(logger log.Logger, config *params.ChainConfig, block *sources.RPCBlock, statedb *gstate.StateDB, cfg vm.Config, chainCtx *remoteChainCtx) (*core.ProcessResult, error) {

	var (
		receipts    types.Receipts
		usedGas     = new(uint64)
		header      = block.CreateGethHeader()
		blockHash   = block.Hash
		blockNumber = new(big.Int).SetUint64(uint64(block.Number))
		allLogs     []*types.Log
		gp          = new(core.GasPool).AddGas(uint64(block.GasLimit))
		signer      = types.MakeSigner(config, header.Number, header.Time)
	)

	// Mutate the block and state according to any hard-fork specs (matching RPC StateProcessor.Process lines 68-70)
	if config.DAOForkSupport && config.DAOForkBlock != nil && config.DAOForkBlock.Cmp(blockNumber) == 0 {
		misc.ApplyDAOHardFork(statedb)
	}
	misc.EnsureCreate2Deployer(config, uint64(block.Time), statedb)

	// Create EVM block context and environment (matching RPC StateProcessor.Process)
	blockContext := core.NewEVMBlockContext(header, chainCtx, nil, config, statedb)
	vmenv := vm.NewEVM(blockContext, statedb, config, cfg)

	// Process beacon root and parent block hash
	if beaconRoot := block.ParentBeaconRoot; beaconRoot != nil {
		core.ProcessBeaconBlockRoot(*beaconRoot, vmenv)
	}
	if config.IsPrague(blockNumber, uint64(block.Time)) {
		core.ProcessParentBlockHash(block.ParentHash, vmenv)
	}

	logger.Info("Prepared EVM state")
	logger.Info("Processing transactions", "count", len(block.Transactions))

	// Iterate over and process the individual transactions
	for i, tx := range block.Transactions {
		logger.Info("Processing tx", "i", i, "hash", tx.Hash().Hex())
		msg, err := core.TransactionToMessage(tx, signer, header.BaseFee)
		if err != nil {
			return nil, fmt.Errorf("could not apply tx %d [%v]: %w", i, tx.Hash().Hex(), err)
		}
		statedb.SetTxContext(tx.Hash(), i)

		receipt, err := core.ApplyTransactionWithEVM(msg, gp, statedb, blockNumber, blockHash, tx, usedGas, vmenv)
		if err != nil {
			return nil, fmt.Errorf("could not apply tx %d [%v]: %w", i, tx.Hash().Hex(), err)
		}
		receipts = append(receipts, receipt)
		allLogs = append(allLogs, receipt.Logs...)
	}

	isIsthmus := config.IsIsthmus(uint64(block.Time))

	// Read requests if Prague is enabled.
	var requests [][]byte
	if config.IsPrague(blockNumber, uint64(block.Time)) && !isIsthmus {
		requests = [][]byte{}
		// EIP-6110
		if err := core.ParseDepositLogs(&requests, allLogs, config); err != nil {
			return nil, err
		}
		// EIP-7002
		if err := core.ProcessWithdrawalQueue(&requests, vmenv); err != nil {
			return nil, err
		}
		// EIP-7251
		if err := core.ProcessConsolidationQueue(&requests, vmenv); err != nil {
			return nil, err
		}
	}

	if isIsthmus {
		requests = [][]byte{}
	}

	// Finalize the block, applying any consensus engine specific extras (e.g. block rewards)
	chainCtx.Engine().Finalize(chainCtx, header, statedb, &types.Body{Transactions: block.Transactions, Withdrawals: *block.Withdrawals})

	return &core.ProcessResult{
		Receipts: receipts,
		Requests: requests,
		Logs:     allLogs,
		GasUsed:  *usedGas,
	}, nil
}

// =============================================================================
// Utility Functions
// =============================================================================

// convertToRPCBlock converts a standard geth block to RPC block format
func convertToRPCBlock(block *types.Block) (*sources.RPCBlock, error) {
	withdrawals := block.Withdrawals()
	return &sources.RPCBlock{
		RPCHeader: sources.RPCHeader{
			ParentHash:       block.ParentHash(),
			UncleHash:        block.UncleHash(),
			Coinbase:         block.Coinbase(),
			Root:             block.Root(),
			TxHash:           block.TxHash(),
			ReceiptHash:      block.ReceiptHash(),
			Bloom:            eth.Bytes256(block.Bloom()),
			Difficulty:       hexutil.Big(*block.Difficulty()),
			Number:           hexutil.Uint64(block.NumberU64()),
			GasLimit:         hexutil.Uint64(block.GasLimit()),
			GasUsed:          hexutil.Uint64(block.GasUsed()),
			Time:             hexutil.Uint64(block.Time()),
			Extra:            hexutil.Bytes(block.Extra()),
			MixDigest:        block.MixDigest(),
			Nonce:            types.EncodeNonce(block.Nonce()),
			BaseFee:          (*hexutil.Big)(block.BaseFee()),
			WithdrawalsRoot:  block.Header().WithdrawalsHash,
			BlobGasUsed:      (*hexutil.Uint64)(block.BlobGasUsed()),
			ExcessBlobGas:    (*hexutil.Uint64)(block.ExcessBlobGas()),
			ParentBeaconRoot: block.BeaconRoot(),
			Hash:             block.Hash(),
		},
		Transactions: block.Transactions(),
		Withdrawals:  &withdrawals,
	}, nil
}

// convertRPCBlockToGethBlock converts an RPCBlock back to a standard geth block
// This is the reverse operation of convertToRPCBlock
func convertRPCBlockToGethBlock(rpcBlock *sources.RPCBlock) (*types.Block, error) {
	header := rpcBlock.CreateGethHeader()

	// Create the block with header and transactions
	block := types.NewBlockWithHeader(header)
	var withdrawals []*types.Withdrawal
	if rpcBlock.Withdrawals != nil {
		withdrawals = *rpcBlock.Withdrawals
	}
	body := types.Body{Transactions: rpcBlock.Transactions, Withdrawals: withdrawals}
	block = block.WithBody(body)

	return block, nil
}
