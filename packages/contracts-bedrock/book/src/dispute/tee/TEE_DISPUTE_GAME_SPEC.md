# TEE Dispute Game Specification

## 1. Overview

TeeDisputeGame is a dispute game contract for the OP Stack that replaces interactive bisection (FaultDisputeGame) and ZK proof verification (hypothetical OPSuccinctFaultDisputeGame) with **TEE (Trusted Execution Environment) ECDSA signature verification** for batch state transition proofs.

**Purpose:** Enable faster, cheaper dispute resolution by leveraging AWS Nitro Enclave attestations. A TEE executor runs the state transition inside an enclave, signs the result with a registered enclave key, and the on-chain contract verifies the ECDSA signature against the registered enclave set.

**How it fits in the OP Stack:**
- Uses the standard `DisputeGameFactory` for game creation (via Clone pattern)
- Integrates with `AnchorStateRegistry` for anchor state management, finalization, and validity checks
- Uses `BondDistributionMode` (NORMAL/REFUND) from the shared Types library
- Implements `IDisputeGame` interface for compatibility with `OptimismPortal` and other OP infrastructure
- Game type constant: `1960`

---

## 2. Architecture

### Contract Relationship Diagram

```
                          +---------------------------+
                          |   DisputeGameFactory      |
                          |  (creates Clone proxies)  |
                          +-----+----------+----------+
                                |          |
                       create() |          | gameAtIndex()
                                v          v
                    +--------------------------+
                    |     TeeDisputeGame       |
                    |   (Clone proxy instance) |
                    +----+-------+-------+-----+
                         |       |       |
              +----------+  +----+----+  +----------+
              v             v         v             v
    +----------------+ +---------+ +------------------+ +---------------+
    | PROPOSER /     | | Anchor  | | TeeProofVerifier | | IDisputeGame  |
    | CHALLENGER     | | State   | | (enclave ECDSA   | | (interface)   |
    | (immutable     | | Registry| |  verification)   | |               |
    |  addresses)    | |         | +-------+----------+ +---------------+
    +----------------+ +---------+         |
                                           v
                                  +------------------+
                                  | IRiscZeroVerifier|
                                  | (enclave         |
                                  |  registration    |
                                  |  only)           |
                                  +------------------+
```

### Immutables (set in constructor, shared across all clones)

| Immutable               | Type                    | Description                                      |
|--------------------------|-------------------------|--------------------------------------------------|
| `GAME_TYPE`              | `GameType`              | Always `GameType.wrap(1960)`                     |
| `MAX_CHALLENGE_DURATION` | `Duration`              | Window for challenger to post challenge           |
| `MAX_PROVE_DURATION`     | `Duration`              | Window for prover to submit proof after challenge |
| `DISPUTE_GAME_FACTORY`   | `IDisputeGameFactory`   | Factory that created this game                   |
| `TEE_PROOF_VERIFIER`     | `ITeeProofVerifier`     | TEE signature verification contract              |
| `CHALLENGER_BOND`        | `uint256`               | Fixed bond amount required to challenge           |
| `ANCHOR_STATE_REGISTRY`  | `IAnchorStateRegistry`  | Anchor state management                          |
| `PROPOSER`               | `address`               | Single allowed proposer address                  |
| `CHALLENGER`             | `address`               | Single allowed challenger address                |

### Clone (CWIA) Data Layout

| Offset | Size    | Field             | Description                                         |
|--------|---------|-------------------|-----------------------------------------------------|
| 0x00   | 20 bytes| `gameCreator`     | Address of the game creator                         |
| 0x14   | 32 bytes| `rootClaim`       | The proposed output root claim                      |
| 0x34   | 32 bytes| `l1Head`          | L1 block hash at game creation                      |
| 0x54   | 32 bytes| `l2SequenceNumber`| Target L2 block number                              |
| 0x74   | 4 bytes | `parentIndex`     | Index of parent game in factory (0xFFFFFFFF = root) |
| 0x78   | 32 bytes| `blockHash`       | L2 block hash component of rootClaim                |
| 0x98   | 32 bytes| `stateHash`       | L2 state hash component of rootClaim                |

Total extraData: 100 bytes (0x64) starting at offset 0x54. Expected calldata size: `0xBE` (190 bytes).

### State Variables

| Variable                          | Type                          | Description                                  |
|------------------------------------|-------------------------------|----------------------------------------------|
| `createdAt`                        | `Timestamp`                   | Block timestamp of initialization            |
| `resolvedAt`                       | `Timestamp`                   | Block timestamp of resolution                |
| `status`                           | `GameStatus`                  | IN_PROGRESS / DEFENDER_WINS / CHALLENGER_WINS|
| `proposer`                         | `address`                     | `tx.origin` of the initialize call           |
| `initialized`                      | `bool`                        | Re-initialization guard                      |
| `claimData`                        | `ClaimData`                   | Single claim (not an array like FDG)         |
| `normalModeCredit[addr]`           | `mapping(address => uint256)` | Bonds distributed to winners                 |
| `refundModeCredit[addr]`           | `mapping(address => uint256)` | Bonds refunded to original depositors        |
| `startingOutputRoot`               | `Proposal`                    | Starting anchor (root hash + block number)   |
| `wasRespectedGameTypeWhenCreated`  | `bool`                        | Was this game type respected at creation?    |
| `bondDistributionMode`             | `BondDistributionMode`        | UNDECIDED / NORMAL / REFUND                  |

---

## 3. Game Lifecycle

### State Machine

```
                              initialize()
                                  |
                                  v
                          +---------------+
                          |  Unchallenged  |  <-- deadline = now + MAX_CHALLENGE_DURATION
                          +-------+-------+
                                  |
               +------------------+------------------+
               |                                     |
          challenge()                         deadline expires
               |                                     |
               v                                     v
        +-------------+                     resolve() -> DEFENDER_WINS
        |  Challenged  |  <-- deadline = now + MAX_PROVE_DURATION
        +------+------+
               |
    +----------+----------+
    |                     |
  prove()           deadline expires
    |                     |
    v                     v
+---------------------------+    resolve() -> CHALLENGER_WINS
| ChallengedAndValid       |
| ProofProvided            |
+----------+---------------+
           |
           v
    resolve() -> DEFENDER_WINS
```

If `prove()` is called while Unchallenged:

```
  Unchallenged --> prove() --> UnchallengedAndValidProofProvided --> resolve() --> DEFENDER_WINS
```

### ProposalStatus Transitions

| From                               | Action      | To                                    |
|--------------------------------------|-------------|---------------------------------------|
| `Unchallenged`                      | `challenge()`| `Challenged`                         |
| `Unchallenged`                      | `prove()`   | `UnchallengedAndValidProofProvided`   |
| `Challenged`                        | `prove()`   | `ChallengedAndValidProofProvided`     |
| Any (on resolve)                    | `resolve()` | `Resolved`                           |

### GameStatus Transitions

| Condition                                         | Result             |
|----------------------------------------------------|--------------------|
| Parent game resolved as CHALLENGER_WINS            | CHALLENGER_WINS    |
| Unchallenged + deadline expired                    | DEFENDER_WINS      |
| Challenged + deadline expired (no proof)           | CHALLENGER_WINS    |
| UnchallengedAndValidProofProvided                  | DEFENDER_WINS      |
| ChallengedAndValidProofProvided                    | DEFENDER_WINS      |

### `gameOver()` Condition

```solidity
gameOver_ = claimData.deadline.raw() < block.timestamp || claimData.prover != address(0);
```

The game is "over" (no more interactions) when the deadline passes OR a valid proof is submitted.

---

## 4. Initialization

`initialize()` is called by the `DisputeGameFactory` immediately after cloning.

### Validation Checks (in order)

1. **Not already initialized** -- reverts `AlreadyInitialized`
2. **Caller is the factory** -- reverts `IncorrectDisputeGameFactory`
3. **tx.origin == PROPOSER** -- reverts `BadAuth`
4. **Calldata size is exactly 0xBE (190 bytes)** -- reverts with selector `0x9824bdab` (BadExtraData)
5. **rootClaim == keccak256(abi.encode(blockHash, stateHash))** -- reverts `RootClaimMismatch`
6. **Parent game validation** (if parentIndex != type(uint32).max):
   - Parent game type must match `GAME_TYPE`
   - Parent must be respected, not blacklisted, not retired (via ASR)
   - Parent must not have status CHALLENGER_WINS
7. **l2SequenceNumber > startingOutputRoot.l2SequenceNumber** -- reverts `UnexpectedRootClaim`

### Parent Game Resolution

- If `parentIndex == type(uint32).max`: uses anchor state from `AnchorStateRegistry.anchors(GAME_TYPE)`
- Otherwise: reads `rootClaim` and `l2SequenceNumber` from the parent TeeDisputeGame proxy

### Initialization Side Effects

- Sets `claimData` with deadline = `now + MAX_CHALLENGE_DURATION`
- Records `proposer = tx.origin`
- Credits `refundModeCredit[proposer] += msg.value` (the bond)
- Sets `createdAt` and `wasRespectedGameTypeWhenCreated`

---

## 5. Challenge-Prove Model

### Single-Round vs Multi-Round

| Aspect                    | TeeDisputeGame                              | FaultDisputeGame                                  |
|----------------------------|----------------------------------------------|---------------------------------------------------|
| Dispute model              | Single-round: challenge + prove              | Multi-round interactive bisection + step           |
| Claim structure            | Single `ClaimData` struct                    | Append-only `ClaimData[]` array (DAG)             |
| Challenge mechanism        | `challenge()` with fixed bond                | `move()` (attack/defend) with position-based bonds |
| Proof                      | TEE ECDSA batch signatures                   | On-chain VM single instruction step                |
| Resolution complexity      | O(1) - single resolve call                   | O(n) - bottom-up subgame resolution               |
| Time model                 | Fixed deadlines (challenge window, prove window) | Chess clock with extensions                     |

### challenge()

- Requires: `Unchallenged` status, whitelisted challenger, game not over, exact bond amount
- Effects: sets `counteredBy`, transitions to `Challenged`, resets deadline to `now + MAX_PROVE_DURATION`
- Bond: credited to `refundModeCredit[challenger]`

### prove()

- Can be called in both `Unchallenged` and `Challenged` states (early proving is by design — TEE is trusted)
- Requires game status `IN_PROGRESS` (cannot prove after resolution)
- Accepts ABI-encoded `BatchProof[]` array
- Verifies chain of batch proofs (see Section 6)
- Records `prover = msg.sender`
- No bond required from prover
- Only the proposer can call `prove()` (`if (msg.sender != proposer) revert BadAuth()`) — this prevents frontrunning attacks where a third party could steal prover credit by submitting observed proof data
- Once proved, `gameOver()` returns true, which blocks further `challenge()` calls — this is intentional since a valid TEE proof confirms the claim is correct

---

## 6. Batch Proof Verification

### BatchProof Structure

```solidity
struct BatchProof {
    bytes32 startBlockHash;
    bytes32 startStateHash;
    bytes32 endBlockHash;
    bytes32 endStateHash;
    uint256 l2Block;
    bytes signature;    // 65 bytes ECDSA (r + s + v)
}
```

### Verification Steps

For a `BatchProof[] proofs` array:

1. **Start anchor**: `keccak256(abi.encode(proofs[0].startBlockHash, proofs[0].startStateHash))` must equal `startingOutputRoot.root`
2. **Chain continuity** (for i > 0): `proofs[i].start{Block,State}Hash == proofs[i-1].end{Block,State}Hash`
3. **Monotonic blocks**: `proofs[i].l2Block > prevBlock` (starting from `startingOutputRoot.l2SequenceNumber`)
4. **TEE signature**: For each batch, compute `batchDigest = keccak256(abi.encode(startBlockHash, startStateHash, endBlockHash, endStateHash, l2Block))` and call `TEE_PROOF_VERIFIER.verifyBatch(batchDigest, signature)`
5. **End anchor**: `keccak256(abi.encode(proofs[last].endBlockHash, proofs[last].endStateHash))` must equal `rootClaim()`
6. **Final block**: `proofs[last].l2Block` must equal `l2SequenceNumber()`

### batchDigest Computation

```
batchDigest = keccak256(abi.encode(
    startBlockHash,   // bytes32
    startStateHash,   // bytes32
    endBlockHash,     // bytes32
    endStateHash,     // bytes32
    l2Block           // uint256
))
```

### TeeProofVerifier.verifyBatch()

1. `ECDSA.tryRecover(digest, signature)` to recover signer
2. Check `registeredEnclaves[recovered].registeredAt != 0`
3. Return signer address

### Enclave Registration (TeeProofVerifier.register())

- Owner-only function
- Verifies RISC Zero ZK proof of AWS Nitro attestation
- Parses journal: timestamp, PCR hash, root key, secp256k1 public key, user data
- Validates root key matches AWS Nitro official root
- Extracts Ethereum address from public key
- Stores `EnclaveInfo{pcrHash, registeredAt}` mapping

---

## 7. Bond Economics

### Bond Flow

| Actor     | When              | Amount              | Credited To               |
|-----------|-------------------|----------------------|---------------------------|
| Proposer  | `initialize()`    | `msg.value` (any)    | `refundModeCredit[proposer]` |
| Challenger| `challenge()`     | `CHALLENGER_BOND`    | `refundModeCredit[challenger]` |

### Bond Distribution on resolve()

| ProposalStatus                       | Winner           | Distribution                                                                                     |
|---------------------------------------|------------------|--------------------------------------------------------------------------------------------------|
| Unchallenged (deadline expired)      | Proposer (DEFENDER_WINS) | `normalModeCredit[proposer] = balance`                                                   |
| Challenged (deadline expired, no proof) | Challenger (CHALLENGER_WINS) | `normalModeCredit[challenger] = balance`                                          |
| UnchallengedAndValidProofProvided    | Proposer (DEFENDER_WINS) | `normalModeCredit[proposer] = balance`                                                   |
| ChallengedAndValidProofProvided      | Proposer (DEFENDER_WINS) | `normalModeCredit[proposer] = balance` (proposer gets all bonds since only proposer can prove) |
| Parent game CHALLENGER_WINS (child challenged) | Challenger (CHALLENGER_WINS) | `normalModeCredit[challenger] = balance`                                |
| Parent game CHALLENGER_WINS (child unchallenged) | Proposer refunded (CHALLENGER_WINS) | `normalModeCredit[proposer] = balance`                           |

### closeGame() and BondDistributionMode

Before credits can be claimed, `closeGame()` determines the distribution mode:

1. Check `ANCHOR_STATE_REGISTRY.isGameFinalized()` (resolved + finality delay elapsed)
2. Try to set this game as the new anchor state
3. Check `ANCHOR_STATE_REGISTRY.isGameProper()` (registered, not blacklisted, not retired, not paused)
4. If proper: `NORMAL` mode (winners get bonds). If not: `REFUND` mode (everyone gets their deposit back)

### claimCredit()

1. Calls `closeGame()` (idempotent)
2. Reads credit based on `bondDistributionMode`
3. Zeroes both credit mappings
4. Transfers ETH via low-level `call`

**Key difference from FaultDisputeGame:** FDG uses `DelayedWETH` (deposit/unlock/withdraw pattern) for bond custody. TeeDisputeGame holds ETH directly in the contract (`address(this).balance`).

---

## 8. Parent-Child Chaining

### How Games Reference Parents

- `parentIndex` is a `uint32` stored in CWIA calldata at offset `0x74`
- `type(uint32).max` (0xFFFFFFFF) means "no parent" (uses anchor state from ASR)
- Any other value is an index into `DisputeGameFactory.gameAtIndex()`

### Parent Validation (in initialize())

```
parentIndex != type(uint32).max:
  1. parentGameType must == GAME_TYPE (cross-type isolation)
  2. Parent must be respected (ASR.isGameRespected)
  3. Parent must not be blacklisted (ASR.isGameBlacklisted)
  4. Parent must not be retired (ASR.isGameRetired)
  5. Parent must not be CHALLENGER_WINS

  startingOutputRoot = {
    l2SequenceNumber: parent.l2SequenceNumber(),
    root: Hash.wrap(parent.rootClaim().raw())
  }
```

### Cross-Chain Isolation

The `GameType` check (`parentGameType == GAME_TYPE`) ensures TEE games can only chain to other TEE games. This prevents a compromised FaultDisputeGame from being used as a starting point for a TEE chain.

### resolve() Parent Dependency

- If parent exists and is still `IN_PROGRESS`: `resolve()` reverts with `ParentGameNotResolved`
- If parent resolved as `CHALLENGER_WINS`: child automatically resolves as `CHALLENGER_WINS`. If the child was challenged, the challenger gets all bonds. If the child was never challenged, the proposer's bond is refunded.
- If parent resolved as `DEFENDER_WINS` (or no parent): normal resolution logic applies

---

## 9. AnchorStateRegistry Integration

### Functions Used by TeeDisputeGame

| ASR Function                | Where Used              | Purpose                                        |
|------------------------------|-------------------------|-------------------------------------------------|
| `anchors(GAME_TYPE)`         | `initialize()`          | Get starting anchor when no parent              |
| `respectedGameType()`        | `initialize()`          | Check if this game type is respected at creation|
| `isGameRespected(proxy)`     | `initialize()`          | Validate parent game                           |
| `isGameBlacklisted(proxy)`   | `initialize()`          | Validate parent game                           |
| `isGameRetired(proxy)`       | `initialize()`          | Validate parent game                           |
| `isGameFinalized(this)`      | `closeGame()`           | Check resolution + finality delay              |
| `setAnchorState(this)`       | `closeGame()`           | Try to update anchor game                      |
| `isGameProper(this)`         | `closeGame()`           | Determine NORMAL vs REFUND mode                |

### Anchor State Update Flow

```
claimCredit() -> closeGame() -> ASR.setAnchorState(this) [try/catch]
                             -> ASR.isGameProper(this)
                             -> set bondDistributionMode
```

`setAnchorState` will succeed only if:
- Game claim is valid (proper + respected + finalized + DEFENDER_WINS)
- Game's `l2SequenceNumber` > current anchor's block number

### rootClaim Format

```
rootClaim = keccak256(abi.encode(blockHash, stateHash))
```

This differs from FaultDisputeGame where rootClaim is an output root hash directly. The ASR stores this combined hash as the anchor root.

---

## 10. Access Control

### Inline Immutable Pattern

TeeDisputeGame uses simple immutable addresses for access control, matching PermissionedDisputeGame's pattern:

| Role        | Storage                       | Check                                      |
|-------------|-------------------------------|---------------------------------------------|
| Proposer    | `address internal immutable PROPOSER` | `initialize()`: `if (tx.origin != PROPOSER) revert BadAuth();` |
| Challenger  | `address internal immutable CHALLENGER` | `challenge()`: `if (msg.sender != CHALLENGER) revert BadAuth();` |

Both addresses are set in the constructor and shared across all Clone instances. No external contract calls are needed for access control checks.

### TeeProofVerifier Roles

| Role     | Function                | Description                                        |
|----------|-------------------------|----------------------------------------------------|
| Owner    | `register(seal, journal)` | Register enclave after ZK proof verification     |
| Owner    | `revoke(enclaveAddress)` | Remove enclave registration                       |
| Owner    | `transferOwnership()`    | Transfer ownership                                |
| Anyone   | `verifyBatch()`          | Verify batch signature (view, no state change)    |

### Comparison: TeeDisputeGame vs PermissionedDisputeGame Access Control

| Aspect                    | TeeDisputeGame                              | PermissionedDisputeGame                    |
|----------------------------|----------------------------------------------|---------------------------------------------|
| Proposer check             | `tx.origin != PROPOSER` (single address)     | `tx.origin == PROPOSER` (single address)   |
| Challenger check           | `msg.sender != CHALLENGER` (single address)  | `msg.sender == PROPOSER \|\| msg.sender == CHALLENGER` |
| Multiple proposers?        | No (single immutable address)                | No (single immutable address)              |
| Permissionless fallback?   | No                                           | No                                         |
| Role management            | Immutable constructor params                 | Immutable constructor params               |

---

## 11. Comparison with Existing Contracts

### Feature Comparison Table

| Feature                       | TeeDisputeGame                        | FaultDisputeGame                       | PermissionedDisputeGame                |
|--------------------------------|----------------------------------------|-----------------------------------------|-----------------------------------------|
| **Proof mechanism**            | TEE ECDSA batch signatures            | Interactive bisection + VM step         | Interactive bisection + VM step (gated) |
| **Dispute rounds**             | 1 (challenge + prove)                 | Many (bisection tree)                   | Many (bisection tree, permissioned)     |
| **Claims**                     | Single ClaimData struct               | Append-only ClaimData[] array           | Append-only ClaimData[] array           |
| **Resolution**                 | O(1), single resolve()               | O(n), bottom-up resolveClaim()          | O(n), bottom-up resolveClaim()          |
| **Bond custody**               | Native ETH in contract                | DelayedWETH                             | DelayedWETH                             |
| **Bond model**                 | Fixed challenger bond                 | Position-dependent bond curve           | Position-dependent bond curve           |
| **Time model**                 | Fixed deadlines (challenge, prove)    | Chess clock with extensions             | Chess clock with extensions             |
| **Access control**             | Single proposer + challenger addresses | Permissionless                          | Single proposer + challenger addresses  |
| **Parent chaining**            | Explicit parentIndex in extraData     | N/A (uses ASR anchor only)             | N/A (uses ASR anchor only)             |
| **L2 block challenge**         | N/A (blockHash in extraData)          | `challengeRootL2Block()` + RLP proof   | `challengeRootL2Block()` + RLP proof   |
| **ASR anchor source**          | `anchors(GAME_TYPE)` (legacy path)    | `getAnchorRoot()` (unified path)       | `getAnchorRoot()` (unified path)       |
| **Clone pattern**              | Solady Clone                          | Solady Clone                            | Solady Clone (inherits FDG)            |
| **Game type**                  | 1960                                  | Configurable                            | Configurable                           |
| **Pause handling**             | Via ASR.isGameProper (closeGame)       | ASR.paused() blocks closeGame           | ASR.paused() blocks closeGame          |
| **l2SequenceNumber source**    | CWIA extraData                        | CWIA extraData (= l2BlockNumber)       | CWIA extraData (= l2BlockNumber)       |

---

## 12. Security Considerations

### Trust Model

1. **TEE enclave integrity**: The system trusts that registered TEE enclaves correctly execute state transitions. If an enclave is compromised, it could sign invalid state transitions.

2. **TeeProofVerifier owner**: The owner can register arbitrary addresses as enclaves (the ZK proof verification is a trust gate, but the owner controls registration). A compromised owner could register a non-enclave address.

3. **Proposer/Challenger immutability**: The PROPOSER and CHALLENGER addresses are immutable constructor params. Changing them requires deploying a new implementation contract.

4. **Single challenge model**: Unlike FaultDisputeGame's multi-round bisection, only ONE challenger can challenge a proposal. If the challenger fails to follow through with a proof, the proposal is accepted. There is no mechanism for a second challenger.

5. **`tx.origin` usage**: `initialize()` checks `tx.origin != PROPOSER`. Using `tx.origin` means the proposer's EOA is checked regardless of intermediate contracts, which is consistent with PermissionedDisputeGame but has known risks (e.g., meta-transaction relayers would inherit the tx.origin of the outer caller).

### Potential Risks

1. **Parent chain invalidation cascade**: If a parent game is resolved as CHALLENGER_WINS after child games are created, all children automatically resolve as CHALLENGER_WINS. If the child was challenged, the challenger gets all bonds. If the child was never challenged, the proposer's bond is refunded (not burned to address(0)).

2. **No replay protection on prove()**: The same proof bytes can be submitted to different game instances if they happen to cover the same state range. This is not a vulnerability (the proof is still valid), but it means provers don't need unique proofs per game.

3. **resolve() when parent not resolved**: If the parent game's status is IN_PROGRESS, `resolve()` reverts with `ParentGameNotResolved`. This blocks resolution until the parent resolves, which could delay credit claims.

4. **Bond held as native ETH**: Unlike FaultDisputeGame which uses DelayedWETH for withdrawal delays, TeeDisputeGame holds ETH directly. The finality delay is enforced by `ASR.isGameFinalized()` instead.

5. **Enclave registration root key comparison**: `TeeProofVerifier.register()` compares root keys via `keccak256(rootKey) != keccak256(expectedRootKey)`, which is correct but uses dynamic memory allocation for the hash. The `expectedRootKey` is stored as `bytes` (storage-heavy) rather than a `bytes32` hash.

---

## 13. Optimization Suggestions

### 1. Use `getAnchorRoot()` instead of `anchors()`

In `initialize()`, when `parentIndex == type(uint32).max`:

```solidity
// Current (uses legacy function):
(startingOutputRoot.root, startingOutputRoot.l2SequenceNumber) =
    IAnchorStateRegistry(ANCHOR_STATE_REGISTRY).anchors(GAME_TYPE);

// Suggested (uses current function):
(startingOutputRoot.root, startingOutputRoot.l2SequenceNumber) =
    ANCHOR_STATE_REGISTRY.getAnchorRoot();
```

The `anchors()` function is explicitly marked `@custom:legacy` in AnchorStateRegistry and ignores the `GameType` parameter entirely. Using `getAnchorRoot()` is more correct and future-proof.

### 2. Add pause check in closeGame()

FaultDisputeGame's `closeGame()` explicitly checks `ANCHOR_STATE_REGISTRY.paused()` and reverts with `GamePaused()` to prevent games from entering REFUND mode during temporary pauses. TeeDisputeGame relies on `isGameProper()` returning false during pauses (which sends games to REFUND mode), but this means paused games permanently enter REFUND mode rather than waiting. Consider adding:

```solidity
if (ANCHOR_STATE_REGISTRY.paused()) revert GamePaused();
```

### 3. Add resolvedAt check in closeGame()

FaultDisputeGame's `closeGame()` explicitly checks `resolvedAt.raw() != 0` before proceeding. TeeDisputeGame relies on `ASR.isGameFinalized()` to check this, but adding an explicit local check would be defensive:

```solidity
if (resolvedAt.raw() == 0) revert GameNotResolved();
```

### 4. Store expectedRootKey as bytes32 hash

In TeeProofVerifier, storing `expectedRootKey` as `bytes` uses ~3 storage slots (96 bytes). Instead, store the keccak256 hash:

```solidity
bytes32 public immutable expectedRootKeyHash;
// In register(): if (keccak256(rootKey) != expectedRootKeyHash) revert InvalidRootKey();
```

### 5. Consider allowing multiple challengers

The current model allows only one challenger per game. If the first challenger colludes with the proposer (challenges but never provides proof), the game resolves in the challenger's favor, not a third party's. While the bond economics discourage this, allowing multiple challengers or a challenge-replacement mechanism would be more robust.

### 6. Use EIP-712 typed data for batchDigest

The current `batchDigest` is a plain `keccak256(abi.encode(...))`. Using EIP-712 structured data would:
- Prevent cross-contract replay if another contract uses the same digest scheme
- Provide better wallet UX for TEE key management

### 7. Add explicit receive()/fallback()

The contract accepts ETH via `initialize()` and `challenge()` (both `payable`), but has no `receive()` function. If ETH is accidentally sent directly, it will be lost. Consider adding a `receive()` that reverts.

### 8. Consider reentrancy guard on claimCredit()

`claimCredit()` makes a low-level `call` to `_recipient` before the function completes. While credits are zeroed before the call, a reentrancy guard would be a defense-in-depth measure consistent with best practices.

### 9. Align resolve() checks with FaultDisputeGame

FaultDisputeGame uses `GameNotInProgress` error for the "already resolved" check. TeeDisputeGame uses `ClaimAlreadyResolved`. Consider using the same error for consistency, or renaming to avoid confusion (since `ClaimAlreadyResolved` is also used in FDG's `resolveClaim` with a different semantic meaning).
