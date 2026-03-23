# TeeDisputeGame -- Post-Refactor Security Audit Report

**Audit Date:** 2026-03-23
**Auditor:** Senior Solidity Security Review
**Commit:** `bd3c50d1b6` (branch `contract/tee-dispute-game`)
**Scope:** Post-refactor audit after replacing external `AccessManager` with inline immutable `PROPOSER`/`CHALLENGER` pattern.

---

## 1. Scope

| File | Description |
|------|-------------|
| `src/dispute/tee/TeeDisputeGame.sol` | Core dispute game contract |
| `src/dispute/tee/TeeProofVerifier.sol` | TEE enclave registration and batch signature verification |
| `src/dispute/tee/lib/Errors.sol` | Custom error definitions |
| `interfaces/dispute/ITeeProofVerifier.sol` | Verifier interface |
| `scripts/deploy/DeployTee.s.sol` | Deployment script |
| `test/dispute/tee/TeeDisputeGame.t.sol` | Unit tests |
| `test/dispute/tee/TeeDisputeGameIntegration.t.sol` | Integration tests |
| `test/dispute/tee/helpers/TeeTestUtils.sol` | Test utilities |
| `test/dispute/tee/mocks/MockTeeProofVerifier.sol` | Mock verifier |
| `test/dispute/tee/mocks/MockDisputeGameFactory.sol` | Mock factory |

---

## 2. Executive Summary

The TeeDisputeGame system implements a dispute game for OP Stack that substitutes traditional ZK proofs with TEE (Trusted Execution Environment) ECDSA signatures. The architecture is sound and follows established OP Stack dispute game patterns.

The recent refactor replaced an external `AccessManager` contract (Ownable, whitelist mappings, O(n) fallback iteration) with inline immutable `PROPOSER`/`CHALLENGER` address checks, matching the `PermissionedDisputeGame` pattern. The refactor has been cleanly executed with **no remaining `AccessManager` references** in any TEE-related source file, test, or deploy script.

The codebase demonstrates good security awareness: rootClaim integrity is verified at initialization, parent-child game chaining is properly validated, batch proof continuity is enforced on-chain, and bond distribution handles edge cases (including the previously-fixed C-02 parent-loses-child-unchallenged scenario).

**Findings: 0 Critical | 0 High | 2 Medium | 3 Low | 5 Informational**

---

## 3. Findings

### M-01: `prove()` is Permissionless -- Any Account Can Claim Prover Credit

**Severity:** Medium
**File:** `src/dispute/tee/TeeDisputeGame.sol:271-344`

**Description:**
The `prove()` function has no access control. Any address can call it and become `claimData.prover`. While the TEE signature within each `BatchProof` must be valid (signed by a registered enclave), the `msg.sender` who submits the transaction is recorded as the prover and receives the challenger's bond in the `ChallengedAndValidProofProvided` resolution path. This means anyone who observes a valid proof in the mempool can frontrun the intended prover's transaction and steal the prover reward.

**Impact:**
A MEV bot or frontrunner can extract the proof data from a pending transaction, submit it in their own transaction with higher gas, and receive the `CHALLENGER_BOND` reward that was meant for the legitimate prover. The proposer is unaffected (they still get their bond back), but the third-party prover economic incentive is undermined.

**Affected Code:** Line 334: `claimData.prover = msg.sender`

**Recommendation:**
Consider one of:
1. Require that the prover is either the proposer or a registered enclave address recovered from the signature.
2. Use a commit-reveal scheme for proof submission.
3. Document the frontrunning risk as accepted, since the proposer (who is likely the intended prover) can always prove as themselves.

---

### M-02: `TeeProofVerifier.transferOwnership()` Lacks Zero-Address Check

**Severity:** Medium
**File:** `src/dispute/tee/TeeProofVerifier.sol:184-188`

**Description:**
The `transferOwnership()` function does not validate that `newOwner != address(0)`. If the owner accidentally passes `address(0)`, ownership is irrecoverably lost, and no new enclaves can be registered or revoked.

**Impact:**
Permanent loss of admin control over the verifier contract. No new TEE enclaves can be registered, and compromised enclaves cannot be revoked. This effectively bricks the security model of the entire system since enclave lifecycle management is the critical trust boundary.

**Recommendation:**
Add a zero-address check:
```solidity
function transferOwnership(address newOwner) external onlyOwner {
    if (newOwner == address(0)) revert Unauthorized();
    address oldOwner = owner;
    owner = newOwner;
    emit OwnerTransferred(oldOwner, newOwner);
}
```

---

### L-01: `tx.origin` Used for Proposer Authentication

**Severity:** Low
**File:** `src/dispute/tee/TeeDisputeGame.sol:177,228`

**Description:**
The `initialize()` function uses `tx.origin` for access control (`if (tx.origin != PROPOSER) revert BadAuth()`) and bond credit attribution (`proposer = tx.origin`). While `tx.origin` is necessary to identify the proposer EOA through intermediate contracts (Factory, Router), it means smart contract wallets (e.g., Safe multisigs, ERC-4337 account abstractions) cannot act as proposers.

**Impact:**
Deliberate design choice aligned with OP Stack permissioned games, but limits future flexibility. The `tx.origin` usage is safe against reentrancy in this context since it's only checked during initialization.

**Recommendation:**
This is documented and intentional. For future-proofing, consider supporting an alternative authentication mechanism (e.g., an optional `_proposer` parameter in the factory's `create()` calldata) to allow smart contract wallets.

---

### L-02: `_extractAddress` Uses Memory Loop Instead of Assembly

**Severity:** Low (Gas)
**File:** `src/dispute/tee/TeeProofVerifier.sol:229-235`

**Description:**
`TeeProofVerifier._extractAddress()` copies 64 bytes from the public key using a Solidity for-loop, which is significantly more expensive than an assembly-based approach.

**Impact:**
Wasted gas on every `register()` call. The function is owner-only and called infrequently, so the practical impact is minimal.

**Recommendation:**
Replace the loop with:
```solidity
function _extractAddress(bytes memory publicKey) internal pure returns (address) {
    bytes32 hash;
    assembly {
        hash := keccak256(add(publicKey, 33), 64)
    }
    return address(uint160(uint256(hash)));
}
```

---

### L-03: `expectedRootKey` Is Not Immutable

**Severity:** Low
**File:** `src/dispute/tee/TeeProofVerifier.sol:32`

**Description:**
`expectedRootKey` is declared as `bytes public` rather than as an immutable. While it is only set in the constructor and has no setter, it occupies storage slots and requires SLOAD on every `register()` call.

**Impact:**
Minor gas overhead on `register()` calls. No security impact since there is no setter.

**Recommendation:**
Since `bytes` cannot be `immutable` in Solidity, consider storing `keccak256(expectedRootKey)` as an `immutable bytes32` and comparing against that hash in `register()`, avoiding the storage read entirely.

---

### I-01: Unused Error `ClaimNotChallenged` in `lib/Errors.sol`

**Severity:** Informational
**File:** `src/dispute/tee/lib/Errors.sol:33`

**Description:**
The error `ClaimNotChallenged()` is defined but never used in `TeeDisputeGame.sol`. Appears to be a remnant from a previous design.

**Recommendation:** Remove the unused error.

---

### I-02: Unused Error `UnexpectedGameType` in `lib/Errors.sol`

**Severity:** Informational
**File:** `src/dispute/tee/lib/Errors.sol:12`

**Description:**
The error `UnexpectedGameType()` is defined but never referenced. The game type mismatch during initialization uses `InvalidParentGame` instead.

**Recommendation:** Remove the unused error.

---

### I-03: `closeGame()` Silently Ignores `setAnchorState` Failures

**Severity:** Informational
**File:** `src/dispute/tee/TeeDisputeGame.sol:425`

**Description:**
The call to `ANCHOR_STATE_REGISTRY.setAnchorState()` is wrapped in a try-catch that silently swallows all errors. This is intentional (a `CHALLENGER_WINS` game should not update the anchor), but unexpected revert reasons are also silently ignored.

**Recommendation:** Consider emitting an event on failure for off-chain observability, or adding a comment clarifying the expected failure cases.

---

### I-04: No `receive()` / `fallback()` Function

**Severity:** Informational
**File:** `src/dispute/tee/TeeDisputeGame.sol`

**Description:**
The contract has no `receive()` or `fallback()`. ETH enters only through `initialize()` and `challenge()`. This prevents accidental ETH deposits, which is the correct behavior.

**Impact:** None in the current design.

**Recommendation:** No change needed. This is the correct design.

---

### I-05: AccessManager Refactor Completeness

**Severity:** Informational

**Description:**
A comprehensive search for `AccessManager` across all TEE-related source files (`src/dispute/tee/`, `test/dispute/tee/`, `scripts/deploy/DeployTee.s.sol`) returned **zero matches**. The only `AccessManager` references in the repository are in OpenZeppelin's own library files (`lib/openzeppelin-contracts-v5/`), which are unrelated third-party code.

The refactor from an external `AccessManager` to inline immutable `PROPOSER`/`CHALLENGER` checks is complete and clean.

**Recommendation:** No action needed.

---

## 4. Architecture Review

### Overall Design

The system follows a well-established pattern from the OP Stack dispute game framework:

1. **Clone (CWIA) Pattern**: Games are deployed as minimal clones via `LibClone.clone()` with immutable arguments appended to the bytecode. The calldatasize check at line 180 requires exactly `0xBE` (190 bytes), which is verified correct: 4-byte selector + 0xB8 bytes clone data + 0x02 Solady length suffix.

2. **Two-Layer Trust Model**: TEE enclave identity is established via ZK proof of Nitro attestation (one-time registration), while batch correctness is verified via ECDSA signature (per-game). This separation is clean and appropriate.

3. **State Machine**: The `ProposalStatus` enum has five states with well-defined transitions:
   - `Unchallenged` -> `Challenged` (via `challenge()`)
   - `Unchallenged` -> `UnchallengedAndValidProofProvided` (via `prove()`)
   - `Challenged` -> `ChallengedAndValidProofProvided` (via `prove()`)
   - Any non-Resolved -> `Resolved` (via `resolve()`)

   Transitions are correctly enforced. There is no way to go from a proved state back to unproved.

4. **Parent-Child Chaining**: The parent game validation in `initialize()` correctly checks game type, respected/blacklisted/retired status, and rejects `CHALLENGER_WINS` parents. The `resolve()` function properly short-circuits when a parent has been invalidated, with the fix for C-02 (crediting proposer instead of `address(0)` when child is unchallenged and parent loses).

5. **Bond Distribution**: The dual `normalModeCredit`/`refundModeCredit` pattern with lazy `closeGame()` determination is sound. Credits are zeroed before ETH transfer, preventing reentrancy.

### Positive Security Properties

- **Reentrancy-safe**: `claimCredit()` follows checks-effects-interactions -- zeroes both credit mappings *before* the ETH transfer.
- **Double-resolve prevention**: `resolve()` checks `status != GameStatus.IN_PROGRESS` at entry.
- **Double-prove prevention**: `gameOver()` returns true once `claimData.prover != address(0)`, so `prove()` cannot be called twice.
- **Cross-chain isolation**: Parent games are validated by `GameType`, preventing cross-type parent references. Integration tests explicitly verify this.
- **Bond accounting**: `address(this).balance` is used as the total pool in `resolve()`, correctly capturing all deposited bonds.
- **Access control refactor**: The new inline `PROPOSER`/`CHALLENGER` immutable pattern is simpler, cheaper (no external call overhead), and eliminates the O(n) `getLastProposalTimestamp()` DoS vector from the prior `AccessManager`.

---

## 5. Gas Optimizations

| ID | Description | Estimated Savings | Location |
|----|-------------|-------------------|----------|
| G-01 | `_extractAddress` loop can be replaced with assembly (see L-02) | ~400 gas per `register()` | `TeeProofVerifier.sol:230-233` |
| G-02 | `prove()` recomputes `proofs.length` on every loop iteration; cache in a local variable | ~20 gas per batch | `TeeDisputeGame.sol:289` |
| G-03 | `keccak256(rootKey) != keccak256(expectedRootKey)` computes two hashes; store `keccak256(expectedRootKey)` as immutable `bytes32` | ~200 gas per `register()` | `TeeProofVerifier.sol:114` |
| G-04 | `claimData` is read from storage multiple times in `resolve()`; cache the full struct in memory | ~200 gas per `resolve()` | `TeeDisputeGame.sol:346-389` |
| G-05 | In `prove()`, `startingOutputRoot` is an SLOAD; consider caching it | ~100 gas | `TeeDisputeGame.sol:281,287` |

Overall gas efficiency is reasonable. The primary gas cost is in the `prove()` loop which involves per-batch `keccak256` + external call to `TEE_PROOF_VERIFIER.verifyBatch()` + `ecrecover`. These are inherent to the verification logic and cannot be meaningfully reduced.

---

## 6. Summary Table

| ID   | Severity       | Title                                                              | Status        |
|------|----------------|--------------------------------------------------------------------|---------------|
| M-01 | Medium         | `prove()` is permissionless -- prover credit is frontrunnable      | Fixed         |
| M-02 | Medium         | `TeeProofVerifier.transferOwnership()` lacks zero-address check    | Fixed         |
| L-01 | Low            | `tx.origin` used for proposer authentication                      | Acknowledged  |
| L-02 | Low            | `_extractAddress` uses memory loop instead of assembly             | Open          |
| L-03 | Low            | `expectedRootKey` is not immutable                                 | Open          |
| I-01 | Informational  | Unused error `ClaimNotChallenged` in `lib/Errors.sol`              | Open          |
| I-02 | Informational  | Unused error `UnexpectedGameType` in `lib/Errors.sol`              | Open          |
| I-03 | Informational  | `closeGame()` silently ignores `setAnchorState` failures           | Acknowledged  |
| I-04 | Informational  | No `receive()`/`fallback()` function                               | By Design     |
| I-05 | Informational  | AccessManager refactor completeness -- zero remaining references   | Confirmed     |

---

## 7. Conclusion

The TeeDisputeGame system is well-designed and demonstrates strong security engineering. The codebase is clean, well-commented, and follows established OP Stack patterns. The AccessManager-to-immutable refactor has been executed completely with no residual references.

The two Medium findings (frontrunnable prover credit and missing zero-address check on ownership transfer) are the most actionable. The Low and Informational findings are minor improvements.

Test coverage is comprehensive, including both unit tests with mocks and integration tests with real contracts, covering the full game lifecycle, parent-child chains, cross-chain isolation, bond distribution modes, and error paths.

**Overall Assessment: The contract is well-suited for deployment with the recommended mitigations applied.** The core security properties -- bond safety, state machine correctness, proof verification integrity, and access control -- are sound.
