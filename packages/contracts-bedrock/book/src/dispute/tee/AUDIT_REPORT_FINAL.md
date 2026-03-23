# TeeDisputeGame -- Final Audit Report (Fix Verification)

**Audit Date:** 2026-03-23
**Auditor:** Senior Solidity Security Review
**Fix Commit:** `6f12abbcc8` (branch `contract/tee-dispute-game`)
**Base Audit:** Post-Refactor Audit Report at commit `bd3c50d1b6`
**Scope:** Verification of fixes for findings M-01 and M-02 from the post-refactor audit, plus review for newly introduced issues.

---

## 1. Executive Summary

The post-refactor audit identified two Medium-severity findings:

- **M-01**: `prove()` was permissionless, allowing frontrunning of prover credit (CHALLENGER_BOND).
- **M-02**: `TeeProofVerifier.transferOwnership()` lacked a zero-address check; the custom ownership pattern was fragile.

Both findings have been **fully and correctly fixed** in commit `6f12abbcc8`. The M-01 fix restricts `prove()` to the proposer via `msg.sender` check and simplifies the `resolve()` bond distribution. The M-02 fix replaces the custom ownership implementation with OpenZeppelin Ownable v4, which provides built-in zero-address validation and a battle-tested ownership model.

One new low-severity observation is noted regarding the inherited `renounceOwnership()` function. No critical, high, or medium issues were introduced by the fixes.

**Verdict: All Medium findings are resolved. The fixes are correct, complete, and well-tested.**

---

## 2. Finding Verification

### M-01: `prove()` is Permissionless -- Prover Credit Frontrunnable

**Original Finding:** Any address could call `prove()` and be recorded as `claimData.prover`, allowing MEV bots to frontrun legitimate provers and steal the CHALLENGER_BOND reward in the `ChallengedAndValidProofProvided` resolution path.

**Fix Applied:**

In `TeeDisputeGame.sol`, line 272:

```solidity
function prove(bytes calldata proofBytes) external returns (ProposalStatus) {
    if (msg.sender != proposer) revert BadAuth();
    ...
```

The `proposer` state variable is set to `tx.origin` during `initialize()`, so only the original proposer EOA can call `prove()`.

Additionally, `resolve()` bond distribution was simplified. Since `prover == proposer` is now guaranteed, the `ChallengedAndValidProofProvided` case awards `normalModeCredit[proposer] = address(this).balance` -- the proposer receives both their own bond and the challenger's bond. There is no longer a need for a separate prover/proposer split or third-party prover logic.

**Verification:**

1. **Access control correctness**: The `msg.sender != proposer` check uses the `proposer` state variable (set from `tx.origin` in `initialize()`), not the `PROPOSER` immutable. This is correct because it checks against the actual proposer of this specific game instance. The `BadAuth` error is the same error used for other access control checks in the contract, maintaining consistency.

2. **State machine interaction**: The `BadAuth` revert occurs before any state mutation, so a rejected `prove()` call has no side effects. The check is positioned after `status != GameStatus.IN_PROGRESS` and before `gameOver()`, which is the correct ordering -- rejecting unauthorized callers early.

3. **Bond distribution correctness in all resolve() paths**:
   - `Unchallenged`: proposer gets `balance` (only proposer bond in contract). Correct.
   - `Challenged` (no proof, timeout): challenger gets `balance` (proposer bond + challenger bond). Correct.
   - `UnchallengedAndValidProofProvided`: proposer gets `balance`. Correct.
   - `ChallengedAndValidProofProvided`: proposer gets `balance` (proposer bond + challenger bond). Correct -- proposer proved, so they win the challenger's bond.
   - Parent `CHALLENGER_WINS` with child challenged: challenger gets `balance`. Correct.
   - Parent `CHALLENGER_WINS` with child unchallenged: proposer gets `balance` (refund). Correct.

4. **No remaining third-party prover references**: Searched for `thirdPartyProver`, `bond split`, and related terms across all TEE source and test files -- zero matches.

5. **Test coverage**: `test_prove_revertUnauthorizedProver` explicitly tests that a non-proposer address receives `BadAuth`. The existing `test_prove_succeedsWithSingleBatch` and `test_prove_succeedsWithChainedBatches` tests call `prove()` via `vm.prank(proposer)`, confirming the happy path. Integration tests (`test_lifecycle_challenged_proveByProposer_defenderWins`, `test_lifecycle_viaRouter_fullCycle`) exercise the full lifecycle with proposer-only proving.

**Status: FIXED** -- The fix fully addresses the finding with no residual risk.

---

### M-02: `TeeProofVerifier.transferOwnership()` Lacks Zero-Address Check

**Original Finding:** The custom `transferOwnership()` function did not validate `newOwner != address(0)`, risking irrecoverable loss of admin control over enclave registration and revocation.

**Fix Applied:**

The entire custom ownership implementation (state variable `owner`, `Unauthorized` error, `OwnerTransferred` event, `onlyOwner` modifier, and `transferOwnership()` function) was removed and replaced with inheritance from OpenZeppelin Ownable v4 (`@openzeppelin/contracts/access/Ownable.sol`):

```solidity
import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";

contract TeeProofVerifier is Ownable {
    ...
}
```

**Verification:**

1. **Zero-address protection**: OpenZeppelin Ownable v4's `transferOwnership()` includes `require(newOwner != address(0), "Ownable: new owner is the zero address")`. This is tested in `test_transferOwnership_revertZeroAddress` which expects the revert string `"Ownable: new owner is the zero address"`.

2. **Constructor behavior**: OZ Ownable v4's constructor calls `_transferOwnership(_msgSender())`, setting the deployer as the initial owner. Since `TeeProofVerifier`'s constructor does not pass an explicit owner, the deployer address is used. This matches the previous behavior where `owner = msg.sender` was set in the constructor.

3. **Storage layout**: OZ Ownable v4 uses a single `address private _owner` slot. The previous custom implementation also used a single `address public owner` slot. Since `TeeProofVerifier` is not upgradeable (no proxy), storage layout conflicts are not a concern. The visibility change from `public` to `private` (with a `public owner()` getter) is functionally equivalent.

4. **Event name change**: The custom `OwnerTransferred(address, address)` event is replaced by OZ's `OwnershipTransferred(address indexed, address indexed)`. This is a breaking change for off-chain indexers monitoring the old event name. Since the contract is being deployed fresh (not upgraded), this is acceptable.

5. **`onlyOwner` modifier**: OZ Ownable's `onlyOwner` modifier uses `require(owner() == _msgSender(), "Ownable: caller is not the owner")`. The `register()` and `revoke()` functions use `onlyOwner`, which is consistent with the previous behavior. Test `test_register_revertUnauthorizedCaller` validates this by expecting `"Ownable: caller is not the owner"`.

6. **No remaining custom ownership references**: Searched for `Unauthorized`, `OwnerTransferred`, and custom ownership patterns across all TEE source files -- zero matches in source code. Test files reference `"Ownable: caller is not the owner"` (the OZ error string), confirming the migration.

**Status: FIXED** -- The fix fully addresses the finding. OZ Ownable v4 is a well-audited, battle-tested implementation that provides zero-address validation, standard event names, and a clean API.

---

## 3. New Findings

### N-01: `renounceOwnership()` Is Inherited and Not Disabled

**Severity:** Low
**File:** `src/dispute/tee/TeeProofVerifier.sol`

**Description:**
OpenZeppelin Ownable v4 exposes `renounceOwnership()`, which sets the owner to `address(0)`. If the owner accidentally calls this function, all admin capabilities (enclave registration and revocation) are permanently lost. The `transferOwnership()` zero-address check does not protect against `renounceOwnership()`, which deliberately bypasses it via the internal `_transferOwnership(address(0))`.

**Impact:**
Same as original M-02 -- permanent loss of admin control. However, the risk is lower because `renounceOwnership()` requires an explicit, deliberate call (not an accidental zero-address parameter), and the function name clearly communicates its intent.

**Recommendation:**
Override `renounceOwnership()` to revert unconditionally:
```solidity
function renounceOwnership() public override onlyOwner {
    revert("TeeProofVerifier: renounce disabled");
}
```
Alternatively, document that `renounceOwnership()` should never be called and rely on operational procedures.

---

## 4. Test Coverage Assessment

### M-01 Fix Tests

| Test | File | What It Verifies |
|------|------|------------------|
| `test_prove_revertUnauthorizedProver` | `TeeDisputeGame.t.sol:551` | Non-proposer address gets `BadAuth` |
| `test_prove_succeedsWithSingleBatch` | `TeeDisputeGame.t.sol:223` | Proposer can prove, recorded as prover |
| `test_prove_succeedsWithChainedBatches` | `TeeDisputeGame.t.sol:250` | Multi-batch proving by proposer |
| `test_lifecycle_challenged_proveByProposer_defenderWins` | `TeeDisputeGameIntegration.t.sol:130` | Full lifecycle: challenge + proposer proves + resolve + claim |
| `test_lifecycle_viaRouter_fullCycle` | `TeeDisputeGameIntegration.t.sol:456` | Prove via router with proposer attribution |
| `test_claimCredit_challengerWinsNormalMode` | `TeeDisputeGame.t.sol:579` | Simplified bond distribution (challenger takes all) |
| `test_claimCredit_refundModeWhenBlacklisted` | `TeeDisputeGame.t.sol:607` | REFUND mode with proposer proving |

**Assessment:** Excellent coverage. Both the access control rejection and the happy-path with simplified bond distribution are tested. Integration tests cover end-to-end flows.

### M-02 Fix Tests

| Test | File | What It Verifies |
|------|------|------------------|
| `test_transferOwnership_updatesOwner` | `TeeProofVerifier.t.sol:121` | Standard ownership transfer works |
| `test_transferOwnership_revertZeroAddress` | `TeeProofVerifier.t.sol:127` | Zero-address transfer is rejected |
| `test_register_revertUnauthorizedCaller` | `TeeProofVerifier.t.sol:36` | Non-owner cannot register (uses OZ error string) |

**Assessment:** Good coverage for the ownership fix. The zero-address rejection test directly validates the M-02 fix. A test for `renounceOwnership()` behavior would strengthen coverage (see N-01).

### Overall Test Quality

- **4 test files** cover the TEE dispute game system: unit tests (`TeeDisputeGame.t.sol`), verifier tests (`TeeProofVerifier.t.sol`), integration tests (`TeeDisputeGameIntegration.t.sol`), and ASR compatibility tests (`AnchorStateRegistryCompatibility.t.sol`).
- Integration tests use real `DisputeGameFactory`, `AnchorStateRegistry`, and `TeeProofVerifier` contracts (only `RiscZeroVerifier` and `SystemConfig` are mocked).
- All game lifecycle paths are covered: unchallenged timeout, challenged + proved, challenged + timeout, blacklisted/refund, parent-child chaining, parent-loses scenarios, cross-chain isolation.
- Error paths are well-tested with `vm.expectRevert` for all custom errors.

---

## 5. Residual Items from Post-Refactor Audit

| ID   | Severity       | Title                                                    | Status Post-Fix     |
|------|----------------|----------------------------------------------------------|---------------------|
| M-01 | Medium         | `prove()` permissionless -- prover credit frontrunnable  | **Fixed**           |
| M-02 | Medium         | `transferOwnership()` lacks zero-address check           | **Fixed**           |
| L-01 | Low            | `tx.origin` for proposer authentication                  | Acknowledged (by design) |
| L-02 | Low            | `_extractAddress` memory loop                            | Open                |
| L-03 | Low            | `expectedRootKey` not immutable                          | Open                |
| I-01 | Informational  | Unused error `ClaimNotChallenged`                        | Open                |
| I-02 | Informational  | Unused error `UnexpectedGameType`                        | Open                |
| I-03 | Informational  | `closeGame()` silently ignores `setAnchorState` failures | Acknowledged        |
| I-04 | Informational  | No `receive()`/`fallback()`                              | By Design           |
| I-05 | Informational  | AccessManager refactor completeness                      | Confirmed           |
| N-01 | Low (New)      | `renounceOwnership()` inherited, not disabled            | Open                |

---

## 6. Conclusion

Both Medium-severity findings from the post-refactor audit have been correctly and completely fixed:

- **M-01** is resolved by restricting `prove()` to the proposer via `msg.sender` check, eliminating the frontrunning vector entirely. The `resolve()` bond distribution was properly simplified to reflect the proposer-only proving model.

- **M-02** is resolved by replacing the custom ownership implementation with OpenZeppelin Ownable v4, which provides built-in zero-address validation on `transferOwnership()`, a standard `onlyOwner` modifier, and the well-known `OwnershipTransferred` event.

The fixes are clean, minimal, and do not introduce storage layout issues or breaking behavioral changes. Test coverage adequately validates both the fix correctness and the absence of regressions.

One new low-severity observation (N-01: inherited `renounceOwnership()` is not disabled) is noted for consideration but does not block deployment.

**Final Assessment: The TeeDisputeGame system is ready for deployment. All Medium findings are resolved, and the codebase maintains its strong security posture.**
