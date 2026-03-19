# TEE TEE Dispute Game — Integration Test Plan

## Background

Current test coverage has three layers:

| Layer | Files | Characteristics |
|-------|-------|-----------------|
| Unit tests | 5 files, ~50 tests | All dependencies mocked, isolated per contract |
| Integration test | `AnchorStateRegistryCompatibility.t.sol` (1 test) | Real ASR + Proxy, but Factory and TeeProofVerifier are mocked |
| Fork E2E | `DisputeGameFactoryRouterFork.t.sol` (3 tests) | Mainnet fork, requires `ETH_RPC_URL`, skipped otherwise |

### Problem

The integration layer only covers **one happy path** (unchallenged → prove → DEFENDER_WINS → setAnchorState). The fork tests cover a challenged DEFENDER_WINS path but are **conditional on `ETH_RPC_URL`** — if CI doesn't configure it, these paths have zero real-contract coverage.

Critical paths involving **fund distribution (REFUND vs NORMAL)**, **parent-child game chains**, and **third-party prover bond splitting** have never been verified with real ASR + real Factory together.

## Goal

Create `TeeDisputeGameIntegration.t.sol` — a non-fork integration test suite that verifies the TEE TEE dispute game full lifecycle with real contracts, runnable in CI without any RPC dependency.

## Contract Setup

### Real contracts (deployed via Proxy where applicable)

| Contract | Deploy Method | Notes |
|----------|--------------|-------|
| `DisputeGameFactory` | Proxy + `initialize(owner)` | Replaces `MockDisputeGameFactory` |
| `AnchorStateRegistry` | Proxy + `initialize(...)` | Already real in current integration test |
| `TeeProofVerifier` | `new TeeProofVerifier(verifier, imageId, rootKey)` | With real enclave registration flow |
| `TeeDisputeGame` | Implementation set in factory, cloned on `create()` | Real game logic |
| `AccessManager` | `new AccessManager(timeout, factory)` | Already real |
| `DisputeGameFactoryRouter` | `new DisputeGameFactoryRouter()` | For Router-based creation tests |

### Mocks (minimal, non-critical)

| Mock | Reason |
|------|--------|
| `MockRiscZeroVerifier` | ZK proof verification is out of scope; only need it to not revert during `register()` |
| `MockSystemConfig` | Provides `paused()` and `guardian`; no real SystemConfig needed for these tests |

## Test Cases

### Test 1: `test_lifecycle_unchallenged_defenderWins`

**Path**: create → (no challenge) → wait MAX_CHALLENGE_DURATION → resolve → closeGame → claimCredit

**Verifies**:
- Simplest happy path with real Factory + real ASR
- `resolve()` returns `DEFENDER_WINS` when unchallenged and time expires
- `closeGame()` → `ASR.isGameFinalized()` passes → `bondDistributionMode = NORMAL`
- `setAnchorState` succeeds, `anchorGame` updates
- Proposer receives back `DEFENDER_BOND`

---

### Test 2: `test_lifecycle_challenged_proveByProposer_defenderWins`

**Path**: create → challenge → proposer proves → resolve → closeGame → claimCredit

**Verifies**:
- Challenge + prove flow with real TeeProofVerifier (registered enclave)
- `resolve()` returns `DEFENDER_WINS`
- `closeGame()` → `bondDistributionMode = NORMAL`
- Proposer receives `DEFENDER_BOND + CHALLENGER_BOND` (wins challenger's bond)
- `setAnchorState` succeeds

---

### Test 3: `test_lifecycle_challenged_proveByThirdParty_bondSplit`

**Path**: create → challenge → third-party proves → resolve → closeGame → claimCredit (proposer) + claimCredit (prover)

**Verifies**:
- Third-party prover bond splitting with real ASR determining `bondDistributionMode`
- `bondDistributionMode = NORMAL`
- Proposer receives `DEFENDER_BOND`, prover receives `CHALLENGER_BOND`
- Both `claimCredit` calls succeed with correct amounts

---

### Test 4: `test_lifecycle_challenged_timeout_challengerWins_refund`

**Path**: create → challenge → (no prove) → wait MAX_PROVE_DURATION → resolve → closeGame → claimCredit

**Verifies**:
- **REFUND mode** — the most critical untested path with real contracts
- `resolve()` returns `CHALLENGER_WINS`
- `closeGame()` → `ASR.isGameProper()` returns false for CHALLENGER_WINS → `bondDistributionMode = REFUND`
- `setAnchorState` is attempted but silently fails (try-catch in `closeGame`)
- `anchorGame` does NOT update
- Proposer gets back `DEFENDER_BOND`, challenger gets back `CHALLENGER_BOND` (each refunded their own deposit)

---

### Test 5: `test_lifecycle_parentChildChain_defenderWins`

**Path**: create parent → resolve parent (DEFENDER_WINS) → create child (parentIndex=0) → resolve child

**Verifies**:
- Child game's `startingOutputRoot` comes from parent's rootClaim (not anchor state)
- Child cannot resolve before parent resolves (revert `ParentGameNotResolved`)
- After parent resolves, child lifecycle works normally
- Real Factory's `gameAtIndex()` is used to look up parent — validates the full lookup chain

---

### Test 6: `test_lifecycle_parentChallengerWins_childShortCircuits`

**Path**: create parent → challenge parent → parent timeout → resolve parent (CHALLENGER_WINS) → create child → challenge child → resolve child

**Verifies**:
- When parent is `CHALLENGER_WINS`, child's resolve short-circuits to `CHALLENGER_WINS`
- Bond distribution for short-circuited child: challenger gets `DEFENDER_BOND + CHALLENGER_BOND`
- Tests the cascading failure propagation through game chains

---

### Test 7: `test_lifecycle_viaRouter_fullCycle`

**Path**: Router.create → challenge → prove → resolve → closeGame → claimCredit

**Verifies**:
- `gameCreator()` is the Router address
- `proposer()` is `tx.origin` (transparent pass-through)
- Full lifecycle works identically when created via Router vs direct Factory call
- Bond accounting attributes correctly to tx.origin proposer, not Router

## Shared Test Infrastructure

Reuse `TeeTestUtils` as base contract. Add a shared `setUp()` helper that deploys the full real-contract stack:

```solidity
function _deployFullStack() internal {
    // 1. Deploy real DisputeGameFactory via Proxy
    // 2. Deploy real AnchorStateRegistry via Proxy
    // 3. Deploy real TeeProofVerifier (with MockRiscZeroVerifier)
    //    - Register enclave via real register() flow
    // 4. Deploy real AccessManager
    // 5. Deploy real TeeDisputeGame implementation
    // 6. Register implementation + init bond in factory
    // 7. (Optional) Deploy DisputeGameFactoryRouter + register zone
}
```

## Relationship to Existing Tests

| File | Action |
|------|--------|
| `AnchorStateRegistryCompatibility.t.sol` | Can be removed or kept as-is; the new integration tests fully subsume it |
| `DisputeGameFactoryRouterFork.t.sol` | Keep — it uniquely tests XLayer cross-zone interop on mainnet fork |
| Unit test files | Keep — they test error paths and edge cases exhaustively with fast mock-based isolation |
