# TeeDisputeGame -- Security Audit Report

**Auditor:** Senior Solidity Security Auditor
**Date:** 2026-03-20
**Scope:** `TeeDisputeGame.sol`, `AccessManager.sol`, `TeeProofVerifier.sol`, `ITeeProofVerifier.sol`, `lib/Errors.sol`
**Branch:** `contract/tee-dispute-game`

---

## 1. Executive Summary

The TeeDisputeGame system implements an OP Stack dispute game that replaces ZK proofs with TEE (AWS Nitro Enclave) ECDSA signatures for batch state transition verification. It uses a Solady Clone (CWIA) proxy pattern, integrates with `DisputeGameFactory` and `AnchorStateRegistry`, and supports chained multi-batch proofs within `prove()`.

**Overall Risk Assessment: HIGH**

The contract has a well-structured state machine and correctly chains batch proofs with continuity checks. The Clone argument layout and calldatasize check (0xBE) are verified correct. However, several critical and high-severity issues were identified:

- **4 Critical findings:** Proof overwrite via repeated `prove()`, bond loss via `address(0)` crediting, challenge window bypass enabling unchallenged claims, and cross-game signature replay.
- **4 High findings:** Raw digest signing, unbounded batch arrays, post-resolution prove(), and `tx.origin` phishing.
- **6 Medium findings:** DoS on `AccessManager`, silent error swallowing, unused state, timestamp overflow, cascade resolution, and misleading `credit()` view.
- **Several Low/Informational findings** covering ownership safety, gas optimizations, and code organization.

---

## 2. Critical Findings

### C-01: `prove()` Can Be Called Multiple Times -- Proof Overwrite Enables Bond Theft

**Severity:** Critical
**File:** `src/dispute/tee/TeeDisputeGame.sol:261-333`

**Description:**
The `prove()` function has no guard preventing repeated calls. There is no check on `claimData.status` to reject already-proven states, and no check on `claimData.prover` to reject overwrite. An attacker can call `prove()` after a legitimate prover has already submitted a valid proof, replacing `claimData.prover` with their own address.

In the `ChallengedAndValidProofProvided` resolution path (lines 356-363), the prover receives `CHALLENGER_BOND`. By overwriting the prover address, an attacker steals the bond.

Since proof data is submitted in calldata, it is publicly visible in the mempool, making extraction and replay trivial.

**Proof of Concept:**
1. Proposer creates game with bond.
2. Challenger challenges with `CHALLENGER_BOND`.
3. Legitimate prover calls `prove()` with valid batch proofs. Status becomes `ChallengedAndValidProofProvided`.
4. Attacker observes the calldata, calls `prove()` again with identical proof bytes. `claimData.prover` is overwritten to attacker.
5. On `resolve()`, attacker (recorded as prover) receives `CHALLENGER_BOND`.

**Root Cause:** No status guard in `prove()`. The function transitions from `Challenged` -> `ChallengedAndValidProofProvided` on first call, but on the second call, it sees `ChallengedAndValidProofProvided`, finds `counteredBy != address(0)`, and sets status to `ChallengedAndValidProofProvided` again -- succeeding silently while overwriting `prover`.

**Recommendation:**
Add a status check at the beginning of `prove()`:
```solidity
if (claimData.status == ProposalStatus.UnchallengedAndValidProofProvided
    || claimData.status == ProposalStatus.ChallengedAndValidProofProvided) {
    revert ProofAlreadyProvided();
}
```

> **Response: Not a bug.** The PoC at step 4 is incorrect — the second `prove()` call will revert. After the first successful `prove()`, `claimData.prover != address(0)`, which causes `gameOver()` (line 428) to return `true`. The second `prove()` hits `if (gameOver()) revert GameOver()` at line 262 and reverts. Double-prove is already prevented by the existing `gameOver()` guard.

---

### C-02: `resolve()` Credits `address(0)` When Parent Loses and Child Is Unchallenged -- Permanent Fund Loss

**Severity:** Critical
**File:** `src/dispute/tee/TeeDisputeGame.sol:341-343`

**Description:**
When `_getParentGameStatus()` returns `CHALLENGER_WINS`, the child game sets:
```solidity
normalModeCredit[claimData.counteredBy] = address(this).balance;
```
If the child game was never challenged, `claimData.counteredBy == address(0)`. The entire contract balance (the proposer's bond) is credited to `address(0)`. If someone calls `claimCredit(address(0))`, ETH is sent to the zero address and burned. If nobody claims, the funds are locked forever.

**Impact:** The proposer permanently loses their bond through no fault of their own -- parent game invalidation cascades and burns the child proposer's bond.

**Recommendation:**
When `counteredBy == address(0)` in the parent-loses path, refund the proposer:
```solidity
if (parentGameStatus == GameStatus.CHALLENGER_WINS) {
    status = GameStatus.CHALLENGER_WINS;
    address recipient = claimData.counteredBy != address(0) ? claimData.counteredBy : proposer;
    normalModeCredit[recipient] = address(this).balance;
}
```

> **Response: Fixed.** Applied the recommended fix at line 341-347. When `counteredBy == address(0)`, the proposer's bond is now credited back to `proposer` (guaranteed non-zero via `tx.origin`). Regression test added: `test_lifecycle_parentChallengerWins_childUnchallenged_proposerRefunded`.

---

### C-03: Challenge Window Can Be Completely Bypassed -- Proposer Can Prevent All Challenges

**Severity:** Critical
**File:** `src/dispute/tee/TeeDisputeGame.sol:236-249, 261-333, 427-429`

**Description:**
Three interacting design flaws combine to allow a proposer to completely bypass the challenge window:

1. **`prove()` has no status restriction:** It can be called when status is `Unchallenged`, transitioning directly to `UnchallengedAndValidProofProvided`.
2. **`gameOver()` short-circuits on proof:** Returns `true` as soon as `claimData.prover != address(0)` (line 428), regardless of deadline.
3. **`challenge()` checks `gameOver()`:** Reverts if game is over (line 239).

A colluding proposer + TEE enclave can submit `prove()` in the same block as game creation. After this, `gameOver()` returns true permanently, and `challenge()` always reverts. The game resolves as `DEFENDER_WINS` without any challenge opportunity.

Additionally, `challenge()` only accepts `Unchallenged` status (line 237). Even if `gameOver()` were fixed, once `prove()` transitions status to `UnchallengedAndValidProofProvided`, no challenge is possible.

**Proof of Concept:**
1. Proposer creates game via factory (block N) with a fraudulent root claim.
2. In the same block, co-conspirator calls `prove()` with a pre-computed TEE proof from a compromised enclave.
3. `claimData.prover != address(0)`, so `gameOver()` is now true.
4. Any `challenge()` call reverts with `GameOver()`.
5. After parent resolves, `resolve()` gives `DEFENDER_WINS` -- proposer takes all funds, no one could contest.

**Impact:** The fundamental security assumption -- that challengers have a window to contest invalid claims -- is completely broken. A compromised TEE enclave + proposer can push through any claim.

**Recommendation:**
Decouple proof submission from the challenge window:
- Option A: `prove()` should only be callable in `Challenged` state (after a challenge occurs).
- Option B: `gameOver()` should not consider `prover != address(0)` -- only the deadline should matter.
- Option C: Keep the challenge deadline independent and always enforce it: challenges should be allowed regardless of proof status.

> **Response: Not a bug (by design), but documented per auditor recommendation.** The analysis is technically correct but the threat model assumption is wrong for this contract. In TeeDisputeGame, `challenge()` does not submit fraud proof data — it is simply a mechanism to request the TEE to prove, with a bond at stake. The TEE is trusted hardware. If the TEE produces a valid proof, the state transition IS correct — there is no "fraudulent root claim" scenario with a valid TEE signature. Early `prove()` before any challenge is a legitimate optimization that accelerates finality. The challenge window exists as an economic incentive for the TEE to prove on demand, not as a fraud-proof security layer.
>
> **Fix applied:** Added NatSpec documentation to `prove()` explicitly documenting the TEE trust model and that early proving is by design.

---

### C-04: Missing Domain Separation in Batch Digest -- Cross-Game Signature Replay

**Severity:** Critical
**File:** `src/dispute/tee/TeeDisputeGame.sol:295-303`

**Description:**
The `batchDigest` is computed as:
```solidity
keccak256(abi.encode(startBlockHash, startStateHash, endBlockHash, endStateHash, l2Block))
```
This digest contains no game-specific or chain-specific identifier. A valid TEE signature for one game is valid for any other game covering the same L2 block range with the same state hashes.

**Scenario:** Two TeeDisputeGames both cover blocks 100-200 with the same starting output root. A TEE executor signs proofs for game A. Those signatures are replayed on game B without the TEE ever verifying game B's state. This also applies cross-chain if two chains share identical L2 state ranges.

While the final root claim check prevents proving an incorrect claim, this breaks the fundamental security assumption that each game's proof is independently verified by a TEE enclave.

**Impact:** Proofs can be replayed across games sharing the same block range. The TEE verification guarantee is bypassed for replayed games.

**Recommendation:**
Include `address(this)` and `block.chainid` in the digest:
```solidity
bytes32 batchDigest = keccak256(abi.encode(
    block.chainid, address(this),
    proofs[i].startBlockHash, proofs[i].startStateHash,
    proofs[i].endBlockHash, proofs[i].endStateHash,
    proofs[i].l2Block
));
```
The TEE enclave must also include these fields when signing.

> **Response: Not a bug.** For replay to work, both games must have identical `startingOutputRoot`, `rootClaim`, and `l2SequenceNumber` — because `prove()` validates `proofs[0].start == startingOutputRoot`, `proofs[last].end == rootClaim`, and `proofs[last].l2Block == l2SequenceNumber`. In a deterministic L2, same start state + same block range = same end state. So replaying a proof across such games proves the same correct state transition. The proof is not "forged" — it's proving an identical truth. Additionally, `prove()` binds the entire proof chain to the game's specific start and end states, providing implicit domain separation.

---

## 3. High Findings

### H-01: `TeeProofVerifier.verifyBatch()` Uses Raw Digest Without EIP-191/EIP-712

**Severity:** High
**File:** `src/dispute/tee/TeeProofVerifier.sol:148`

**Description:**
`verifyBatch()` calls `ECDSA.tryRecover(digest, signature)` with a raw `bytes32` hash. The digest has no EIP-191 (`\x19Ethereum Signed Message:\n32`) or EIP-712 prefix.

This means:
1. The signature scheme is non-standard and cannot leverage standard wallet signing flows.
2. If the TEE enclave's private key is ever used in any other context that also does raw `ecrecover`, signatures could be cross-purpose replayed.
3. A raw `keccak256` digest could coincidentally match a valid Ethereum transaction hash, though this is astronomically unlikely.

**Impact:** Signatures lack cryptographic domain separation, increasing cross-context replay risk.

**Recommendation:**
Use EIP-712 structured data with a domain separator including the verifier address and chain ID:
```solidity
bytes32 prefixed = keccak256(abi.encodePacked("\x19\x01", DOMAIN_SEPARATOR, digest));
```

> **Response: Acknowledged, won't fix.** TEE enclave private keys are generated inside the enclave and are purpose-specific — they are never used as standard Ethereum wallets or in any other signing context. The cross-context replay risk does not apply. Adding EIP-712 would require changes to both the on-chain contract and the TEE enclave signing logic with no practical security benefit.

---

### H-02: Unbounded Batch Array in `prove()` -- Gas Griefing / DoS

**Severity:** High
**File:** `src/dispute/tee/TeeDisputeGame.sol:264, 278`

**Description:**
`prove()` decodes `proofBytes` into `BatchProof[]` with no upper bound. Each iteration performs:
- Two `keccak256` operations
- One external call to `TEE_PROOF_VERIFIER.verifyBatch()` (which includes `ecrecover`)
- Multiple storage reads and comparisons

If the L2 block range is very large and split into many small batches, the gas cost could exceed block gas limits, preventing legitimate proofs from being submitted.

**Impact:** Could prevent legitimate proofs from being submitted if the required batch count is too high, or could be used to submit transactions that fail unpredictably.

**Recommendation:**
Add a maximum batch count:
```solidity
uint256 constant MAX_BATCH_COUNT = 256;
if (proofs.length > MAX_BATCH_COUNT) revert TooManyBatches();
```

> **Response: Acknowledged, won't fix.** TEE proof submission is permissioned — only trusted operators submit proofs. They will not submit oversized arrays. Even in a permissionless context, the attacker pays their own gas for a failed transaction that does not affect anyone else. The practical batch count is 1-5 segments; gas limit is not a realistic concern.

---

### H-03: `prove()` Missing `IN_PROGRESS` Status Guard -- Post-Resolution Mutation

**Severity:** High
**File:** `src/dispute/tee/TeeDisputeGame.sol:261-333`

**Description:**
`prove()` checks `gameOver()` but does not check `status == GameStatus.IN_PROGRESS`. In the parent-loses resolution path (lines 338-343), `resolve()` can be called before the deadline expires (it does not require `gameOver()`). If `prover` is still `address(0)` at that point, `gameOver()` returns false (deadline not passed), and `prove()` could be called on an already-resolved game, mutating `claimData.prover` and `claimData.status`.

**Impact:** State mutation after resolution. While bond distribution was already assigned during `resolve()`, changing `claimData.prover` and `claimData.status` corrupts on-chain game state for any external readers.

**Recommendation:**
Add `if (status != GameStatus.IN_PROGRESS) revert ClaimAlreadyResolved();` at the start of `prove()`.

> **Response: Fixed.** Added `if (status != GameStatus.IN_PROGRESS) revert ClaimAlreadyResolved();` at the top of `prove()` (line 262). This prevents state mutation after resolution in all paths, including the parent-loses cascade where `resolve()` can be called before the deadline expires.

---

### H-04: `tx.origin` Enables Proposer Phishing

**Severity:** High
**File:** `src/dispute/tee/TeeDisputeGame.sol:174, 225`

**Description:**
`initialize()` uses `tx.origin` for both authorization and identity:
```solidity
if (!ACCESS_MANAGER.isAllowedProposer(tx.origin)) revert BadAuth();
...
proposer = tx.origin;
```
A whitelisted proposer interacting with any malicious contract could have `factory.create()` called in the same transaction, creating a game with attacker-controlled parameters (root claim, extraData) under the proposer's identity.

**Impact:** A whitelisted proposer can be tricked into creating games with invalid root claims. Their bond is at risk when the claim is challenged.

**Recommendation:**
This is a known OP Stack pattern. Document the risk prominently. Proposers should be advised to never interact with untrusted contracts from their whitelisted EOA. Long-term, consider passing the proposer address explicitly from the factory.

> **Response: Acknowledged, won't fix.** This is a known OP Stack pattern used across all permissioned dispute games (including `PermissionedDisputeGame`). `tx.origin` is necessary to attribute the proposer through intermediate contracts like `DisputeGameFactoryRouter`. Proposers are trusted operators that should use dedicated EOAs.

---

## 4. Medium Findings

### M-01: `AccessManager.getLastProposalTimestamp()` Unbounded Loop -- DoS Risk

**Severity:** Medium
**File:** `src/dispute/tee/AccessManager.sol:86-106`

**Description:**
`getLastProposalTimestamp()` iterates backward through all games in the factory. If many non-TEE games are created after the last TEE game, this loop's gas cost can exceed the block gas limit.

This function is called during `isAllowedProposer()`, which is called during `initialize()`. A DoS here prevents new TEE game creation.

**Attack vector:** Anyone can create cheap non-TEE games to inflate `gameCount()`, making `isAllowedProposer()` exceed gas limits. Eventually, `FALLBACK_TIMEOUT` expires, but `isProposalPermissionlessMode()` itself calls `getLastProposalTimestamp()`, so even permissionless mode may be DoS'd.

**Impact:** Temporary DoS on TEE game creation, followed by permanent bypass of proposer access control (or permanent DoS if the loop always exceeds gas limits).

**Recommendation:**
Cache the latest TEE game timestamp in a storage variable updated during game creation, or set a maximum scan depth with a fallback.

> **Response: Acknowledged, known issue.** Will optimize in a future iteration by caching the latest TEE game timestamp in a storage variable.

---

### M-02: Silent Error Swallowing in `closeGame()` Try-Catch

**Severity:** Medium
**File:** `src/dispute/tee/TeeDisputeGame.sol:410`

**Description:**
```solidity
try ANCHOR_STATE_REGISTRY.setAnchorState(IDisputeGame(address(this))) {} catch {}
```
All errors from `setAnchorState()` are silently swallowed. If the call fails for a legitimate reason (e.g., the registry is paused or upgraded), the anchor state is not updated, potentially causing subsequent games to reference stale starting output roots.

**Impact:** Anchor state may not be updated when it should be. No way for off-chain monitoring to detect failures.

**Recommendation:**
Emit an event in the catch block:
```solidity
try ANCHOR_STATE_REGISTRY.setAnchorState(IDisputeGame(address(this))) {}
catch (bytes memory reason) {
    emit AnchorStateUpdateFailed(reason);
}
```

> **Response: Acknowledged, won't fix.** This pattern is consistent with existing OP Stack dispute games. `setAnchorState()` failing is non-critical — the anchor state will be updated by the next successful game. Off-chain monitoring can detect this via the absence of anchor state changes.

---

### M-03: `wasRespectedGameTypeWhenCreated` Is Set But Never Read

**Severity:** Medium
**File:** `src/dispute/tee/TeeDisputeGame.sol:141, 228-229`

**Description:**
This boolean is written during `initialize()` (costing ~20,000 gas for cold SSTORE) but never read by any function in this contract or apparent external caller.

**Impact:** Wasted gas on every game initialization. If intended for external use by `AnchorStateRegistry`, it lacks documentation.

**Recommendation:**
Remove if unused, or document its intended external consumer.

> **Response: Acknowledged, won't fix.** This field is intended for external consumers (e.g., `AnchorStateRegistry.isGameRespected()`) and aligns with the OP Stack convention used in other dispute game implementations.

---

### M-04: Timestamp Overflow Edge Case in Deadline Computation

**Severity:** Medium
**File:** `src/dispute/tee/TeeDisputeGame.sol:221, 244`

**Description:**
```solidity
Timestamp.wrap(uint64(block.timestamp + MAX_CHALLENGE_DURATION.raw()))
```
If `MAX_CHALLENGE_DURATION` is misconfigured to a very large value, the `uint256` addition could exceed `type(uint64).max`, and the `uint64` cast silently truncates, potentially setting a deadline in the past. This would make the game instantly "over."

**Impact:** Misconfigured durations could make games instantly expire. Low probability since durations are set at construction, but no protection exists.

**Recommendation:**
Add a constructor validation:
```solidity
require(block.timestamp + _maxChallengeDuration.raw() <= type(uint64).max, "duration overflow");
require(block.timestamp + _maxProveDuration.raw() <= type(uint64).max, "duration overflow");
```

> **Response: Acknowledged, won't fix.** Constructor parameters are set by the deployer (trusted). Misconfiguration is an operational issue, not a contract vulnerability. The same pattern is used in other OP Stack contracts without overflow checks.

---

### M-05: Cascade Resolution Bypasses Child Game's Challenge/Prove Period

**Severity:** Medium
**File:** `src/dispute/tee/TeeDisputeGame.sol:338-343`

**Description:**
When `parentGameStatus == CHALLENGER_WINS`, `resolve()` does not check `gameOver()`. A child game can be resolved as `CHALLENGER_WINS` immediately, even if its own challenge/prove period has not ended and a valid proof was about to be submitted.

**Impact:** A child game's proposer loses their bond due to parent invalidation, even if their own claim was independently valid and provable.

**Recommendation:**
This may be intentional (cascading invalidation should be immediate). Document the behavior. Consider using REFUND mode for parent-invalidated games so no party is unfairly penalized (see also C-02 fix).

> **Response: By design.** Parent invalidation means the child's `startingOutputRoot` was derived from an invalid parent — the entire proof chain is based on incorrect state. Immediate cascade is the correct behavior. The C-02 fix ensures that when a child was never challenged, the proposer gets their bond refunded rather than losing it to `address(0)`.

---

### M-06: `credit()` View Function Returns Misleading Data When `UNDECIDED`

**Severity:** Medium
**File:** `src/dispute/tee/TeeDisputeGame.sol:431-437`

**Description:**
```solidity
function credit(address _recipient) external view returns (uint256 credit_) {
    if (bondDistributionMode == BondDistributionMode.REFUND) {
        credit_ = refundModeCredit[_recipient];
    } else {
        credit_ = normalModeCredit[_recipient];
    }
}
```
When `bondDistributionMode == UNDECIDED`, the else branch returns `normalModeCredit`, which is 0 for all addresses before resolution. This could mislead off-chain consumers into believing there are no credits when in fact `refundModeCredit` holds deposited amounts.

**Impact:** Front-end/off-chain confusion. Users may not see their refundable bonds when the game is still undecided.

**Recommendation:**
Return 0 or revert when `bondDistributionMode == UNDECIDED`, or return both credit amounts.

> **Response: Acknowledged, won't fix.** Low impact — only affects off-chain display before game resolution. `claimCredit()` correctly handles mode selection and reverts with `InvalidBondDistributionMode` if mode is still `UNDECIDED`.

---

## 5. Low/Informational Findings

### L-01: `TeeProofVerifier` Ownership Transfer Lacks Two-Step Pattern

**Severity:** Low
**File:** `src/dispute/tee/TeeProofVerifier.sol:184-188`

`transferOwnership()` immediately transfers ownership. If the wrong address is supplied, ownership is irrecoverably lost. The owner controls enclave registration and revocation.

**Recommendation:** Use a two-step transfer pattern (propose + accept) or OpenZeppelin's `Ownable2Step`.

> **Response: Acknowledged, won't fix.** Acceptable for admin operations with trusted deployers.

---

### L-02: `TeeProofVerifier.transferOwnership()` Missing Zero-Address Check

**Severity:** Low
**File:** `src/dispute/tee/TeeProofVerifier.sol:184`

Transferring to `address(0)` permanently disables `register()` and `revoke()`.

**Recommendation:** Add `require(newOwner != address(0))`.

> **Response: Acknowledged, won't fix.** Operational risk managed by trusted admin.

---

### L-03: `expectedRootKey` Is Not Enforced Immutable

**Severity:** Low
**File:** `src/dispute/tee/TeeProofVerifier.sol:32`

`expectedRootKey` is `bytes public` (Solidity does not support `immutable` for `bytes`). Only set in the constructor with no setter, but a future code change could accidentally add one, or the slot could be manipulated if this contract were behind a `delegatecall` proxy.

**Recommendation:** Store `keccak256(expectedRootKey)` as an `immutable bytes32` and validate against it during registration.

> **Response: Acknowledged, won't fix.** No setter exists and the contract is not used behind a delegatecall proxy. Safe by construction.

---

### L-04: Custom Errors Defined Inline Instead of in Errors Library

**Severity:** Informational
**File:** `src/dispute/tee/TeeDisputeGame.sol:101-107`

Seven custom errors (`EmptyBatchProofs`, `StartHashMismatch`, `BatchChainBreak`, `BatchBlockNotIncreasing`, `FinalHashMismatch`, `FinalBlockMismatch`, `RootClaimMismatch`) are defined in the main contract file instead of `src/dispute/tee/lib/Errors.sol`.

**Recommendation:** Move to the errors library for consistency and discoverability.

> **Response: Acknowledged, won't fix.** Code organization preference. These errors are specific to `prove()` batch verification logic and are co-located with the code that uses them.

---

### L-05: No `receive()` or `fallback()` Function

**Severity:** Informational
**File:** `src/dispute/tee/TeeDisputeGame.sol`

The contract has no `receive()` or `fallback()`. ETH can only enter via `initialize()` and `challenge()`. ETH force-sent via deprecated `selfdestruct` would inflate `address(this).balance` beyond tracked amounts. Since `resolve()` uses `address(this).balance` directly (e.g., line 349), forced ETH would be distributed to the winner -- a minor accounting discrepancy.

**Recommendation:** Acceptable by design. Document that direct ETH transfers are not supported.

> **Response: By design.** Consistent with OP Stack dispute game conventions. Force-sent ETH goes to the winner — a minor surplus, not a vulnerability.

---

### L-06: `challenge()` Single-Challenger Model

**Severity:** Low
**File:** `src/dispute/tee/TeeDisputeGame.sol:237`

Only one challenger can participate (first caller wins). Competing challengers waste gas on reverted transactions.

**Recommendation:** Document as intentional.

> **Response: By design.** Single-challenger model is intentional — aligns with the TEE dispute game's simplified challenge-prove model.

---

### L-07: Missing Detailed Events for Bond Credit Assignments

**Severity:** Low
**File:** `src/dispute/tee/TeeDisputeGame.sol:335-374`

The `Resolved` event only emits `GameStatus`. Credit assignments (who gets how much) are not emitted, making off-chain monitoring harder.

**Recommendation:** Emit events with recipient addresses and amounts during credit assignment.

> **Response: Acknowledged, won't fix.** Low priority. Credit assignments can be derived from `normalModeCredit`/`refundModeCredit` state reads after resolution.

---

### I-01: Calldatasize Check (0xBE) Is Correct

**File:** `src/dispute/tee/TeeDisputeGame.sol:177`

The calldatasize check of `0xBE` (190 bytes) is verified correct:
- 4 bytes: function selector (`initialize()`)
- 184 bytes (0xB8): Solady Clone immutable args
  - 0x00: `gameCreator` (address, 20 bytes)
  - 0x14: `rootClaim` (bytes32, 32 bytes)
  - 0x34: `l1Head` (bytes32, 32 bytes)
  - 0x54: `l2SequenceNumber` (uint256, 32 bytes)
  - 0x74: `parentIndex` (uint32, 4 bytes)
  - 0x78: `blockHash` (bytes32, 32 bytes)
  - 0x98: `stateHash` (bytes32, 32 bytes)
- 2 bytes: Solady Clone length suffix

Total: 4 + 184 + 2 = 190 = 0xBE. **Correct.**

> **Response: Confirmed.**

---

### I-02: `claimCredit()` Follows CEI Pattern Correctly

**File:** `src/dispute/tee/TeeDisputeGame.sol:376-395`

The `claimCredit()` function zeroes out both `refundModeCredit` and `normalModeCredit` (lines 390-391) before making the external ETH transfer (line 393). This follows the Checks-Effects-Interactions pattern and prevents reentrancy even without a dedicated reentrancy guard. **Safe.**

> **Response: Confirmed.**

---

### I-03: `extraData()` Layout Verified Correct

**File:** `src/dispute/tee/TeeDisputeGame.sol:454`

`extraData()` returns `_getArgBytes(0x54, 0x64)` (100 bytes from offset 0x54):
- `l2SequenceNumber` (32 bytes at 0x54)
- `parentIndex` (4 bytes at 0x74)
- `blockHash` (32 bytes at 0x78)
- `stateHash` (32 bytes at 0x98)

Total: 32 + 4 + 32 + 32 = 100 = 0x64. **Correct.**

> **Response: Confirmed.**

---

### I-04: `closeGame()` Is Correctly Idempotent

**File:** `src/dispute/tee/TeeDisputeGame.sol:397-421`

`closeGame()` returns early if `bondDistributionMode` is already `REFUND` or `NORMAL`. Combined with `claimCredit()` always calling `closeGame()` first, this makes the claim flow safe for repeated calls.

> **Response: Confirmed.**

---

## 6. Gas Optimizations

### G-01: Cache `claimData` in Memory in `resolve()`

`resolve()` reads `claimData.status`, `claimData.counteredBy`, `claimData.prover` multiple times from storage. Caching the struct in memory saves ~2100 gas per cold SLOAD.

> **Response: Acknowledged, may optimize later.**

### G-02: Use `unchecked` for Loop Increment in `prove()`

```solidity
for (uint256 i = 0; i < proofs.length; ) {
    ...
    unchecked { ++i; }
}
```
Saves ~40 gas per iteration since `i` is bounded by `proofs.length`.

> **Response: Acknowledged, may optimize later.**

### G-03: `_extractAddress` Byte-by-Byte Copy

**File:** `src/dispute/tee/TeeProofVerifier.sol:229-234`

The function copies 64 bytes one at a time. Assembly-based copy would save gas:
```solidity
function _extractAddress(bytes memory publicKey) internal pure returns (address) {
    bytes32 hash;
    assembly {
        hash := keccak256(add(publicKey, 33), 64)
    }
    return address(uint160(uint256(hash)));
}
```
Only called during `register()` (infrequent), so low impact.

> **Response: Acknowledged, may optimize later.**

---

## 7. State Machine Analysis

### ProposalStatus Transitions

```
Unchallenged ----[challenge()]----> Challenged
Unchallenged ----[prove()]-------> UnchallengedAndValidProofProvided
Challenged ------[prove()]-------> ChallengedAndValidProofProvided
Any status ------[resolve()]-----> Resolved
```

**Issues in state machine:**
1. `prove()` can be called in ANY non-gameOver status, including after proof already provided (C-01).
2. `prove()` can be called in `Unchallenged` state, bypassing the challenge window entirely (C-03).
3. `challenge()` can only be called from `Unchallenged` -- once proved, challenging is impossible.
4. Once `prove()` is called, `gameOver()` returns true permanently, blocking all further `challenge()` calls (C-03).

### GameStatus Transitions

```
IN_PROGRESS ----[resolve(), parent lost]----------> CHALLENGER_WINS
IN_PROGRESS ----[resolve(), Unchallenged]----------> DEFENDER_WINS
IN_PROGRESS ----[resolve(), Challenged, no proof]--> CHALLENGER_WINS
IN_PROGRESS ----[resolve(), Unchallenged+proof]----> DEFENDER_WINS
IN_PROGRESS ----[resolve(), Challenged+proof]------> DEFENDER_WINS
```

`resolve()` correctly prevents double-resolution via `status != GameStatus.IN_PROGRESS` check (line 336).

> **Response: State machine analysis is correct. Points 1-4 under "Issues" are by design in the TEE trust model — see C-01 and C-03 responses above.**

---

## 8. Bond Flow Analysis

### Deposit Paths
| Action | Depositor | Amount | Credited To |
|--------|-----------|--------|-------------|
| `initialize()` | Proposer (`tx.origin`) | `msg.value` | `refundModeCredit[proposer]` |
| `challenge()` | Challenger (`msg.sender`) | `CHALLENGER_BOND` (exact) | `refundModeCredit[msg.sender]` |

### Distribution Paths (Normal Mode)
| Scenario | Proposer | Challenger | Prover |
|----------|----------|------------|--------|
| Unchallenged, no proof | All balance | N/A | N/A |
| Unchallenged + proof | All balance | N/A | Nothing |
| Challenged, no proof (deadline) | Nothing | All balance | N/A |
| Challenged + proof, prover == proposer | All balance | Nothing | (same) |
| Challenged + proof, prover != proposer | Balance - CHALLENGER_BOND | Nothing | CHALLENGER_BOND |
| Parent lost, has challenger | Nothing | All balance | N/A |
| **Parent lost, no challenger** | **Nothing (BUG C-02)** | **N/A** | **N/A** |

### Refund Mode
Each participant receives back exactly what they deposited via `refundModeCredit`.

> **Response: Bond flow analysis is correct. The "Parent lost, no challenger" row has been fixed — proposer now receives their bond back. See C-02 fix.**

---

## Summary Table

| ID   | Severity     | Title                                                                    |
|------|-------------|--------------------------------------------------------------------------|
| C-01 | Critical    | `prove()` can be called multiple times -- proof overwrite enables theft   |
| C-02 | Critical    | `resolve()` credits `address(0)` when parent loses, child unchallenged   |
| C-03 | Critical    | Challenge window bypass -- proposer can prevent all challenges            |
| C-04 | Critical    | No domain separation in batch digest -- cross-game signature replay      |
| H-01 | High        | Raw digest signing without EIP-191/EIP-712 prefix                        |
| H-02 | High        | Unbounded batch array in `prove()` -- gas griefing/DoS                   |
| H-03 | High        | `prove()` missing `IN_PROGRESS` status guard -- post-resolution mutation |
| H-04 | High        | `tx.origin` enables proposer phishing                                    |
| M-01 | Medium      | `AccessManager.getLastProposalTimestamp()` unbounded loop DoS            |
| M-02 | Medium      | Silent error swallowing in `closeGame()` try-catch                       |
| M-03 | Medium      | `wasRespectedGameTypeWhenCreated` set but never read                     |
| M-04 | Medium      | Timestamp overflow edge case in deadline computation                     |
| M-05 | Medium      | Cascade resolution bypasses child game period                            |
| M-06 | Medium      | `credit()` view returns misleading data when `UNDECIDED`                 |
| L-01 | Low         | `TeeProofVerifier` ownership lacks two-step transfer                     |
| L-02 | Low         | Missing zero-address check in `transferOwnership()`                      |
| L-03 | Low         | `expectedRootKey` not enforced immutable                                 |
| L-04 | Info        | Custom errors defined inline instead of in Errors.sol                    |
| L-05 | Info        | No `receive()`/`fallback()` -- force-sent ETH unaccounted               |
| L-06 | Low         | Single-challenger model enables front-running                            |
| L-07 | Low         | Missing detailed events for bond credit assignments                      |

**Priority Recommendations (before deployment):**
1. **Immediate:** Fix C-01 -- add status guard to `prove()` to prevent proof overwrite.
2. **Immediate:** Fix C-02 -- handle `counteredBy == address(0)` in parent-loses path.
3. **Immediate:** Fix C-03 -- enforce challenge window independently from proof submission.
4. **Immediate:** Fix C-04 -- add domain separation to `batchDigest`.
5. **High priority:** Add `IN_PROGRESS` check to `prove()` (H-03).
6. **High priority:** Add EIP-712 domain separator to TEE signing (H-01).
7. **Before mainnet:** Address AccessManager DoS vector (M-01).

> **Response summary:**
> - **C-02: Fixed.** Bond now credited to proposer when `counteredBy == address(0)`.
> - **C-03: Documented.** Added NatSpec to `prove()` explicitly documenting TEE trust model and early prove design.
> - **C-01, C-04: Not bugs** under the TEE trust model — see individual responses above.
> - **H-01, H-02, H-04: Won't fix** — acceptable given permissioned TEE architecture.
> - **H-03: Fixed.** Added `IN_PROGRESS` status guard to `prove()`.
> - **M-01: Known issue** — will optimize in future iteration.
> - **M-02 through M-06: Won't fix / by design** — see individual responses.
> - **Additional fix (not from audit):** Cross-chain GameType isolation added in `initialize()` line 190-191.
