// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import { IDisputeGameFactory } from "interfaces/dispute/IDisputeGameFactory.sol";
import { IDisputeGame } from "interfaces/dispute/IDisputeGame.sol";
import { IAnchorStateRegistry } from "interfaces/dispute/IAnchorStateRegistry.sol";
import { ITeeProofVerifier } from "interfaces/dispute/ITeeProofVerifier.sol";
import { IAccessManager } from "interfaces/dispute/zk/IAccessManager.sol";
import { AccessManager } from "src/dispute/tee/AccessManager.sol";
import { TeeDisputeGame, TEE_DISPUTE_GAME_TYPE } from "src/dispute/tee/TeeDisputeGame.sol";
import { BadAuth, GameNotFinalized, IncorrectBondAmount, UnexpectedRootClaim } from "src/dispute/lib/Errors.sol";
import {
    ClaimAlreadyChallenged,
    InvalidParentGame,
    ParentGameNotResolved,
    GameOver,
    GameNotOver
} from "src/dispute/tee/lib/Errors.sol";
import { ClaimAlreadyResolved } from "src/dispute/lib/Errors.sol";
import { BondDistributionMode, Duration, GameType, Claim, Hash, GameStatus } from "src/dispute/lib/Types.sol";
import { MockAnchorStateRegistry } from "test/dispute/tee/mocks/MockAnchorStateRegistry.sol";
import { MockDisputeGameFactory } from "test/dispute/tee/mocks/MockDisputeGameFactory.sol";
import { MockStatusDisputeGame } from "test/dispute/tee/mocks/MockStatusDisputeGame.sol";
import { MockTeeProofVerifier } from "test/dispute/tee/mocks/MockTeeProofVerifier.sol";
import { TeeTestUtils } from "test/dispute/tee/helpers/TeeTestUtils.sol";

contract TeeDisputeGameTest is TeeTestUtils {
    uint256 internal constant DEFENDER_BOND = 1 ether;
    uint256 internal constant CHALLENGER_BOND = 2 ether;
    uint64 internal constant MAX_CHALLENGE_DURATION = 1 days;
    uint64 internal constant MAX_PROVE_DURATION = 12 hours;

    bytes32 internal constant ANCHOR_BLOCK_HASH = keccak256("anchor-block");
    bytes32 internal constant ANCHOR_STATE_HASH = keccak256("anchor-state");
    uint256 internal constant ANCHOR_L2_BLOCK = 10;

    MockDisputeGameFactory internal factory;
    MockAnchorStateRegistry internal anchorStateRegistry;
    MockTeeProofVerifier internal teeProofVerifier;
    AccessManager internal accessManager;
    TeeDisputeGame internal implementation;

    address internal proposer;
    address internal challenger;
    address internal executor;

    function setUp() public {
        proposer = makeWallet(DEFAULT_PROPOSER_KEY, "proposer").addr;
        challenger = makeWallet(DEFAULT_CHALLENGER_KEY, "challenger").addr;
        executor = makeWallet(DEFAULT_EXECUTOR_KEY, "executor").addr;

        vm.deal(proposer, 100 ether);
        vm.deal(challenger, 100 ether);

        factory = new MockDisputeGameFactory();
        anchorStateRegistry = new MockAnchorStateRegistry();
        teeProofVerifier = new MockTeeProofVerifier();

        accessManager = new AccessManager(7 days, IDisputeGameFactory(address(factory)));
        accessManager.setProposer(proposer, true);
        accessManager.setChallenger(challenger, true);

        implementation = new TeeDisputeGame(
            Duration.wrap(MAX_CHALLENGE_DURATION),
            Duration.wrap(MAX_PROVE_DURATION),
            IDisputeGameFactory(address(factory)),
            ITeeProofVerifier(address(teeProofVerifier)),
            IAccessManager(address(accessManager)),
            CHALLENGER_BOND,
            IAnchorStateRegistry(address(anchorStateRegistry))
        );

        factory.setImplementation(GameType.wrap(TEE_DISPUTE_GAME_TYPE), implementation);
        factory.setInitBond(GameType.wrap(TEE_DISPUTE_GAME_TYPE), DEFENDER_BOND);

        anchorStateRegistry.setAnchor(
            Hash.wrap(computeRootClaim(ANCHOR_BLOCK_HASH, ANCHOR_STATE_HASH).raw()), ANCHOR_L2_BLOCK
        );
        anchorStateRegistry.setRespectedGameType(GameType.wrap(TEE_DISPUTE_GAME_TYPE));
    }

    function test_initialize_usesAnchorStateForRootGame() public {
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, keccak256("end-block"), keccak256("end-state"));

        (Hash startingRoot, uint256 startingBlockNumber) = game.startingOutputRoot();
        assertEq(startingRoot.raw(), computeRootClaim(ANCHOR_BLOCK_HASH, ANCHOR_STATE_HASH).raw());
        assertEq(startingBlockNumber, ANCHOR_L2_BLOCK);
        assertEq(game.refundModeCredit(proposer), DEFENDER_BOND);
        assertTrue(game.wasRespectedGameTypeWhenCreated());
    }

    function test_initialize_usesParentGameOutput() public {
        MockStatusDisputeGame parent = new MockStatusDisputeGame(
            proposer,
            GameType.wrap(TEE_DISPUTE_GAME_TYPE),
            computeRootClaim(keccak256("parent-block"), keccak256("parent-state")),
            ANCHOR_L2_BLOCK + 3,
            bytes("parent"),
            GameStatus.IN_PROGRESS,
            uint64(block.timestamp),
            0,
            true,
            IAnchorStateRegistry(address(anchorStateRegistry))
        );
        factory.pushGame(
            GameType.wrap(TEE_DISPUTE_GAME_TYPE),
            uint64(block.timestamp),
            IDisputeGame(address(parent)),
            parent.rootClaim(),
            bytes("parent")
        );
        anchorStateRegistry.setGameFlags(IDisputeGame(address(parent)), true, true, false, false, false, true, false);

        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 7, 0, keccak256("child-block"), keccak256("child-state"));

        (Hash startingRoot, uint256 startingBlockNumber) = game.startingOutputRoot();
        assertEq(startingRoot.raw(), parent.rootClaim().raw());
        assertEq(startingBlockNumber, parent.l2SequenceNumber());
    }

    function test_initialize_revertUnauthorizedProposer() public {
        address unauthorized = makeAddr("unauthorized");
        vm.deal(unauthorized, DEFENDER_BOND);

        vm.expectRevert(BadAuth.selector);
        _createGame(unauthorized, ANCHOR_L2_BLOCK + 1, type(uint32).max, keccak256("block"), keccak256("state"));
    }

    function test_initialize_revertRootClaimMismatch() public {
        bytes memory extraData =
            buildExtraData(ANCHOR_L2_BLOCK + 1, type(uint32).max, keccak256("block"), keccak256("state"));
        Claim wrongRootClaim = Claim.wrap(keccak256("wrong-root-claim"));
        Claim expectedRootClaim = computeRootClaim(keccak256("block"), keccak256("state"));

        vm.startPrank(proposer, proposer);
        vm.expectRevert(
            abi.encodeWithSelector(
                TeeDisputeGame.RootClaimMismatch.selector, expectedRootClaim.raw(), wrongRootClaim.raw()
            )
        );
        factory.create{ value: DEFENDER_BOND }(GameType.wrap(TEE_DISPUTE_GAME_TYPE), wrongRootClaim, extraData);
        vm.stopPrank();
    }

    function test_initialize_revertWhenL2SequenceNumberDoesNotAdvance() public {
        vm.expectRevert(
            abi.encodeWithSelector(
                UnexpectedRootClaim.selector, computeRootClaim(keccak256("block"), keccak256("state"))
            )
        );
        _createGame(proposer, ANCHOR_L2_BLOCK, type(uint32).max, keccak256("block"), keccak256("state"));
    }

    function test_initialize_revertInvalidParentGame() public {
        MockStatusDisputeGame parent = new MockStatusDisputeGame(
            proposer,
            GameType.wrap(TEE_DISPUTE_GAME_TYPE),
            computeRootClaim(keccak256("parent-block"), keccak256("parent-state")),
            ANCHOR_L2_BLOCK + 3,
            bytes("parent"),
            GameStatus.CHALLENGER_WINS,
            uint64(block.timestamp),
            uint64(block.timestamp),
            true,
            IAnchorStateRegistry(address(anchorStateRegistry))
        );
        factory.pushGame(
            GameType.wrap(TEE_DISPUTE_GAME_TYPE),
            uint64(block.timestamp),
            IDisputeGame(address(parent)),
            parent.rootClaim(),
            bytes("parent")
        );
        anchorStateRegistry.setGameFlags(IDisputeGame(address(parent)), true, true, false, false, false, true, false);

        vm.expectRevert(InvalidParentGame.selector);
        _createGame(proposer, ANCHOR_L2_BLOCK + 7, 0, keccak256("child-block"), keccak256("child-state"));
    }

    /// @notice initialize() should revert when parent's l2SequenceNumber <= anchor state
    function test_initialize_revertWhenParentBelowAnchor() public {
        // Anchor is at block 10. Create a parent at block 8 (below anchor).
        MockStatusDisputeGame staleParent = new MockStatusDisputeGame(
            proposer,
            GameType.wrap(TEE_DISPUTE_GAME_TYPE),
            computeRootClaim(keccak256("stale-block"), keccak256("stale-state")),
            ANCHOR_L2_BLOCK - 2, // l2SequenceNumber = 8, anchor = 10
            bytes("stale"),
            GameStatus.IN_PROGRESS,
            uint64(block.timestamp),
            0,
            true,
            IAnchorStateRegistry(address(anchorStateRegistry))
        );
        factory.pushGame(
            GameType.wrap(TEE_DISPUTE_GAME_TYPE),
            uint64(block.timestamp),
            IDisputeGame(address(staleParent)),
            staleParent.rootClaim(),
            bytes("stale")
        );
        anchorStateRegistry.setGameFlags(
            IDisputeGame(address(staleParent)), true, true, false, false, false, true, false
        );

        vm.expectRevert(InvalidParentGame.selector);
        _createGame(proposer, ANCHOR_L2_BLOCK + 5, 0, keccak256("child-block"), keccak256("child-state"));
    }

    function test_challenge_updatesState() public {
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, keccak256("end-block"), keccak256("end-state"));

        vm.prank(challenger);
        TeeDisputeGame.ProposalStatus proposalStatus = game.challenge{ value: CHALLENGER_BOND }();

        (, address counteredBy,,, TeeDisputeGame.ProposalStatus storedStatus,) = game.claimData();
        assertEq(counteredBy, challenger);
        assertEq(uint8(proposalStatus), uint8(TeeDisputeGame.ProposalStatus.Challenged));
        assertEq(uint8(storedStatus), uint8(TeeDisputeGame.ProposalStatus.Challenged));
        assertEq(game.refundModeCredit(challenger), CHALLENGER_BOND);
    }

    function test_challenge_revertIncorrectBond() public {
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, keccak256("end-block"), keccak256("end-state"));

        vm.prank(challenger);
        vm.expectRevert(IncorrectBondAmount.selector);
        game.challenge{ value: CHALLENGER_BOND - 1 }();
    }

    function test_challenge_revertWhenAlreadyChallenged() public {
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, keccak256("end-block"), keccak256("end-state"));

        vm.prank(challenger);
        game.challenge{ value: CHALLENGER_BOND }();

        vm.prank(challenger);
        vm.expectRevert(ClaimAlreadyChallenged.selector);
        game.challenge{ value: CHALLENGER_BOND }();
    }

    function test_prove_succeedsWithSingleBatch() public {
        bytes32 endBlockHash = keccak256("end-block");
        bytes32 endStateHash = keccak256("end-state");
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, endBlockHash, endStateHash);

        teeProofVerifier.setRegistered(executor, true);
        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](1);
        proofs[0] = buildBatchProof(
            BatchInput({
                startBlockHash: ANCHOR_BLOCK_HASH,
                startStateHash: ANCHOR_STATE_HASH,
                endBlockHash: endBlockHash,
                endStateHash: endStateHash,
                l2Block: game.l2SequenceNumber()
            }),
            DEFAULT_EXECUTOR_KEY,
            game.domainSeparator()
        );

        vm.prank(proposer);
        TeeDisputeGame.ProposalStatus status = game.prove(abi.encode(proofs));
        (,, address prover,, TeeDisputeGame.ProposalStatus storedStatus,) = game.claimData();
        assertEq(prover, proposer);
        assertEq(uint8(status), uint8(TeeDisputeGame.ProposalStatus.UnchallengedAndValidProofProvided));
        assertEq(uint8(storedStatus), uint8(TeeDisputeGame.ProposalStatus.UnchallengedAndValidProofProvided));
    }

    function test_prove_succeedsWithChainedBatches() public {
        bytes32 middleBlockHash = keccak256("middle-block");
        bytes32 middleStateHash = keccak256("middle-state");
        bytes32 endBlockHash = keccak256("end-block");
        bytes32 endStateHash = keccak256("end-state");
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 8, type(uint32).max, endBlockHash, endStateHash);

        teeProofVerifier.setRegistered(executor, true);

        bytes32 domainSep = game.domainSeparator();
        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](2);
        proofs[0] = buildBatchProof(
            BatchInput({
                startBlockHash: ANCHOR_BLOCK_HASH,
                startStateHash: ANCHOR_STATE_HASH,
                endBlockHash: middleBlockHash,
                endStateHash: middleStateHash,
                l2Block: ANCHOR_L2_BLOCK + 4
            }),
            DEFAULT_EXECUTOR_KEY,
            domainSep
        );
        proofs[1] = buildBatchProof(
            BatchInput({
                startBlockHash: middleBlockHash,
                startStateHash: middleStateHash,
                endBlockHash: endBlockHash,
                endStateHash: endStateHash,
                l2Block: game.l2SequenceNumber()
            }),
            DEFAULT_EXECUTOR_KEY,
            domainSep
        );

        vm.prank(proposer);
        TeeDisputeGame.ProposalStatus status = game.prove(abi.encode(proofs));
        assertEq(uint8(status), uint8(TeeDisputeGame.ProposalStatus.UnchallengedAndValidProofProvided));
    }

    function test_prove_revertEmptyBatchProofs() public {
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, keccak256("end-block"), keccak256("end-state"));

        vm.prank(proposer);
        vm.expectRevert(TeeDisputeGame.EmptyBatchProofs.selector);
        game.prove(abi.encode(new TeeDisputeGame.BatchProof[](0)));
    }

    function test_prove_revertStartHashMismatch() public {
        bytes32 endBlockHash = keccak256("end-block");
        bytes32 endStateHash = keccak256("end-state");
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, endBlockHash, endStateHash);

        teeProofVerifier.setRegistered(executor, true);
        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](1);
        proofs[0] = buildBatchProof(
            BatchInput({
                startBlockHash: keccak256("wrong-start-block"),
                startStateHash: ANCHOR_STATE_HASH,
                endBlockHash: endBlockHash,
                endStateHash: endStateHash,
                l2Block: game.l2SequenceNumber()
            }),
            DEFAULT_EXECUTOR_KEY,
            game.domainSeparator()
        );

        vm.prank(proposer);
        vm.expectRevert(
            abi.encodeWithSelector(
                TeeDisputeGame.StartHashMismatch.selector,
                computeRootClaim(ANCHOR_BLOCK_HASH, ANCHOR_STATE_HASH).raw(),
                keccak256(abi.encode(keccak256("wrong-start-block"), ANCHOR_STATE_HASH))
            )
        );
        game.prove(abi.encode(proofs));
    }

    function test_prove_revertBatchChainBreak() public {
        bytes32 endBlockHash = keccak256("end-block");
        bytes32 endStateHash = keccak256("end-state");
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 8, type(uint32).max, endBlockHash, endStateHash);

        teeProofVerifier.setRegistered(executor, true);
        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](2);
        proofs[0] = buildBatchProof(
            BatchInput({
                startBlockHash: ANCHOR_BLOCK_HASH,
                startStateHash: ANCHOR_STATE_HASH,
                endBlockHash: keccak256("middle-block"),
                endStateHash: keccak256("middle-state"),
                l2Block: ANCHOR_L2_BLOCK + 4
            }),
            DEFAULT_EXECUTOR_KEY,
            game.domainSeparator()
        );
        proofs[1] = buildBatchProof(
            BatchInput({
                startBlockHash: keccak256("different-block"),
                startStateHash: keccak256("different-state"),
                endBlockHash: endBlockHash,
                endStateHash: endStateHash,
                l2Block: game.l2SequenceNumber()
            }),
            DEFAULT_EXECUTOR_KEY,
            game.domainSeparator()
        );

        vm.prank(proposer);
        vm.expectRevert(abi.encodeWithSelector(TeeDisputeGame.BatchChainBreak.selector, 1));
        game.prove(abi.encode(proofs));
    }

    function test_prove_revertBatchBlockNotIncreasing() public {
        bytes32 middleBlockHash = keccak256("middle-block");
        bytes32 middleStateHash = keccak256("middle-state");
        bytes32 endBlockHash = keccak256("end-block");
        bytes32 endStateHash = keccak256("end-state");
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 8, type(uint32).max, endBlockHash, endStateHash);

        teeProofVerifier.setRegistered(executor, true);
        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](2);
        proofs[0] = buildBatchProof(
            BatchInput({
                startBlockHash: ANCHOR_BLOCK_HASH,
                startStateHash: ANCHOR_STATE_HASH,
                endBlockHash: middleBlockHash,
                endStateHash: middleStateHash,
                l2Block: ANCHOR_L2_BLOCK + 4
            }),
            DEFAULT_EXECUTOR_KEY,
            game.domainSeparator()
        );
        proofs[1] = buildBatchProof(
            BatchInput({
                startBlockHash: middleBlockHash,
                startStateHash: middleStateHash,
                endBlockHash: endBlockHash,
                endStateHash: endStateHash,
                l2Block: ANCHOR_L2_BLOCK + 4
            }),
            DEFAULT_EXECUTOR_KEY,
            game.domainSeparator()
        );

        vm.prank(proposer);
        vm.expectRevert(
            abi.encodeWithSelector(
                TeeDisputeGame.BatchBlockNotIncreasing.selector, 1, ANCHOR_L2_BLOCK + 4, ANCHOR_L2_BLOCK + 4
            )
        );
        game.prove(abi.encode(proofs));
    }

    function test_prove_revertFinalHashMismatch() public {
        bytes32 endBlockHash = keccak256("end-block");
        bytes32 endStateHash = keccak256("end-state");
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, endBlockHash, endStateHash);

        teeProofVerifier.setRegistered(executor, true);
        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](1);
        proofs[0] = buildBatchProof(
            BatchInput({
                startBlockHash: ANCHOR_BLOCK_HASH,
                startStateHash: ANCHOR_STATE_HASH,
                endBlockHash: keccak256("wrong-end-block"),
                endStateHash: endStateHash,
                l2Block: game.l2SequenceNumber()
            }),
            DEFAULT_EXECUTOR_KEY,
            game.domainSeparator()
        );

        vm.prank(proposer);
        vm.expectRevert(
            abi.encodeWithSelector(
                TeeDisputeGame.FinalHashMismatch.selector,
                computeRootClaim(endBlockHash, endStateHash).raw(),
                keccak256(abi.encode(keccak256("wrong-end-block"), endStateHash))
            )
        );
        game.prove(abi.encode(proofs));
    }

    function test_prove_revertFinalBlockMismatch() public {
        bytes32 endBlockHash = keccak256("end-block");
        bytes32 endStateHash = keccak256("end-state");
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, endBlockHash, endStateHash);

        teeProofVerifier.setRegistered(executor, true);
        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](1);
        proofs[0] = buildBatchProof(
            BatchInput({
                startBlockHash: ANCHOR_BLOCK_HASH,
                startStateHash: ANCHOR_STATE_HASH,
                endBlockHash: endBlockHash,
                endStateHash: endStateHash,
                l2Block: game.l2SequenceNumber() - 1
            }),
            DEFAULT_EXECUTOR_KEY,
            game.domainSeparator()
        );

        vm.expectRevert(
            abi.encodeWithSelector(
                TeeDisputeGame.FinalBlockMismatch.selector, game.l2SequenceNumber(), game.l2SequenceNumber() - 1
            )
        );
        vm.prank(proposer);
        game.prove(abi.encode(proofs));
    }

    function test_prove_revertWhenVerifierRejectsSignature() public {
        bytes32 endBlockHash = keccak256("end-block");
        bytes32 endStateHash = keccak256("end-state");
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, endBlockHash, endStateHash);

        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](1);
        proofs[0] = buildBatchProof(
            BatchInput({
                startBlockHash: ANCHOR_BLOCK_HASH,
                startStateHash: ANCHOR_STATE_HASH,
                endBlockHash: endBlockHash,
                endStateHash: endStateHash,
                l2Block: game.l2SequenceNumber()
            }),
            DEFAULT_EXECUTOR_KEY,
            game.domainSeparator()
        );

        vm.prank(proposer);
        vm.expectRevert(MockTeeProofVerifier.EnclaveNotRegistered.selector);
        game.prove(abi.encode(proofs));
    }

    /// @notice prove() should revert when parent game resolved as CHALLENGER_WINS
    function test_prove_revertWhenParentChallengerWins() public {
        // Create parent game (IN_PROGRESS initially)
        bytes32 parentBlockHash = keccak256("parent-block");
        bytes32 parentStateHash = keccak256("parent-state");
        MockStatusDisputeGame parent = new MockStatusDisputeGame(
            proposer,
            GameType.wrap(TEE_DISPUTE_GAME_TYPE),
            computeRootClaim(parentBlockHash, parentStateHash),
            ANCHOR_L2_BLOCK + 3,
            bytes("parent"),
            GameStatus.IN_PROGRESS,
            uint64(block.timestamp),
            0,
            true,
            IAnchorStateRegistry(address(anchorStateRegistry))
        );
        factory.pushGame(
            GameType.wrap(TEE_DISPUTE_GAME_TYPE),
            uint64(block.timestamp),
            IDisputeGame(address(parent)),
            parent.rootClaim(),
            bytes("parent")
        );
        anchorStateRegistry.setGameFlags(IDisputeGame(address(parent)), true, true, false, false, false, true, false);

        // Create child game referencing parent
        bytes32 childBlockHash = keccak256("child-block");
        bytes32 childStateHash = keccak256("child-state");
        (TeeDisputeGame child,,) = _createGame(proposer, ANCHOR_L2_BLOCK + 7, 0, childBlockHash, childStateHash);

        // Parent resolves as CHALLENGER_WINS
        parent.setStatus(GameStatus.CHALLENGER_WINS);

        // Build valid batch proof for child
        teeProofVerifier.setRegistered(executor, true);
        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](1);
        proofs[0] = buildBatchProof(
            BatchInput({
                startBlockHash: parentBlockHash,
                startStateHash: parentStateHash,
                endBlockHash: childBlockHash,
                endStateHash: childStateHash,
                l2Block: child.l2SequenceNumber()
            }),
            DEFAULT_EXECUTOR_KEY,
            child.domainSeparator()
        );

        // prove() should revert because parent is CHALLENGER_WINS
        vm.prank(proposer);
        vm.expectRevert(InvalidParentGame.selector);
        child.prove(abi.encode(proofs));
    }

    function test_resolve_revertWhenParentInProgress() public {
        MockStatusDisputeGame parent = new MockStatusDisputeGame(
            proposer,
            GameType.wrap(TEE_DISPUTE_GAME_TYPE),
            computeRootClaim(keccak256("parent-block"), keccak256("parent-state")),
            ANCHOR_L2_BLOCK + 3,
            bytes("parent"),
            GameStatus.IN_PROGRESS,
            uint64(block.timestamp),
            0,
            true,
            IAnchorStateRegistry(address(anchorStateRegistry))
        );
        factory.pushGame(
            GameType.wrap(TEE_DISPUTE_GAME_TYPE),
            uint64(block.timestamp),
            IDisputeGame(address(parent)),
            parent.rootClaim(),
            bytes("parent")
        );
        anchorStateRegistry.setGameFlags(IDisputeGame(address(parent)), true, true, false, false, false, true, false);

        (TeeDisputeGame child,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 7, 0, keccak256("child-block"), keccak256("child-state"));

        // Wait for child's challenge window to expire so gameOver() passes
        vm.warp(block.timestamp + MAX_CHALLENGE_DURATION + 1);

        // Child still cannot resolve because parent is IN_PROGRESS
        vm.expectRevert(ParentGameNotResolved.selector);
        child.resolve();
    }

    /// @notice When parent resolves as CHALLENGER_WINS, child short-circuits to CHALLENGER_WINS.
    /// @dev Realistic timing: child is created while parent is IN_PROGRESS (required by initialize),
    ///      then parent later resolves as CHALLENGER_WINS, then child resolve short-circuits.
    function test_resolve_parentChallengerWinsShortCircuits() public {
        MockStatusDisputeGame parent = new MockStatusDisputeGame(
            proposer,
            GameType.wrap(TEE_DISPUTE_GAME_TYPE),
            computeRootClaim(keccak256("parent-block"), keccak256("parent-state")),
            ANCHOR_L2_BLOCK + 3,
            bytes("parent"),
            GameStatus.IN_PROGRESS,
            uint64(block.timestamp),
            0,
            true,
            IAnchorStateRegistry(address(anchorStateRegistry))
        );
        factory.pushGame(
            GameType.wrap(TEE_DISPUTE_GAME_TYPE),
            uint64(block.timestamp),
            IDisputeGame(address(parent)),
            parent.rootClaim(),
            bytes("parent")
        );
        anchorStateRegistry.setGameFlags(IDisputeGame(address(parent)), true, true, false, false, false, true, false);

        // Child created while parent is still IN_PROGRESS
        (TeeDisputeGame child,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 7, 0, keccak256("child-block"), keccak256("child-state"));

        // Challenger challenges the child
        vm.prank(challenger);
        child.challenge{ value: CHALLENGER_BOND }();

        // Time passes: parent is challenged and times out → CHALLENGER_WINS
        vm.warp(block.timestamp + MAX_PROVE_DURATION + 1);
        parent.setStatus(GameStatus.CHALLENGER_WINS);
        parent.setResolvedAt(uint64(block.timestamp));

        // Child resolve short-circuits because parent lost
        GameStatus status = child.resolve();
        assertEq(uint8(status), uint8(GameStatus.CHALLENGER_WINS));
        assertEq(child.normalModeCredit(challenger), DEFENDER_BOND + CHALLENGER_BOND);
    }

    function test_prove_revertUnauthorizedProver() public {
        bytes32 endBlockHash = keccak256("end-block");
        bytes32 endStateHash = keccak256("end-state");
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, endBlockHash, endStateHash);

        teeProofVerifier.setRegistered(executor, true);
        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](1);
        proofs[0] = buildBatchProof(
            BatchInput({
                startBlockHash: ANCHOR_BLOCK_HASH,
                startStateHash: ANCHOR_STATE_HASH,
                endBlockHash: endBlockHash,
                endStateHash: endStateHash,
                l2Block: game.l2SequenceNumber()
            }),
            DEFAULT_EXECUTOR_KEY,
            game.domainSeparator()
        );

        address unauthorized = makeAddr("unauthorized");
        vm.prank(unauthorized);
        vm.expectRevert(BadAuth.selector);
        game.prove(abi.encode(proofs));
    }

    /// @notice CHALLENGER_WINS in NORMAL mode: challenger takes all bonds, proposer gets nothing.
    /// @dev A CHALLENGER_WINS game is still "proper" in real ASR (registered, not blacklisted,
    ///      not retired, not paused), so closeGame → NORMAL mode → normalModeCredit only.
    function test_claimCredit_challengerWinsNormalMode() public {
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, keccak256("end-block"), keccak256("end-state"));

        vm.prank(challenger);
        game.challenge{ value: CHALLENGER_BOND }();

        // Timeout without proof → CHALLENGER_WINS
        vm.warp(block.timestamp + MAX_PROVE_DURATION + 1);
        assertEq(uint8(game.resolve()), uint8(GameStatus.CHALLENGER_WINS));

        // CHALLENGER_WINS game is still proper → NORMAL mode
        // (registered, respected, not blacklisted, not retired, finalized)
        anchorStateRegistry.setGameFlags(game, true, true, false, false, true, true, false);

        // Challenger takes all bonds
        uint256 challengerBalanceBefore = challenger.balance;
        game.claimCredit(challenger);

        assertEq(uint8(game.bondDistributionMode()), uint8(BondDistributionMode.NORMAL));
        assertEq(challenger.balance, challengerBalanceBefore + DEFENDER_BOND + CHALLENGER_BOND);
        // Proposer has zero credit — lost their bond
        assertEq(game.normalModeCredit(proposer), 0);
    }

    /// @notice REFUND mode: only triggered by guardian blacklisting (not by CHALLENGER_WINS).
    /// @dev In real ASR, isGameProper returns false only when the game is blacklisted, retired,
    ///      or the system is paused. Here we simulate a blacklisted DEFENDER_WINS game.
    function test_claimCredit_refundModeWhenBlacklisted() public {
        bytes32 endBlockHash = keccak256("end-block");
        bytes32 endStateHash = keccak256("end-state");
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, endBlockHash, endStateHash);

        vm.prank(challenger);
        game.challenge{ value: CHALLENGER_BOND }();

        // Proposer proves — game would normally be DEFENDER_WINS
        teeProofVerifier.setRegistered(executor, true);
        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](1);
        proofs[0] = buildBatchProof(
            BatchInput({
                startBlockHash: ANCHOR_BLOCK_HASH,
                startStateHash: ANCHOR_STATE_HASH,
                endBlockHash: endBlockHash,
                endStateHash: endStateHash,
                l2Block: game.l2SequenceNumber()
            }),
            DEFAULT_EXECUTOR_KEY,
            game.domainSeparator()
        );
        vm.prank(proposer);
        game.prove(abi.encode(proofs));

        assertEq(uint8(game.resolve()), uint8(GameStatus.DEFENDER_WINS));

        // Guardian blacklists the game (e.g. discovered exploit)
        // → isGameProper returns false → REFUND mode
        anchorStateRegistry.setGameFlags(game, true, true, true, false, true, false, false);

        uint256 proposerBalanceBefore = proposer.balance;
        uint256 challengerBalanceBefore = challenger.balance;
        game.claimCredit(proposer);
        game.claimCredit(challenger);

        assertEq(uint8(game.bondDistributionMode()), uint8(BondDistributionMode.REFUND));
        assertEq(proposer.balance, proposerBalanceBefore + DEFENDER_BOND);
        assertEq(challenger.balance, challengerBalanceBefore + CHALLENGER_BOND);
    }

    function test_closeGame_revertWhenNotFinalized() public {
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, keccak256("end-block"), keccak256("end-state"));
        vm.warp(block.timestamp + MAX_CHALLENGE_DURATION + 1);
        game.resolve();

        vm.expectRevert(GameNotFinalized.selector);
        game.closeGame();
    }

    function test_resolve_revertWhenGameNotOver() public {
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, keccak256("end-block"), keccak256("end-state"));

        vm.expectRevert(GameNotOver.selector);
        game.resolve();
    }

    ////////////////////////////////////////////////////////////////
    //     Audit: Boundary & Invariant Tests                      //
    ////////////////////////////////////////////////////////////////

    /// @notice INV-6: prove() should revert after resolve (claimData immutable)
    function test_prove_revertAfterResolve() public {
        bytes32 endBlockHash = keccak256("end-block");
        bytes32 endStateHash = keccak256("end-state");
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, endBlockHash, endStateHash);

        // Wait for challenge deadline to expire, then resolve
        vm.warp(block.timestamp + MAX_CHALLENGE_DURATION + 1);
        game.resolve();

        // prove after resolve should revert
        teeProofVerifier.setRegistered(executor, true);
        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](1);
        proofs[0] = buildBatchProof(
            BatchInput({
                startBlockHash: ANCHOR_BLOCK_HASH,
                startStateHash: ANCHOR_STATE_HASH,
                endBlockHash: endBlockHash,
                endStateHash: endStateHash,
                l2Block: game.l2SequenceNumber()
            }),
            DEFAULT_EXECUTOR_KEY,
            game.domainSeparator()
        );

        vm.prank(proposer);
        vm.expectRevert(ClaimAlreadyResolved.selector);
        game.prove(abi.encode(proofs));
    }

    /// @notice INV-6: challenge() should revert after resolve (claimData immutable)
    function test_challenge_revertAfterResolve() public {
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, keccak256("end-block"), keccak256("end-state"));

        vm.warp(block.timestamp + MAX_CHALLENGE_DURATION + 1);
        game.resolve();

        vm.prank(challenger);
        vm.expectRevert(ClaimAlreadyChallenged.selector);
        game.challenge{ value: CHALLENGER_BOND }();
    }

    /// @notice Double prove: second prove() should revert with GameOver
    function test_prove_revertWhenAlreadyProved() public {
        bytes32 endBlockHash = keccak256("end-block");
        bytes32 endStateHash = keccak256("end-state");
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, endBlockHash, endStateHash);

        teeProofVerifier.setRegistered(executor, true);
        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](1);
        proofs[0] = buildBatchProof(
            BatchInput({
                startBlockHash: ANCHOR_BLOCK_HASH,
                startStateHash: ANCHOR_STATE_HASH,
                endBlockHash: endBlockHash,
                endStateHash: endStateHash,
                l2Block: game.l2SequenceNumber()
            }),
            DEFAULT_EXECUTOR_KEY,
            game.domainSeparator()
        );

        vm.prank(proposer);
        game.prove(abi.encode(proofs));

        // Second prove should revert (gameOver = true because prover != address(0))
        vm.prank(proposer);
        vm.expectRevert(GameOver.selector);
        game.prove(abi.encode(proofs));
    }

    /// @notice challenge should revert after prove (gameOver blocks further challenges)
    function test_challenge_revertAfterProve() public {
        bytes32 endBlockHash = keccak256("end-block");
        bytes32 endStateHash = keccak256("end-state");
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, endBlockHash, endStateHash);

        teeProofVerifier.setRegistered(executor, true);
        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](1);
        proofs[0] = buildBatchProof(
            BatchInput({
                startBlockHash: ANCHOR_BLOCK_HASH,
                startStateHash: ANCHOR_STATE_HASH,
                endBlockHash: endBlockHash,
                endStateHash: endStateHash,
                l2Block: game.l2SequenceNumber()
            }),
            DEFAULT_EXECUTOR_KEY,
            game.domainSeparator()
        );

        vm.prank(proposer);
        game.prove(abi.encode(proofs));

        // After prove, status is UnchallengedAndValidProofProvided.
        // challenge() checks status != Unchallenged first → revert ClaimAlreadyChallenged
        vm.prank(challenger);
        vm.expectRevert(ClaimAlreadyChallenged.selector);
        game.challenge{ value: CHALLENGER_BOND }();
    }

    /// @notice challenge should revert after deadline expires
    function test_challenge_revertAfterDeadline() public {
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, keccak256("end-block"), keccak256("end-state"));

        vm.warp(block.timestamp + MAX_CHALLENGE_DURATION + 1);

        vm.prank(challenger);
        vm.expectRevert(GameOver.selector);
        game.challenge{ value: CHALLENGER_BOND }();
    }

    /// @notice Double resolve should revert
    function test_resolve_revertWhenAlreadyResolved() public {
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, keccak256("end-block"), keccak256("end-state"));

        vm.warp(block.timestamp + MAX_CHALLENGE_DURATION + 1);
        game.resolve();

        vm.expectRevert(ClaimAlreadyResolved.selector);
        game.resolve();
    }

    /// @notice INV-6: claimData.prover and claimData.counteredBy cannot change after resolve
    function test_invariant6_claimDataImmutableAfterResolve() public {
        bytes32 endBlockHash = keccak256("end-block");
        bytes32 endStateHash = keccak256("end-state");
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, endBlockHash, endStateHash);

        // Challenge
        vm.prank(challenger);
        game.challenge{ value: CHALLENGER_BOND }();

        // Prove
        teeProofVerifier.setRegistered(executor, true);
        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](1);
        proofs[0] = buildBatchProof(
            BatchInput({
                startBlockHash: ANCHOR_BLOCK_HASH,
                startStateHash: ANCHOR_STATE_HASH,
                endBlockHash: endBlockHash,
                endStateHash: endStateHash,
                l2Block: game.l2SequenceNumber()
            }),
            DEFAULT_EXECUTOR_KEY,
            game.domainSeparator()
        );
        vm.prank(proposer);
        game.prove(abi.encode(proofs));

        // resolve
        game.resolve();

        // Snapshot claimData after resolve
        (, address counteredBy, address prover,, TeeDisputeGame.ProposalStatus postStatus,) = game.claimData();
        assertEq(counteredBy, challenger);
        assertEq(prover, proposer);
        assertEq(uint8(postStatus), uint8(TeeDisputeGame.ProposalStatus.Resolved));

        // Confirm prove / challenge cannot modify claimData
        vm.prank(proposer);
        vm.expectRevert(ClaimAlreadyResolved.selector);
        game.prove(abi.encode(proofs));

        vm.prank(challenger);
        vm.expectRevert(ClaimAlreadyChallenged.selector);
        game.challenge{ value: CHALLENGER_BOND }();

        // claimData remains unchanged
        (, address counteredBy2, address prover2,, TeeDisputeGame.ProposalStatus postStatus2,) = game.claimData();
        assertEq(counteredBy2, challenger);
        assertEq(prover2, proposer);
        assertEq(uint8(postStatus2), uint8(TeeDisputeGame.ProposalStatus.Resolved));
    }

    /// @notice INV-1: contract balance >= active mode credit sum after resolve
    /// @dev Both normalModeCredit and refundModeCredit coexist in storage, but claimCredit
    ///      only reads one mode. Correct invariant: balance >= max(sum(normal), sum(refund)).
    function test_invariant1_balanceCoversCredits_defenderWins() public {
        bytes32 endBlockHash = keccak256("end-block");
        bytes32 endStateHash = keccak256("end-state");
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, endBlockHash, endStateHash);

        vm.prank(challenger);
        game.challenge{ value: CHALLENGER_BOND }();

        teeProofVerifier.setRegistered(executor, true);
        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](1);
        proofs[0] = buildBatchProof(
            BatchInput({
                startBlockHash: ANCHOR_BLOCK_HASH,
                startStateHash: ANCHOR_STATE_HASH,
                endBlockHash: endBlockHash,
                endStateHash: endStateHash,
                l2Block: game.l2SequenceNumber()
            }),
            DEFAULT_EXECUTOR_KEY,
            game.domainSeparator()
        );
        vm.prank(proposer);
        game.prove(abi.encode(proofs));
        game.resolve();

        // INV-1: balance ≥ max(sum(normalModeCredit), sum(refundModeCredit))
        uint256 totalNormal = game.normalModeCredit(proposer) + game.normalModeCredit(challenger);
        uint256 totalRefund = game.refundModeCredit(proposer) + game.refundModeCredit(challenger);
        assertGe(address(game).balance, totalNormal, "INV-1: balance < sum(normalModeCredit)");
        assertGe(address(game).balance, totalRefund, "INV-1: balance < sum(refundModeCredit)");

        // INV-12: In NORMAL mode, exactly one address has normalModeCredit (proposer wins)
        assertGt(game.normalModeCredit(proposer), 0);
        assertEq(game.normalModeCredit(challenger), 0);
    }

    /// @notice INV-1 + INV-12: balance covers credit when CHALLENGER_WINS
    function test_invariant1_balanceCoversCredits_challengerWins() public {
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, keccak256("end-block"), keccak256("end-state"));

        vm.prank(challenger);
        game.challenge{ value: CHALLENGER_BOND }();

        vm.warp(block.timestamp + MAX_PROVE_DURATION + 1);
        game.resolve();

        // INV-1: balance ≥ max(sum(normalModeCredit), sum(refundModeCredit))
        uint256 totalNormal = game.normalModeCredit(proposer) + game.normalModeCredit(challenger);
        uint256 totalRefund = game.refundModeCredit(proposer) + game.refundModeCredit(challenger);
        assertGe(address(game).balance, totalNormal, "INV-1: balance < sum(normalModeCredit)");
        assertGe(address(game).balance, totalRefund, "INV-1: balance < sum(refundModeCredit)");

        // INV-12: when challenger wins, only challenger has credit
        assertEq(game.normalModeCredit(proposer), 0);
        assertGt(game.normalModeCredit(challenger), 0);
    }

    /// @notice INV-5: GameStatus is irreversible — status unchanged after resolve
    function test_invariant5_gameStatusIrreversible() public {
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, keccak256("end-block"), keccak256("end-state"));

        vm.warp(block.timestamp + MAX_CHALLENGE_DURATION + 1);
        game.resolve();

        GameStatus statusAfterResolve = game.status();
        assertEq(uint8(statusAfterResolve), uint8(GameStatus.DEFENDER_WINS));

        // Second resolve should revert
        vm.expectRevert(ClaimAlreadyResolved.selector);
        game.resolve();

        // Status unchanged
        assertEq(uint8(game.status()), uint8(statusAfterResolve));
    }

    /// @notice INV-13: bondDistributionMode is irreversible once set
    function test_invariant13_bondDistributionModeIrreversible() public {
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, keccak256("end-block"), keccak256("end-state"));

        vm.warp(block.timestamp + MAX_CHALLENGE_DURATION + 1);
        game.resolve();

        // Set ASR flags so closeGame can execute
        anchorStateRegistry.setGameFlags(game, true, true, false, false, true, true, false);
        game.closeGame();

        BondDistributionMode modeAfterClose = game.bondDistributionMode();
        assertEq(uint8(modeAfterClose), uint8(BondDistributionMode.NORMAL));

        // closeGame is idempotent, mode unchanged
        game.closeGame();
        assertEq(uint8(game.bondDistributionMode()), uint8(modeAfterClose));
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
                address(
                    factory.create{ value: DEFENDER_BOND }(GameType.wrap(TEE_DISPUTE_GAME_TYPE), rootClaim, extraData)
                )
            )
        );
        vm.stopPrank();
    }
}
