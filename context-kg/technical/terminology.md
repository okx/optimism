---
name: "terminology"
description: "Domain term glossary for unified terminology across backend skills"
---
# Domain Terminology

| Term | Description | References |
|------|-------------|------------|
| AltDAConfig | Alternative Data Availability configuration enabling off-chain DA mode | op-node/rollup/types.go |
| BidirectionalTree | Flat claim tree with parent-child links for dispute analysis | op-dispute-mon/mon/types/types.go |
| BlockReplacement | Record of invalid block replaced with valid block at same height | op-supervisor/supervisor/types/types.go |
| BlockSeal | Block identity snapshot: hash, number, timestamp triplet | op-supervisor/supervisor/types/types.go |
| BondDistributionMode | Bond distribution strategy enum: Undecided, Normal, Refund, Legacy | op-challenger/game/fault/types/types.go |
| BuildJobID | Block building job identifier for parallel block construction | op-test-sequencer/sequencer/seqtypes/types.go |
| Bytes32 / Bytes48 / Bytes65 / Bytes96 / Bytes256 | Fixed-size byte arrays for hashes, commitments, signatures, proofs | op-service/eth/types.go |
| Claim | Complete dispute game claim with relationships and contract state | op-challenger/game/fault/types/types.go |
| ClaimData | Core dispute claim: value hash, bond, position in game tree | op-challenger/game/fault/types/types.go |
| Clock (Chess) | Duration and timestamp tracking for claim response deadlines | op-challenger/game/fault/types/types.go |
| CompressionAlgo | Data compression algorithm enum: zlib, brotli, brotli-9/10/11 | op-node/rollup/derive/types.go |
| Config (Rollup) | Main rollup configuration: batch settings, sender, derivation params | op-node/rollup/types.go |
| ContainsQuery | Cross-chain message validation query with timestamp, block, log index | op-supervisor/supervisor/types/types.go |
| CriticalErrorEvent | Unrecoverable error event triggering system abort | op-node/rollup/event.go |
| DerivedBlockRefPair | L1-to-L2 block mapping: source L1 ref with derived L2 ref | op-supervisor/supervisor/types/types.go |
| DeployConfig | Deployment configuration interface for governance and fork timing | op-core/predeploys/predeploy.go |
| EcotoneScalars | Ecotone fork fee scalar params: blob base fee and base fee scalars | op-service/eth/types.go |
| EngineAPIMethod | Engine API method identifier string type | op-service/eth/types.go |
| EnrichedClaim | Enhanced claim with resolved flag for monitoring | op-dispute-mon/mon/types/types.go |
| EnrichedGameData | Complete game analysis data with metadata, claims, bonds | op-dispute-mon/mon/types/types.go |
| Epoch | Block number as uint64 type for L2 block height | op-node/rollup/types.go |
| ErrorCode | RPC error code as int type for Engine API error classification | op-service/eth/types.go |
| ExecutePayloadStatus | Execution result status: VALID, INVALID, SYNCING | op-service/eth/types.go |
| ExecutingMessage | Cross-chain message execution record with chain, block, log, timestamp | op-supervisor/supervisor/types/types.go |
| ExecutionPayload | Execution layer block data: transactions, receipts, state root | op-service/eth/types.go |
| ExecutionPayloadEnvelope | Versioned payload container wrapping ExecutionPayload | op-service/eth/types.go |
| ForkchoiceState | Chain tip specification: head, safe, finalized block hashes | op-service/eth/types.go |
| GameMetadata | Dispute game identification: index, type, timestamp, proxy address | op-challenger/game/types/types.go |
| GameStatus | Fault dispute game status: InProgress, ChallengerWon, DefenderWon | op-challenger/game/types/types.go |
| Genesis | L1 and L2 genesis block information for chain initialization | op-node/rollup/types.go |
| Identifier | Message source identification: origin, block, log index, timestamp, chainID | op-supervisor/supervisor/types/types.go |
| IndexingEvent | Indexing node state update: reset, unsafe, derivation, exhaustion signals | op-supervisor/supervisor/types/types.go |
| InputError | Engine API input validation error with error code | op-service/eth/types.go |
| InvalidL2BlockNumberChallenge | L2 block number challenge proof with output response and header | op-challenger/game/fault/types/types.go |
| Message | Cross-chain message: identifier and payload hash pair | op-supervisor/supervisor/types/types.go |
| MessageChecksum | Hash-based message integrity check for access validation | op-supervisor/supervisor/types/types.go |
| OperatorFeeParams | Operator fee configuration for fee vault and balance tracking | op-service/eth/types.go |
| PayloadAttributes | Block building input: timestamp, fee recipient, withdrawals | op-service/eth/types.go |
| PayloadID | Engine payload identifier (8-byte Bytes8) | op-service/eth/types.go |
| PayloadInfo | Execution payload metadata: block time and hash info | op-service/eth/types.go |
| PayloadStatusV1 | Execution result: status, latest block hash, validation error | op-service/eth/types.go |
| Position | Claim position in game tree: depth and index coordinates | op-challenger/game/fault/types/types.go |
| Predeploy | Smart contract predeploy definition: address, proxy status, enablement | op-core/predeploys/predeploy.go |
| PreimageOracleData | Preimage data for oracle: key, data, offset, blob info | op-challenger/game/fault/types/types.go |
| Revision | Block height replacement version tracking invalidation | op-supervisor/supervisor/types/types.go |
| SafeHeadListener | Safe head update callback for advancement and reset notifications | op-node/rollup/iface.go |
| SafetyLevel | Data safety classification: finalized > safe > cross-unsafe > local-safe > unsafe > invalid | op-supervisor/supervisor/types/types.go |
| SizedBlock | Block with cached raw and DA size info for batching | op-batcher/batcher/types.go |
| StepCallData | Step execution parameters: claim index, attack flag, state/proof data | op-challenger/game/fault/types/types.go |
| SystemConfig | L1 blockchain configuration: fee scalars, batch sender, overhead | op-service/eth/types.go |
| ThrottleConfig | Throttling parameter limits: TX and block size bounds | op-batcher/batcher/throttler/types.go |
| ThrottleParams | Current throttling configuration: max sizes and intensity (0.0–1.0) | op-batcher/batcher/throttler/types.go |
| TraceAccessor | Interface for position-dependent trace data queries | op-challenger/game/fault/types/types.go |
| TraceProvider | Generic trace value provider for claim values and step data | op-challenger/game/fault/types/types.go |
| WithdrawalMessage | Cross-domain withdrawal interface: encode/decode/hash/storage ops | op-chain-ops/crossdomain/types.go |

## Synonyms

| Term A | Term B | Context |
|--------|--------|---------|
| Claim | EnrichedClaim | EnrichedClaim extends faultTypes.Claim with resolved flag |
| L1 | Source | In DerivedBlockRefPair, Source represents L1 origin |
| L2 | Derived | In DerivedBlockRefPair, Derived represents L2 block |
| Block | BlockSeal | BlockSeal is simplified identity snapshot of a full Block |
| Finalized | SafetyLevel.Finalized | Highest safety level constant |
| CrossSafe | SafetyLevel.Safe | Cross-chain validated safety level |
| LocalSafe | SafetyLevel.LocalSafe | Locally validated but not cross-chain verified |
| CrossUnsafe | SafetyLevel.CrossUnsafe | Cross-chain unverified safety level |
| LocalUnsafe | SafetyLevel.Unsafe | Base unverified safety level |
