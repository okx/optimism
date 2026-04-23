# op-replay-seq

`op-replay-seq` is a tool that uses sequencer logic to replay blocks, verifying block execution consistency by reusing the block building logic from the op-geth miner package.

## Purpose

The main objectives of this tool are:

1. **Verify Sequencer Logic Upgrades**: When upgrading sequencer logic, use this tool to verify that new sequencer logic doesn't affect state when replaying old blocks, preventing forks.

2. **State Consistency Verification**: Ensure block execution consistency by replaying blocks and comparing state roots.

3. **Debugging and Testing**: Help developers debug block execution issues and verify state transition correctness.

## How It Works

Unlike `op-replay`, `op-replay-seq` completely reuses sequencer's block building logic:

### Method Correspondence

The methods in `op-replay-seq` correspond one-to-one with miner methods:

| op-replay-seq Method | Miner Method | Purpose |
|----------------------|---------------|---------|
| `processBlockWithSequencerLogic()` | `buildPayload` | Block building entry point (skipped during replay) |
| `extractGenerateParams()` | - | Parameter extraction (replay-specific) |
| `ReplayMiner.generateWork()` | `Miner.generateWork()` | Main block generation logic |
| `ReplayMiner.prepareWork()` | `Miner.prepareWork()` | Prepare block building environment |
| `ReplayMiner.makeEnv()` | `Miner.makeEnv()` | Create execution environment |
| `ReplayMiner.commitTransaction()` | `Miner.commitTransaction()` | Commit individual transactions |
| `ReplayMiner.applyTransaction()` | `Miner.applyTransaction()` | Apply transactions to state |

### Call Chain

The tool follows the same call chain as the miner:

#### Miner Complete Call Chain
```
forkchoiceUpdated() -> buildPayload() -> generateWork() -> prepareWork() -> makeEnv() -> commitTransaction() -> applyTransaction()
```

#### op-replay-seq Call Chain
```
processBlockWithSequencerLogic() -> extractGenerateParams() -> generateWork() -> prepareWork() -> makeEnv() -> commitTransaction() -> applyTransaction()
```

**Note**:
- `op-replay-seq` starts from `processBlockWithSequencerLogic`, skipping the miner's API layer and `buildPayload` stage
- It directly extracts parameters from `extractGenerateParams`, then enters the same `generateWork` call chain as the miner
- This ensures that the replay process uses identical core logic while adapting to replay-specific requirements

### Key Features

1. **Parameter Extraction**: Extract `generateParams` from existing blocks, including timestamp, transaction list, gas limit, etc.
2. **State Reuse**: Use the same state processing logic as the miner
3. **Transaction Processing**: Reuse miner's transaction validation and application logic
4. **Consensus Logic**: Use the same consensus engine and FinalizeAndAssemble logic
5. **Object Reuse**: `ReplayMiner` and `remoteChainCtx` are created outside the block processing loop and reused for each block, improving performance

## Code Structure

### Core Data Structures

#### generateParams
- **Purpose**: Wraps various settings for block generation
- **Relationship with Miner**: Completely mirrors the miner's `generateParams` structure
- **Replay-Specific**: Extracted from existing blocks through `extractGenerateParams()`

#### environment
- **Purpose**: Contains all information needed for block generation
- **Relationship with Miner**: Completely mirrors the miner's `environment` structure
- **Replay-Specific**: State is passed in externally, not fetched from the chain

#### ReplayMiner
- **Purpose**: Simulates the miner structure for replaying blocks
- **Relationship with Miner**: Provides the same method interface as the miner
- **Replay-Specific**: State management adapted for replay scenarios

### Method Implementation Details

#### 1. generateWork Method

**Correspondence**: `ReplayMiner.generateWork()` ↔ `Miner.generateWork()`

**Reused Logic**:
- Calls `prepareWork` to prepare environment
- Sets up gas pool
- Processes forced-included transactions
- Collects consensus layer requests (EIP-6110, EIP-7002, EIP-7251)
- Calls `FinalizeAndAssemble`

**Differences from Miner**:
- **Skip `fillTransactions`**: Transactions are already determined during replay, no need to fill from transaction pool
- **Skip Interrupt Checks**: Replay doesn't need interruption mechanism
- **Reconstruct Block Header**: Reconstruct block header based on existing block information, then call FinalizeAndAssemble
- **Witness Export**: Records witness creation status at the end of the method for subsequent export

**Reason**: The purpose of replay tools is to verify execution of existing blocks, not to dynamically build transaction lists or handle interruptions. Witness export logic is now in the correct position, maintaining consistency with miner logic structure.

#### 2. prepareWork Method

**Correspondence**: `ReplayMiner.prepareWork()` ↔ `Miner.prepareWork()`

**Reused Logic**:
- Timestamp validation
- Block header construction
- EIP-1559 base fee processing
- EIP-4844 blob gas processing
- Consensus engine preparation
- Calls `makeEnv`

**Differences from Miner**:
- **Use External Parent Block**: Don't fetch parent block from chain, use the passed-in one
- **Use Existing Block's Extra**: Don't calculate Extra data, directly use the one in the block
- **Use Existing Block's BaseFee**: Don't calculate BaseFee, directly use the one in the block
- **Simplified Gas Limit Handling**: Directly use parent block's gas limit

**Reason**: During replay, we already have complete block information and can use it directly without recalculation. This ensures replay results are completely consistent with the original block.

#### 3. makeEnv Method

**Correspondence**: `ReplayMiner.makeEnv()` ↔ `Miner.makeEnv()`

**Reused Logic**:
- Witness data collection
- EVM environment creation
- State database configuration
- Prefetcher management

**Differences from Miner**:
- **Use External State**: Don't fetch state from chain, use the passed-in state
- **Simplified State Access**: Don't need complex historical state fetching logic
- **Witness Management**: Unified management of witness creation and prefetcher startup, avoiding duplicate logic

**Reason**: During replay, state is managed externally, not needing the miner's complex state fetching mechanism. Now witness creation and prefetcher management are all in `makeEnv`, completely consistent with miner logic.

#### 4. commitTransaction Method

**Correspondence**: `ReplayMiner.commitTransaction()` ↔ `Miner.commitTransaction()`

**Reused Logic**:
- Conditional transaction checks
- Calls `applyTransaction`
- Updates environment state

**Differences from Miner**:
- **Skip Interop Transaction Checks**: Don't need to check interop transactions during replay
- **Simplified Conditional Transaction Handling**: Retain core conditional check logic

**Reason**: During replay, miner's interop transaction validation isn't needed, but conditional transaction validation needs to be retained to ensure correctness.

#### 5. applyTransaction Method

**Correspondence**: `ReplayMiner.applyTransaction()` ↔ `Miner.applyTransaction()`

**Reused Logic**:
- State snapshot management
- Calls `core.ApplyTransaction`
- Error rollback handling

**Differences from Miner**:
- **Completely Identical**: No differences, logic is exactly the same

**Reason**: This is the lowest-level transaction application logic, which should be consistent between miner and replay.

## Refactored Logic Structure

### Witness and Prefetcher Management Optimization

**Problems Before Refactoring**:
- **Logic Duplication**: Both `processBlockWithSequencerLogic` and `makeEnv` created witness and started prefetcher
- **Unclear Responsibilities**: Witness export logic was in the wrong position, inconsistent with miner logic structure

**Structure After Refactoring**:
- **Unified Management**: Witness creation and prefetcher startup are all in the `makeEnv` method, completely consistent with miner logic
- **Correct Export**: Witness export logic is in `processBlockWithSequencerLogic`, processed after calling `generateWork`
- **Avoid Duplication**: Eliminated duplicate logic for witness creation and prefetcher startup

**Refactoring Advantages**:
1. **Logic Consistency**: Completely consistent with miner's `makeEnv` method
2. **Clear Responsibilities**: Each method's responsibilities are more clear
3. **Maintenance Friendly**: When miner logic updates, easy to identify code that needs synchronization
4. **Performance Optimization**: Avoid duplicate witness creation and prefetcher startup

## Replay-Specific Methods

### extractGenerateParams

**Purpose**: Extract miner-required parameters from existing blocks

**Relationship with Miner**: No corresponding method in miner, this is replay tool-specific

**Implementation Details**:
```go
func extractGenerateParams(block *types.Block) *generateParams {
    return &generateParams{
        timestamp:     block.Time(),
        forceTime:     true,        // Timestamp is fixed during replay
        parentHash:    block.ParentHash(),
        coinbase:      block.Coinbase(),
        random:        block.MixDigest(),
        withdrawals:   block.Withdrawals(),
        beaconRoot:    block.BeaconRoot(),
        noTxs:         true,        // Transactions are forced-included during replay
        txs:           block.Transactions(),
        gasLimit:      func() *uint64 { limit := block.GasLimit(); return &limit }(),
        // ... other fields
    }
}
```

**Note**: All parameters are extracted from the current block to be processed, no parent block information needed

## Usage

### Basic Usage

```bash
# Replay blocks 1000-1100
op-replay-seq --rpc <RPC_ENDPOINT> --start 1000 --end 1100

# Replay blocks with trace output
op-replay-seq --rpc <RPC_ENDPOINT> --start 1000 --end 1100 --out ./trace_output
```

### Parameter Description

- `--rpc`: RPC endpoint address (required)
- `--start`: Start block number (required)
- `--end`: End block number (required)
- `--out`: Output directory path (optional, for saving trace files)

### Environment Variables

You can set parameters via environment variables:

```bash
export OP_REPLAY_SEQ_RPC="http://localhost:8545"
export OP_REPLAY_SEQ_START=1000
export OP_REPLAY_SEQ_END=1100
export OP_REPLAY_SEQ_OUT="./trace_output"

op-replay-seq
```

## Output

### Console Output

The tool displays processing progress and results in the console:

```
INFO Starting block range replay using sequencer logic start=1000 end=1100
INFO Initialized state from parent block parent_number=999 parent_root=0x1234...
INFO Processing block number=1000
INFO Block processed successfully block_number=1000 state_root=0x5678... gas_used=15000000 tx_count=150
...
INFO Replay completed total_blocks=101 success_blocks=101 mismatch_blocks=0 success_rate=100.00%
```

### Trace Files (Optional)

If `--out` parameter is specified, the tool generates:

- `block_<N>_trace.txt`: Transaction execution traces
- `block_<N>_witness.json`: State access witness data

## Differences from op-replay

| Feature | op-replay | op-replay-seq |
|---------|-----------|---------------|
| **Execution Logic** | Verifier node logic | Sequencer node logic (reusing miner) |
| **Transaction Source** | From block parsing | From block parsing (reusing miner logic) |
| **State Processing** | Direct state transition | Reusing miner's state processing logic |
| **Method Correspondence** | Independent implementation | One-to-one correspondence with miner methods |
| **Purpose** | Verify block execution | Verify sequencer logic consistency |

## Code Reuse Analysis

### Completely Reused Logic

The following logic is completely reused from the miner:

1. **Block Header Preparation** (`prepareWork`)
   - Timestamp validation
   - EIP-1559 base fee calculation
   - EIP-4844 blob gas processing
   - Consensus engine preparation

2. **Environment Creation** (`makeEnv`)
   - EVM environment setup
   - State database configuration
   - Witness data collection

3. **Transaction Processing** (`commitTransaction` + `applyTransaction`)
   - Conditional transaction checks
   - Transaction application and rollback
   - Gas pool management

4. **Consensus Layer Requests** (`generateWork`)
   - EIP-6110 deposits
   - EIP-7002 withdrawals
   - EIP-7251 consolidations

### Replay-Specific Logic

The following logic is specific to the replay tool:

1. **Parameter Extraction** (`extractGenerateParams`)
   - Extract build parameters from existing blocks
   - Skip transaction pool logic

2. **State Management**
   - Use externally passed-in state
   - Skip miner's state fetching logic

3. **Output Processing**
   - Trace file generation
   - Witness data export

4. **Skipped Logic**
   - Transaction pool filling, interrupt checks, etc.

5. **Object Reuse Optimization**
   - `ReplayMiner` created outside the loop, reused for each block
   - `remoteChainCtx` created outside the loop, updated for each block
   - Avoid duplicate object creation, improve performance
   - Unified state management: `currentState` and `hdr` are both updated in the main loop, ensuring state consistency
   - Avoid duplicate parent block information fetching in functions, improve efficiency
   - Optimized state update timing: update state uniformly after processing blocks, prepare for the next block

### Skipped Miner Logic

1. **API Layer** (`forkchoiceUpdated`)
   - Don't need to handle external API calls during replay
   - Directly process existing blocks

2. **Block Building Entry** (`buildPayload`)
   - Don't need to build new blocks during replay
   - Directly use existing blocks for replay verification

## Use Cases

### 1. Sequencer Upgrade Verification

After upgrading sequencer logic, use this tool to replay historical blocks:

```bash
# Replay recent 1000 blocks to verify upgrade
op-replay-seq --rpc <RPC> --start <CURRENT-1000> --end <CURRENT>
```

### 2. Fork Detection

If state root mismatches are found during replay, it indicates potential fork risk:

```
ERROR State root mismatch block_number=1050 expected_root=0x1234... computed_root=0x5678...
```

### 3. Performance Testing

Test the performance and stability of new sequencer logic by replaying large numbers of blocks.

## Notes

1. **RPC Performance**: The tool needs to frequently call RPC interfaces, ensure the RPC endpoint has sufficient performance
2. **Memory Usage**: Pay attention to memory usage when replaying large numbers of blocks
3. **Network Stability**: Ensure stable network connection with the RPC endpoint
4. **Block Range**: Start with small ranges for testing, then gradually expand

## Troubleshooting

### Common Errors

1. **RPC Connection Failure**: Check RPC endpoint address and network connectivity
2. **Block Fetch Failure**: Confirm block number range is valid and RPC endpoint has corresponding data
3. **State Root Mismatch**: May indicate sequencer logic issues, need further investigation

### Debugging Tips

1. Use `--out` parameter to generate detailed trace files
2. Start with small block ranges for testing
3. Check log output for detailed error information
4. Verify chain configuration and hardfork settings

## Maintenance Guide

### When Miner Logic Updates

1. **Check Method Signatures**: Confirm that corresponding method signatures haven't changed
2. **Check Reused Logic**: Confirm that reused logic sections don't need updates
3. **Check Difference Logic**: Confirm that replay-specific logic doesn't need adjustment
4. **Test Validation**: Run tests to ensure functionality works correctly

### Adding New Features

1. **Determine Reusability**: Judge whether new features should exist in both miner and replay
2. **Method Correspondence**: If reusing, ensure method names and logic correspond
3. **Documentation Updates**: Update method correspondence table

## Advantages Summary

1. **Code Consistency**: Maintains consistency with miner logic
2. **Maintenance Convenience**: When miner updates, easy to identify code that needs synchronization
3. **Complete Functionality**: Reuses all core miner logic
4. **Debugging Friendly**: Can be compared directly with miner code for debugging
5. **Clear Structure**: Clearly distinguishes reused code from replay-specific code

## Contributing

Welcome to submit Issues and Pull Requests to improve this tool.
