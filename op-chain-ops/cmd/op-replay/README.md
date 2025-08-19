# op-replay

`op-replay` is a tool for local op-geth EVM debugging that replays existing blocks in a controlled local environment. It simulates the RPC node's block processing logic to verify state consistency and enable debugging with arbitrary tracers.

This tool helps debug if existing blocks may have forks due to new client logic by replaying them using the same processing chain as a standard Ethereum RPC node.

## Purpose

The main objectives of this tool are:

1. **State Root Verification**: Replay blocks and compare computed state roots with expected values to ensure execution consistency
2. **RPC Logic Validation**: Verify that the replay logic matches the standard RPC node processing chain
3. **Debugging and Testing**: Enable detailed EVM execution tracing and witness data collection for debugging
4. **Fork Detection**: Identify potential state inconsistencies that could lead to blockchain forks

## How It Works

`op-replay` completely mimics the RPC node's block processing logic:

### Method Correspondence

The methods in `op-replay` correspond one-to-one with RPC node methods:

| op-replay Method | RPC Node Method | Purpose |
|------------------|-----------------|---------|
| `processBlockWithRpcLogic()` | `ConsensusAPI.newPayload` | Entry point for new block processing |
| `insertChain()` | `BlockChain.insertChain` | Core method for inserting a chain of blocks |
| `processBlock()` | `BlockChain.processBlock` | Process a single block, including transaction execution |
| `Process()` | `StateProcessor.Process` | Execute transactions and finalize the block |

### Call Chain

The tool follows the same call chain as the RPC node:

#### RPC Node Complete Call Chain
```
ConsensusAPI.newPayload() -> BlockChain.insertChain() -> BlockChain.processBlock() -> StateProcessor.Process()
```

#### op-replay Call Chain
```
processBlockWithRpcLogic() -> insertChain() -> processBlock() -> Process()
```

**Note**:
- `op-replay` starts from `processBlockWithRpcLogic`, which mimics the RPC node's `newPayload` entry point
- It directly processes blocks using the same core logic as the RPC node
- This ensures that the replay process uses identical processing logic, while adapting to replay-specific requirements

### Key Features

1. **RPC Logic Simulation**: Completely mimics the RPC node's block processing flow
2. **State Management**: Uses the same state transition logic as the RPC node
3. **Transaction Processing**: Reuses the RPC node's transaction validation and execution logic
4. **Consensus Logic**: Uses the same consensus engine and finalization logic
5. **Object Reuse**: `remoteChainCtx` is created outside the block processing loop and reused for each block, improving performance

## Code Structure

### Core Data Structures

#### remoteChainCtx
- **Purpose**: Provides access to block headers and state for the state transition
- **RPC Correspondence**: Implements `core.ChainContext` and `consensus.ChainHeaderReader` interfaces
- **Replay Specific**: Manages state access in replay mode with external state management

### Method Implementation Details

#### 1. processBlockWithRpcLogic Method

**Correspondence**: `processBlockWithRpcLogic()` ↔ `ConsensusAPI.newPayload()`

**Reused Logic**:
- Calls `insertChain` to process the block
- Handles state commitment and state root computation
- Returns processing results and witness data

**Differences from RPC**:
- **No API Parameters**: Skips `versionedHashes`, `beaconRoot`, `requests` parameters (not needed for replay)
- **Direct Processing**: Directly processes the block instead of going through consensus API
- **Witness Handling**: Witness generation is handled in `insertChain`, not directly here

**Reason**: Replay tools don't need the full consensus API interface, but require the same core processing logic.

#### 2. insertChain Method

**Correspondence**: `insertChain()` ↔ `BlockChain.insertChain()`

**Reused Logic**:
- Parallel signature recovery using `core.SenderCacher().RecoverFromBlocks()`
- Witness creation and prefetcher management
- Calls `processBlock` for core processing

**Differences from RPC**:
- **Conditional Witness**: Witness generation is conditional based on the `witness` parameter
- **Direct Processing**: Directly processes the block without blockchain validation
- **Simplified Logic**: Focuses on core processing without RPC-specific overhead

**Reason**: Replay tools need the same core logic but don't require full blockchain validation.

#### 3. processBlock Method

**Correspondence**: `processBlock()` ↔ `BlockChain.processBlock()`

**Reused Logic**:
- Calls `Process` method for transaction execution
- Handles block processing results

**Differences from RPC**:
- **No Block Validation**: Block validation is already done in replay context
- **Witness Generation**: Witness generation is handled in `insertChain`
- **Direct Processing**: Directly processes the block without blockchain context

**Reason**: Replay tools focus on core processing and validation, not blockchain management.

#### 4. Process Method

**Correspondence**: `Process()` ↔ `StateProcessor.Process()`

**Reused Logic**:
- Hardfork specifications application (DAO fork, Create2 deployer)
- EVM block context and environment creation
- Transaction processing and receipt generation
- Consensus layer requests processing (EIP-6110, EIP-7002, EIP-7251)
- Block finalization using consensus engine

**Differences from RPC**:
- **No Witness Generation**: Witness generation is handled in `insertChain`
- **Direct Processing**: Directly processes consensus requests without blockchain context

**Reason**: This is the core transaction processing logic that should be identical between RPC and replay.

## Code Reuse Analysis

### Completely Reused Logic

The following logic is completely reused from the RPC node:

1. **Block Processing** (`processBlock`)
   - Transaction execution flow
   - Receipt generation
   - Gas management

2. **State Processing** (`Process`)
   - Hardfork specifications
   - EVM environment setup
   - Transaction application

3. **Transaction Handling** (`insertChain`)
   - Parallel signature recovery
   - Witness creation and prefetcher management

4. **Consensus Logic**
   - Consensus engine finalization
   - EIP-6110, EIP-7002, EIP-7251 processing

### Replay-Specific Logic

The following logic is specific to the replay tool:

1. **State Management**
   - External state initialization and updates
   - StateDB lifecycle management
   - State root comparison and validation

2. **Output Processing**
   - Trace file generation
   - Witness data export
   - Performance mode (no file output)

3. **Skipped Logic**
   - Full blockchain validation
   - RPC-specific overhead
   - Consensus API interface

4. **Object Reuse Optimization**
   - `remoteChainCtx` created outside the loop and reused for each block
   - Efficient state database handling
   - Conditional witness generation and tracing

### Skipped RPC Logic

1. **API Layer** (`ConsensusAPI.newPayload`)
   - Replay tools don't need to handle external API calls
   - Directly process existing blocks

2. **Blockchain Management**
   - No need for full blockchain validation
   - Focus on core processing logic

## Usage

### Basic Usage

```bash
# Replay blocks 1000-1100
op-replay --rpc <RPC_ENDPOINT> --start 1000 --end 1100

# Replay blocks with trace and witness generation
op-replay --rpc <RPC_ENDPOINT> --start 1000 --end 1100 --out ./trace_output
```

### Parameter Description

- `--rpc`: RPC endpoint address (required)
- `--start`: Start block number (required)
- `--end`: End block number (required)
- `--out`: Output directory path (optional, for saving trace files)

### Environment Variables

You can set parameters via environment variables:

```bash
export OP_REPLAY_RPC="http://localhost:8545"
export OP_REPLAY_START=1000
export OP_REPLAY_END=1100
export OP_REPLAY_OUT="./trace_output"

op-replay
```

## Output

### Console Output

The tool displays processing progress and results in the console:

```
INFO Starting block range replay using RPC logic start=1000 end=1100
INFO Initialized state from parent block parent_number=999 parent_root=0x1234...
INFO Processing block number=1000
INFO Block processed successfully block_number=1000 state_root=0x5678... gas_used=15000000 tx_count=150
...
INFO Replay completed total_blocks=101 success_blocks=101 mismatch_blocks=0 success_rate=100.00%
```

### Trace Files (Optional)

If `--out` parameter is specified, the tool generates:

- `block_{N}_trace.txt`: Transaction execution traces
- `block_{N}_witness.json`: State access witness data

## Differences from op-replay-seq

| Feature | op-replay | op-replay-seq |
|---------|-----------|---------------|
| **Execution Logic** | RPC node logic | Sequencer logic (reusing miner) |
| **Transaction Source** | From block parsing | From block parsing (reusing miner logic) |
| **State Processing** | Direct state transition | Reusing miner's state processing logic |
| **Method Correspondence** | Independent implementation | One-to-one correspondence with miner methods |
| **Purpose** | Verify block execution | Verify sequencer logic consistency |

## Use Cases

### 1. State Root Verification

Verify that replayed blocks produce the same state roots as the original blockchain:

```bash
# Replay recent 1000 blocks to verify state consistency
op-replay --rpc <RPC> --start <CURRENT-1000> --end <CURRENT>
```

### 2. RPC Logic Testing

Test new RPC node logic by replaying historical blocks:

```bash
# Test new logic on historical blocks
op-replay --rpc <RPC> --start 1000000 --end 1000100
```

### 3. Performance Testing

Test RPC node performance improvements:

```bash
# Performance test without file output
op-replay --rpc <RPC> --start 1000000 --end 1000100
```

## Notes

1. **RPC Performance**: The tool needs to frequently call RPC interfaces, ensure the RPC endpoint has sufficient performance
2. **Memory Usage**: Pay attention to memory usage when replaying large numbers of blocks
3. **Network Stability**: Ensure stable network connection with the RPC endpoint
4. **Block Range**: Start with small ranges for testing, then gradually expand

## Troubleshooting

### Common Errors

1. **RPC Connection Failure**: Check RPC endpoint address and network connectivity
2. **Block Fetch Failure**: Confirm block number range is valid and RPC endpoint has corresponding data
3. **State Root Mismatch**: May indicate RPC logic issues, need further investigation

### Debugging Tips

1. Use `--out` parameter to generate detailed trace files
2. Start with small block ranges for testing
3. Check log output for detailed error information
4. Verify chain configuration and hardfork settings

## Maintenance Guide

### When RPC Logic Updates

1. **Check Method Signatures**: Confirm that corresponding method signatures haven't changed
2. **Review Reused Logic**: Ensure reused logic sections don't need updates
3. **Check Difference Logic**: Confirm replay-specific logic doesn't need adjustment
4. **Test Validation**: Run tests to ensure functionality works correctly

### Adding New Features

1. **Determine Reusability**: Judge whether new features should exist in both RPC and replay
2. **Method Correspondence**: If reusing, ensure method names and logic correspond
3. **Documentation Updates**: Update method correspondence table

## Advantages Summary

1. **Code Consistency**: Maintains consistency with RPC node logic
2. **Maintenance Convenience**: Easy to identify code that needs synchronization when RPC updates
3. **Complete Functionality**: Reuses all core RPC processing logic
4. **Debugging Friendly**: Can be compared directly with RPC node code for debugging
5. **Clear Structure**: Clearly distinguishes reused code from replay-specific code

## Contributing

Welcome to submit Issues and Pull Requests to improve this tool.
