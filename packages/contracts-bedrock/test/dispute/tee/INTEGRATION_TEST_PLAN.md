# TEE Dispute Game — Integration Test Plan

## Status: IMPLEMENTED

**File**: `TeeDisputeGameIntegration.t.sol` — 9 tests, all passing.

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

## Contract Setup

### Real contracts (deployed via Proxy where applicable)

| Contract | Deploy Method | Notes |
|----------|--------------|-------|
| `DisputeGameFactory` | Proxy + `initialize(owner)` | Replaces `MockDisputeGameFactory` |
| `AnchorStateRegistry` | Proxy + `initialize(...)` | DISPUTE_GAME_FINALITY_DELAY_SECONDS = 0 |
| `TeeProofVerifier` | `new TeeProofVerifier(verifier, imageId, rootKey)` | With real enclave registration flow |
| `TeeDisputeGame` | Implementation set in factory, cloned on `create()` | Real game logic |
| `AccessManager` | `new AccessManager(timeout, factory)` | Manages proposer/challenger permissions |
| `DisputeGameFactoryRouter` | `new DisputeGameFactoryRouter()` | For Router-based creation tests |

### Mocks (minimal, non-critical)

| Mock | Reason |
|------|--------|
| `MockRiscZeroVerifier` | ZK proof verification is out of scope; only need it to not revert during `register()` |
| `MockSystemConfig` | Provides `paused()` and `guardian`; test contract acts as guardian for blacklist tests |

## Test Cases

### Test 1: `test_lifecycle_unchallenged_defenderWins`

**Path**: create → (no challenge) → wait MAX_CHALLENGE_DURATION → resolve → closeGame → claimCredit

**Verifies**:
- Simplest happy path with real Factory + real ASR
- `resolve()` returns `DEFENDER_WINS` when unchallenged and time expires
- `closeGame()` reverts with `GameNotFinalized` before finality delay passes
- After finality: `bondDistributionMode = NORMAL`, `setAnchorState` succeeds
- Proposer receives back `DEFENDER_BOND`

---

### Test 2: `test_lifecycle_challenged_proveByProposer_defenderWins`

**Path**: create → challenge → proposer proves → resolve → closeGame → claimCredit

**Verifies**:
- Challenge + prove flow with real TeeProofVerifier (registered enclave)
- `resolve()` returns `DEFENDER_WINS`
- `bondDistributionMode = NORMAL`
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

### Test 4: `test_lifecycle_challenged_timeout_challengerWins`

**Path**: create → challenge → (no prove) → wait MAX_PROVE_DURATION → resolve → closeGame → claimCredit

**Verifies**:
- `resolve()` returns `CHALLENGER_WINS`
- `bondDistributionMode = NORMAL` (game is still "proper" per real ASR — not blacklisted/retired/paused)
- Challenger receives `DEFENDER_BOND + CHALLENGER_BOND` (takes all)
- Proposer has zero credit — loses bond
- `setAnchorState` silently fails (requires DEFENDER_WINS), anchor does NOT update

**Key finding during implementation**: Real ASR considers a CHALLENGER_WINS game "proper" (registered, not blacklisted, not retired, not paused). The unit test used MockASR with manually set flags to force REFUND, which masked this behavior. REFUND mode only occurs via guardian intervention (blacklist) or system pause.

---

### Test 4b: `test_lifecycle_blacklisted_refund`

**Path**: create → challenge → prove → resolve (DEFENDER_WINS) → guardian blacklists → closeGame → REFUND

**Verifies**:
- Guardian blacklist triggers REFUND mode even for a DEFENDER_WINS game
- `isGameProper()` returns false after blacklisting
- Each party gets their deposit back: proposer gets `DEFENDER_BOND`, challenger gets `CHALLENGER_BOND`
- `setAnchorState` silently fails (blacklisted), anchor does NOT update

---

### Test 5: `test_lifecycle_parentChildChain_defenderWins`

**Path**: create parent → resolve parent (DEFENDER_WINS) → create child (parentIndex=0) → prove child → resolve child

**Verifies**:
- Child game's `startingOutputRoot` comes from parent's rootClaim (not anchor state)
- After parent resolves and becomes anchor, child lifecycle works normally
- Real Factory's `gameAtIndex()` is used to look up parent — validates the full lookup chain
- Child becomes the new anchor (higher l2SequenceNumber)

---

### Test 6: `test_lifecycle_parentChallengerWins_childShortCircuits`

**Path**: create parent → create child (while parent IN_PROGRESS) → challenge child → challenge parent → parent timeout → resolve parent (CHALLENGER_WINS) → resolve child

**Verifies**:
- Child's resolve short-circuits to `CHALLENGER_WINS` when parent lost
- Bond distribution for short-circuited child: challenger gets `DEFENDER_BOND + CHALLENGER_BOND`
- Tests the cascading failure propagation through game chains

**Key finding during implementation**: `initialize()` checks `proxy.status() == GameStatus.CHALLENGER_WINS` and reverts with `InvalidParentGame`. The child MUST be created while parent is still `IN_PROGRESS`. The short-circuit only happens at `resolve()` time, not at creation time.

---

### Test 7: `test_lifecycle_childCannotResolveBeforeParent`

**Path**: create parent → create child → (fast forward) → child.resolve() reverts → parent.resolve() → child.resolve() succeeds

**Verifies**:
- `ParentGameNotResolved` revert when parent is still `IN_PROGRESS`
- After parent resolves, child can resolve normally
- Ordering dependency between parent and child resolution

---

### Test 8: `test_lifecycle_viaRouter_fullCycle`

**Path**: Router.create → challenge → prove → resolve → closeGame → claimCredit

**Verifies**:
- `gameCreator()` is the Router address
- `proposer()` is `tx.origin` (transparent pass-through)
- Full lifecycle works identically when created via Router vs direct Factory call
- Bond accounting attributes correctly to tx.origin proposer, not Router
- `refundModeCredit(router)` is zero — Router doesn't capture any bonds

## Key Findings from Implementation

1. **REFUND mode requires guardian intervention**: A CHALLENGER_WINS game is still "proper" per real ASR. The unit test's MockASR with `setGameFlags` to force REFUND was misleading — in production, REFUND only triggers via blacklist or system pause.

2. **Child creation timing constraint**: `initialize()` rejects a parent with `CHALLENGER_WINS` status. Children must be created while parent is `IN_PROGRESS`. The cascading failure only manifests at `resolve()` time.

3. **Finality delay is load-bearing**: `closeGame()` requires `isGameFinalized()` which checks `resolvedAt + DISPUTE_GAME_FINALITY_DELAY_SECONDS < block.timestamp`. Even with delay=0, `vm.warp(block.timestamp + 1)` is needed after resolve.

## Relationship to Existing Tests

| File | Action |
|------|--------|
| `AnchorStateRegistryCompatibility.t.sol` | Subsumed by the new integration tests; can be removed |
| `DisputeGameFactoryRouterFork.t.sol` | Keep — it uniquely tests XLayer cross-zone interop on mainnet fork |
| Unit test files | Keep — they test error paths and edge cases exhaustively with fast mock-based isolation |
