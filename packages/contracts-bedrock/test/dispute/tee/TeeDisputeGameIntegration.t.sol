// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import {Vm} from "forge-std/Vm.sol";
import {Proxy} from "src/universal/Proxy.sol";
import {AnchorStateRegistry} from "src/dispute/AnchorStateRegistry.sol";
import {DisputeGameFactory} from "src/dispute/DisputeGameFactory.sol";
import {IDisputeGameFactory} from "interfaces/dispute/IDisputeGameFactory.sol";
import {IDisputeGame} from "interfaces/dispute/IDisputeGame.sol";
import {IAnchorStateRegistry} from "interfaces/dispute/IAnchorStateRegistry.sol";
import {ISystemConfig} from "interfaces/L1/ISystemConfig.sol";
import {ITeeProofVerifier} from "interfaces/dispute/ITeeProofVerifier.sol";
import {TeeDisputeGame} from "src/dispute/tee/TeeDisputeGame.sol";
import {TeeProofVerifier} from "src/dispute/tee/TeeProofVerifier.sol";
import {AccessManager, TEE_DISPUTE_GAME_TYPE} from "src/dispute/tee/AccessManager.sol";
import {DisputeGameFactoryRouter} from "src/dispute/DisputeGameFactoryRouter.sol";
import {
    BondDistributionMode,
    Claim,
    Duration,
    GameStatus,
    GameType,
    Hash,
    Proposal
} from "src/dispute/lib/Types.sol";
import {GameNotFinalized} from "src/dispute/lib/Errors.sol";
import {ParentGameNotResolved} from "src/dispute/tee/lib/Errors.sol";
import {TeeTestUtils} from "test/dispute/tee/helpers/TeeTestUtils.sol";
import {MockRiscZeroVerifier} from "test/dispute/tee/mocks/MockRiscZeroVerifier.sol";
import {MockSystemConfig} from "test/dispute/tee/mocks/MockSystemConfig.sol";

/// @title TeeDisputeGameIntegrationTest
/// @notice Integration tests for the full TEE dispute game lifecycle using real contracts.
///         Only MockRiscZeroVerifier and MockSystemConfig are mocked; all core contracts
///         (DisputeGameFactory, AnchorStateRegistry, TeeProofVerifier, AccessManager) are real.
contract TeeDisputeGameIntegrationTest is TeeTestUtils {
    uint256 internal constant DEFENDER_BOND = 1 ether;
    uint256 internal constant CHALLENGER_BOND = 2 ether;
    uint64 internal constant MAX_CHALLENGE_DURATION = 1 days;
    uint64 internal constant MAX_PROVE_DURATION = 12 hours;
    bytes32 internal constant IMAGE_ID = keccak256("integration-tee-image");
    bytes32 internal constant PCR_HASH = keccak256("integration-pcr-hash");

    bytes32 internal constant ANCHOR_BLOCK_HASH = keccak256("anchor-block");
    bytes32 internal constant ANCHOR_STATE_HASH = keccak256("anchor-state");
    uint256 internal constant ANCHOR_L2_BLOCK = 10;

    GameType internal constant TEE_GAME_TYPE = GameType.wrap(TEE_DISPUTE_GAME_TYPE);

    DisputeGameFactory internal factory;
    AnchorStateRegistry internal anchorStateRegistry;
    TeeProofVerifier internal teeProofVerifier;
    AccessManager internal accessManager;
    TeeDisputeGame internal implementation;

    address internal proposer;
    address internal challenger;
    address internal executor;
    address internal thirdPartyProver;

    function setUp() public {
        proposer = makeWallet(DEFAULT_PROPOSER_KEY, "proposer").addr;
        challenger = makeWallet(DEFAULT_CHALLENGER_KEY, "challenger").addr;
        executor = makeWallet(DEFAULT_EXECUTOR_KEY, "executor").addr;
        thirdPartyProver = makeWallet(DEFAULT_THIRD_PARTY_PROVER_KEY, "third-party-prover").addr;

        vm.deal(proposer, 100 ether);
        vm.deal(challenger, 100 ether);

        // --- Deploy real DisputeGameFactory via Proxy ---
        factory = _deployFactory();

        // --- Deploy real AnchorStateRegistry via Proxy ---
        anchorStateRegistry = _deployAnchorStateRegistry(factory);

        // --- Deploy real TeeProofVerifier (with MockRiscZeroVerifier) ---
        teeProofVerifier = _deployTeeProofVerifier();

        // --- Deploy real AccessManager ---
        accessManager = new AccessManager(MAX_CHALLENGE_DURATION, IDisputeGameFactory(address(factory)));
        accessManager.setProposer(proposer, true);
        accessManager.setChallenger(challenger, true);

        // --- Deploy TeeDisputeGame implementation ---
        implementation = new TeeDisputeGame(
            Duration.wrap(MAX_CHALLENGE_DURATION),
            Duration.wrap(MAX_PROVE_DURATION),
            IDisputeGameFactory(address(factory)),
            ITeeProofVerifier(address(teeProofVerifier)),
            CHALLENGER_BOND,
            IAnchorStateRegistry(address(anchorStateRegistry)),
            accessManager
        );

        factory.setImplementation(TEE_GAME_TYPE, IDisputeGame(address(implementation)), bytes(""));
        factory.setInitBond(TEE_GAME_TYPE, DEFENDER_BOND);

        // Warp past the retirement timestamp so games are not retired.
        vm.warp(block.timestamp + 1);
    }

    ////////////////////////////////////////////////////////////////
    //               Test 1: Unchallenged DEFENDER_WINS            //
    ////////////////////////////////////////////////////////////////

    /// @notice create → (no challenge) → timeout → resolve → closeGame → claimCredit
    function test_lifecycle_unchallenged_defenderWins() public {
        (TeeDisputeGame game,,) = _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, keccak256("end-block"), keccak256("end-state"));

        // Wait for challenge window to expire
        vm.warp(block.timestamp + MAX_CHALLENGE_DURATION + 1);

        // Resolve — unchallenged, so DEFENDER_WINS
        assertEq(uint8(game.resolve()), uint8(GameStatus.DEFENDER_WINS));

        // closeGame not yet callable — need finality delay
        vm.expectRevert(GameNotFinalized.selector);
        game.closeGame();

        // Wait for finality delay (DISPUTE_GAME_FINALITY_DELAY_SECONDS = 0 in our ASR)
        vm.warp(block.timestamp + 1);
        assertTrue(anchorStateRegistry.isGameFinalized(game));

        // claimCredit triggers closeGame → setAnchorState → NORMAL mode
        uint256 proposerBalanceBefore = proposer.balance;
        game.claimCredit(proposer);

        assertEq(uint8(game.bondDistributionMode()), uint8(BondDistributionMode.NORMAL));
        assertEq(proposer.balance, proposerBalanceBefore + DEFENDER_BOND);
        assertEq(address(anchorStateRegistry.anchorGame()), address(game));
    }

    ////////////////////////////////////////////////////////////////
    //     Test 2: Challenged + Proposer Proves → DEFENDER_WINS   //
    ////////////////////////////////////////////////////////////////

    /// @notice create → challenge → proposer proves → resolve → claimCredit
    function test_lifecycle_challenged_proveByProposer_defenderWins() public {
        bytes32 endBlockHash = keccak256("end-block");
        bytes32 endStateHash = keccak256("end-state");
        (TeeDisputeGame game,,) = _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, endBlockHash, endStateHash);

        // Challenger challenges
        vm.prank(challenger);
        game.challenge{value: CHALLENGER_BOND}();

        // Proposer proves with real TeeProofVerifier
        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](1);
        proofs[0] = buildBatchProof(
            BatchInput({
                startBlockHash: ANCHOR_BLOCK_HASH,
                startStateHash: ANCHOR_STATE_HASH,
                endBlockHash: endBlockHash,
                endStateHash: endStateHash,
                l2Block: game.l2SequenceNumber()
            }),
            DEFAULT_EXECUTOR_KEY
        );

        vm.prank(proposer);
        game.prove(abi.encode(proofs));

        // Resolve — challenged + proved by proposer → DEFENDER_WINS, proposer gets all
        assertEq(uint8(game.resolve()), uint8(GameStatus.DEFENDER_WINS));
        assertEq(game.normalModeCredit(proposer), DEFENDER_BOND + CHALLENGER_BOND);

        // Wait for finality
        vm.warp(block.timestamp + 1);

        uint256 proposerBalanceBefore = proposer.balance;
        game.claimCredit(proposer);

        assertEq(uint8(game.bondDistributionMode()), uint8(BondDistributionMode.NORMAL));
        assertEq(proposer.balance, proposerBalanceBefore + DEFENDER_BOND + CHALLENGER_BOND);
        assertEq(address(anchorStateRegistry.anchorGame()), address(game));
    }

    ////////////////////////////////////////////////////////////////
    //   Test 3: Challenged + Third-Party Proves → Bond Split     //
    ////////////////////////////////////////////////////////////////

    /// @notice create → challenge → third-party proves → resolve → claimCredit (proposer + prover)
    function test_lifecycle_challenged_proveByThirdParty_bondSplit() public {
        bytes32 endBlockHash = keccak256("end-block");
        bytes32 endStateHash = keccak256("end-state");
        (TeeDisputeGame game,,) = _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, endBlockHash, endStateHash);

        // Challenger challenges
        vm.prank(challenger);
        game.challenge{value: CHALLENGER_BOND}();

        // Third-party prover proves
        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](1);
        proofs[0] = buildBatchProof(
            BatchInput({
                startBlockHash: ANCHOR_BLOCK_HASH,
                startStateHash: ANCHOR_STATE_HASH,
                endBlockHash: endBlockHash,
                endStateHash: endStateHash,
                l2Block: game.l2SequenceNumber()
            }),
            DEFAULT_EXECUTOR_KEY
        );

        vm.prank(thirdPartyProver);
        game.prove(abi.encode(proofs));

        // Resolve — third-party proved → DEFENDER_WINS with split
        assertEq(uint8(game.resolve()), uint8(GameStatus.DEFENDER_WINS));
        assertEq(game.normalModeCredit(proposer), DEFENDER_BOND);
        assertEq(game.normalModeCredit(thirdPartyProver), CHALLENGER_BOND);

        // Wait for finality
        vm.warp(block.timestamp + 1);

        // Both claim credit
        uint256 proposerBalanceBefore = proposer.balance;
        uint256 proverBalanceBefore = thirdPartyProver.balance;
        game.claimCredit(proposer);
        game.claimCredit(thirdPartyProver);

        assertEq(uint8(game.bondDistributionMode()), uint8(BondDistributionMode.NORMAL));
        assertEq(proposer.balance, proposerBalanceBefore + DEFENDER_BOND);
        assertEq(thirdPartyProver.balance, proverBalanceBefore + CHALLENGER_BOND);
        assertEq(address(anchorStateRegistry.anchorGame()), address(game));
    }

    ////////////////////////////////////////////////////////////////
    //   Test 4: Challenged + Timeout → CHALLENGER_WINS → NORMAL  //
    ////////////////////////////////////////////////////////////////

    /// @notice create → challenge → (no prove) → timeout → resolve → NORMAL → challenger takes all
    /// @dev A CHALLENGER_WINS game is still "proper" per ASR (registered, not blacklisted,
    ///      not retired, not paused), so closeGame → NORMAL mode. The challenger wins all bonds.
    function test_lifecycle_challenged_timeout_challengerWins() public {
        (TeeDisputeGame game,,) = _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, keccak256("end-block"), keccak256("end-state"));

        // Challenger challenges
        vm.prank(challenger);
        game.challenge{value: CHALLENGER_BOND}();

        // Nobody proves — wait for prove deadline
        vm.warp(block.timestamp + MAX_PROVE_DURATION + 1);

        // Resolve — challenged + no proof → CHALLENGER_WINS
        assertEq(uint8(game.resolve()), uint8(GameStatus.CHALLENGER_WINS));
        assertEq(game.normalModeCredit(challenger), DEFENDER_BOND + CHALLENGER_BOND);

        // Wait for finality
        vm.warp(block.timestamp + 1);
        assertTrue(anchorStateRegistry.isGameFinalized(game));

        // Anchor should NOT update (setAnchorState requires DEFENDER_WINS)
        address anchorBefore = address(anchorStateRegistry.anchorGame());

        // Challenger claims all bonds
        uint256 challengerBalanceBefore = challenger.balance;
        game.claimCredit(challenger);

        assertEq(uint8(game.bondDistributionMode()), uint8(BondDistributionMode.NORMAL));
        assertEq(challenger.balance, challengerBalanceBefore + DEFENDER_BOND + CHALLENGER_BOND);

        // Proposer has no credit — lost their bond
        assertEq(game.normalModeCredit(proposer), 0);

        // Anchor state did NOT change
        assertEq(address(anchorStateRegistry.anchorGame()), anchorBefore);
    }

    ////////////////////////////////////////////////////////////////
    //   Test 4b: Blacklisted Game → REFUND                       //
    ////////////////////////////////////////////////////////////////

    /// @notice Guardian blacklists a game → closeGame → REFUND → each gets deposit back
    function test_lifecycle_blacklisted_refund() public {
        bytes32 endBlockHash = keccak256("end-block");
        bytes32 endStateHash = keccak256("end-state");
        (TeeDisputeGame game,,) = _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, endBlockHash, endStateHash);

        // Challenger challenges
        vm.prank(challenger);
        game.challenge{value: CHALLENGER_BOND}();

        // Proposer proves
        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](1);
        proofs[0] = buildBatchProof(
            BatchInput({
                startBlockHash: ANCHOR_BLOCK_HASH,
                startStateHash: ANCHOR_STATE_HASH,
                endBlockHash: endBlockHash,
                endStateHash: endStateHash,
                l2Block: game.l2SequenceNumber()
            }),
            DEFAULT_EXECUTOR_KEY
        );
        vm.prank(proposer);
        game.prove(abi.encode(proofs));

        // Resolve — DEFENDER_WINS
        assertEq(uint8(game.resolve()), uint8(GameStatus.DEFENDER_WINS));

        // Guardian blacklists the game before finalization
        // (address(this) is the guardian via MockSystemConfig)
        anchorStateRegistry.blacklistDisputeGame(game);

        // Wait for finality
        vm.warp(block.timestamp + 1);
        assertTrue(anchorStateRegistry.isGameFinalized(game));
        assertFalse(anchorStateRegistry.isGameProper(game));

        // claimCredit → closeGame → isGameProper = false → REFUND mode
        uint256 proposerBalanceBefore = proposer.balance;
        uint256 challengerBalanceBefore = challenger.balance;
        game.claimCredit(proposer);
        game.claimCredit(challenger);

        assertEq(uint8(game.bondDistributionMode()), uint8(BondDistributionMode.REFUND));
        assertEq(proposer.balance, proposerBalanceBefore + DEFENDER_BOND);
        assertEq(challenger.balance, challengerBalanceBefore + CHALLENGER_BOND);
    }

    ////////////////////////////////////////////////////////////////
    //      Test 5: Parent-Child Chain → DEFENDER_WINS             //
    ////////////////////////////////////////////////////////////////

    /// @notice create parent → resolve parent → create child (parentIndex=0) → resolve child
    function test_lifecycle_parentChildChain_defenderWins() public {
        bytes32 parentEndBlockHash = keccak256("parent-end-block");
        bytes32 parentEndStateHash = keccak256("parent-end-state");

        // Create parent game (root game, parentIndex = max)
        (TeeDisputeGame parent,,) = _createGame(
            proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, parentEndBlockHash, parentEndStateHash
        );

        // Wait for challenge window and resolve parent
        vm.warp(block.timestamp + MAX_CHALLENGE_DURATION + 1);
        parent.resolve();

        // Wait for parent finality so it can become anchor
        vm.warp(block.timestamp + 1);
        parent.claimCredit(proposer);

        // Create child game referencing parent (parentIndex = 0)
        bytes32 childEndBlockHash = keccak256("child-end-block");
        bytes32 childEndStateHash = keccak256("child-end-state");
        (TeeDisputeGame child,,) = _createGame(
            proposer, ANCHOR_L2_BLOCK + 10, 0, childEndBlockHash, childEndStateHash
        );

        // Verify child's startingOutputRoot comes from parent
        (Hash childStartRoot, uint256 childStartBlock) = child.startingOutputRoot();
        assertEq(childStartRoot.raw(), computeRootClaim(parentEndBlockHash, parentEndStateHash).raw());
        assertEq(childStartBlock, ANCHOR_L2_BLOCK + 5);

        // Prove and resolve child
        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](1);
        proofs[0] = buildBatchProof(
            BatchInput({
                startBlockHash: parentEndBlockHash,
                startStateHash: parentEndStateHash,
                endBlockHash: childEndBlockHash,
                endStateHash: childEndStateHash,
                l2Block: child.l2SequenceNumber()
            }),
            DEFAULT_EXECUTOR_KEY
        );

        vm.prank(proposer);
        child.prove(abi.encode(proofs));

        assertEq(uint8(child.resolve()), uint8(GameStatus.DEFENDER_WINS));

        // Wait for finality and claim
        vm.warp(block.timestamp + 1);
        child.claimCredit(proposer);

        assertEq(uint8(child.bondDistributionMode()), uint8(BondDistributionMode.NORMAL));
        // Child should be the new anchor (higher l2SequenceNumber)
        assertEq(address(anchorStateRegistry.anchorGame()), address(child));
    }

    ////////////////////////////////////////////////////////////////
    //  Test 6: Parent CHALLENGER_WINS → Child Short-Circuits      //
    ////////////////////////////////////////////////////////////////

    /// @notice parent CHALLENGER_WINS → child resolve short-circuits to CHALLENGER_WINS
    /// @dev Child must be created while parent is still IN_PROGRESS (initialize rejects
    ///      a CHALLENGER_WINS parent). The short-circuit happens at resolve() time.
    function test_lifecycle_parentChallengerWins_childShortCircuits() public {
        bytes32 parentEndBlockHash = keccak256("parent-end-block");
        bytes32 parentEndStateHash = keccak256("parent-end-state");

        // Create parent (still IN_PROGRESS)
        (TeeDisputeGame parent,,) = _createGame(
            proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, parentEndBlockHash, parentEndStateHash
        );

        // Create child BEFORE parent resolves (parentIndex = 0)
        bytes32 childEndBlockHash = keccak256("child-end-block");
        bytes32 childEndStateHash = keccak256("child-end-state");
        (TeeDisputeGame child,,) = _createGame(
            proposer, ANCHOR_L2_BLOCK + 10, 0, childEndBlockHash, childEndStateHash
        );

        // Challenge child so there's a challenger to receive bonds
        vm.prank(challenger);
        child.challenge{value: CHALLENGER_BOND}();

        // Now challenge parent and let it timeout → CHALLENGER_WINS
        vm.prank(challenger);
        parent.challenge{value: CHALLENGER_BOND}();

        vm.warp(block.timestamp + MAX_PROVE_DURATION + 1);
        parent.resolve();
        assertEq(uint8(parent.status()), uint8(GameStatus.CHALLENGER_WINS));

        // Child resolve short-circuits to CHALLENGER_WINS because parent lost
        assertEq(uint8(child.resolve()), uint8(GameStatus.CHALLENGER_WINS));

        // Challenger gets all child bonds
        assertEq(child.normalModeCredit(challenger), DEFENDER_BOND + CHALLENGER_BOND);

        // Wait for finality and claim
        vm.warp(block.timestamp + 1);
        uint256 challengerBalanceBefore = challenger.balance;
        child.claimCredit(challenger);

        assertEq(uint8(child.bondDistributionMode()), uint8(BondDistributionMode.NORMAL));
        assertEq(challenger.balance, challengerBalanceBefore + DEFENDER_BOND + CHALLENGER_BOND);
    }

    ////////////////////////////////////////////////////////////////
    //  Test 7: Child Cannot Resolve Before Parent                 //
    ////////////////////////////////////////////////////////////////

    /// @notice child.resolve() reverts with ParentGameNotResolved when parent is IN_PROGRESS
    function test_lifecycle_childCannotResolveBeforeParent() public {
        // Create parent (unchallenged, still in progress)
        (TeeDisputeGame parent,,) = _createGame(
            proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, keccak256("parent-end-block"), keccak256("parent-end-state")
        );

        // Create child referencing parent
        (TeeDisputeGame child,,) = _createGame(
            proposer, ANCHOR_L2_BLOCK + 10, 0, keccak256("child-end-block"), keccak256("child-end-state")
        );

        // Fast forward past child's challenge window
        vm.warp(block.timestamp + MAX_CHALLENGE_DURATION + 1);

        // Child cannot resolve because parent is still IN_PROGRESS
        vm.expectRevert(ParentGameNotResolved.selector);
        child.resolve();

        // Now resolve parent first
        parent.resolve();

        // Now child can resolve
        assertEq(uint8(child.resolve()), uint8(GameStatus.DEFENDER_WINS));
    }

    ////////////////////////////////////////////////////////////////
    //     Test 8: Full Cycle via Router                           //
    ////////////////////////////////////////////////////////////////

    /// @notice Router.create → challenge → prove → resolve → claimCredit
    function test_lifecycle_viaRouter_fullCycle() public {
        DisputeGameFactoryRouter router = new DisputeGameFactoryRouter(address(this));
        uint256 zoneId = 1;
        router.setZone(zoneId, address(factory));

        bytes32 endBlockHash = keccak256("router-end-block");
        bytes32 endStateHash = keccak256("router-end-state");
        bytes memory extraData = buildExtraData(ANCHOR_L2_BLOCK + 5, type(uint32).max, endBlockHash, endStateHash);
        Claim rootClaim = computeRootClaim(endBlockHash, endStateHash);

        // Create via router
        vm.startPrank(proposer, proposer);
        address proxy = router.create{value: DEFENDER_BOND}(zoneId, TEE_GAME_TYPE, rootClaim, extraData);
        vm.stopPrank();

        TeeDisputeGame game = TeeDisputeGame(payable(proxy));

        // Verify creator/proposer attribution
        assertEq(game.gameCreator(), address(router));
        assertEq(game.proposer(), proposer);

        // Challenge
        vm.prank(challenger);
        game.challenge{value: CHALLENGER_BOND}();

        // Prove
        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](1);
        proofs[0] = buildBatchProof(
            BatchInput({
                startBlockHash: ANCHOR_BLOCK_HASH,
                startStateHash: ANCHOR_STATE_HASH,
                endBlockHash: endBlockHash,
                endStateHash: endStateHash,
                l2Block: game.l2SequenceNumber()
            }),
            DEFAULT_EXECUTOR_KEY
        );

        vm.prank(proposer);
        game.prove(abi.encode(proofs));

        // Resolve
        assertEq(uint8(game.resolve()), uint8(GameStatus.DEFENDER_WINS));

        // Wait for finality
        vm.warp(block.timestamp + 1);

        // claimCredit — proposer proved, gets all
        uint256 proposerBalanceBefore = proposer.balance;
        game.claimCredit(proposer);

        assertEq(uint8(game.bondDistributionMode()), uint8(BondDistributionMode.NORMAL));
        assertEq(proposer.balance, proposerBalanceBefore + DEFENDER_BOND + CHALLENGER_BOND);
        // Bond attributed to proposer (tx.origin), not to router
        assertEq(game.refundModeCredit(address(router)), 0);
    }

    ////////////////////////////////////////////////////////////////
    //                 Infrastructure Helpers                       //
    ////////////////////////////////////////////////////////////////

    function _deployFactory() internal returns (DisputeGameFactory) {
        DisputeGameFactory impl = new DisputeGameFactory();
        Proxy proxy = new Proxy(address(this));
        proxy.upgradeToAndCall(address(impl), abi.encodeCall(impl.initialize, (address(this))));
        return DisputeGameFactory(address(proxy));
    }

    function _deployAnchorStateRegistry(DisputeGameFactory _factory) internal returns (AnchorStateRegistry) {
        MockSystemConfig systemConfig = new MockSystemConfig(address(this));
        AnchorStateRegistry impl = new AnchorStateRegistry(0);
        Proxy proxy = new Proxy(address(this));
        proxy.upgradeToAndCall(
            address(impl),
            abi.encodeCall(
                impl.initialize,
                (
                    ISystemConfig(address(systemConfig)),
                    IDisputeGameFactory(address(_factory)),
                    Proposal({
                        root: Hash.wrap(computeRootClaim(ANCHOR_BLOCK_HASH, ANCHOR_STATE_HASH).raw()),
                        l2SequenceNumber: ANCHOR_L2_BLOCK
                    }),
                    TEE_GAME_TYPE
                )
            )
        );
        return AnchorStateRegistry(address(proxy));
    }

    function _deployTeeProofVerifier() internal returns (TeeProofVerifier) {
        MockRiscZeroVerifier riscZeroVerifier = new MockRiscZeroVerifier();
        bytes memory expectedRootKey = abi.encodePacked(bytes32(uint256(1)), bytes32(uint256(2)), bytes32(uint256(3)));
        TeeProofVerifier verifier = new TeeProofVerifier(riscZeroVerifier, IMAGE_ID, expectedRootKey);

        // Register the executor enclave via real register() flow
        Vm.Wallet memory enclaveWallet = makeWallet(DEFAULT_EXECUTOR_KEY, "integration-enclave");
        bytes memory journal = buildJournal(1234, PCR_HASH, expectedRootKey, uncompressedPublicKey(enclaveWallet), "");
        verifier.register("", journal);

        return verifier;
    }

    function _createGame(
        address creator,
        uint256 l2SequenceNumber,
        uint32 parentIndex,
        bytes32 blockHash_,
        bytes32 stateHash_
    )
        internal
        returns (TeeDisputeGame game, bytes memory extraData, Claim rootClaim)
    {
        extraData = buildExtraData(l2SequenceNumber, parentIndex, blockHash_, stateHash_);
        rootClaim = computeRootClaim(blockHash_, stateHash_);

        vm.startPrank(creator, creator);
        game = TeeDisputeGame(
            payable(
                address(factory.create{value: DEFENDER_BOND}(TEE_GAME_TYPE, rootClaim, extraData))
            )
        );
        vm.stopPrank();
    }
}
