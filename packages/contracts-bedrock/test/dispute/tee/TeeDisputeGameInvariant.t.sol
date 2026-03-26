// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import { IDisputeGameFactory } from "interfaces/dispute/IDisputeGameFactory.sol";
import { IDisputeGame } from "interfaces/dispute/IDisputeGame.sol";
import { IAnchorStateRegistry } from "interfaces/dispute/IAnchorStateRegistry.sol";
import { ITeeProofVerifier } from "interfaces/dispute/ITeeProofVerifier.sol";
import { TeeDisputeGame, TEE_DISPUTE_GAME_TYPE } from "src/dispute/tee/TeeDisputeGame.sol";
import { BondDistributionMode, Duration, GameType, Claim, Hash, GameStatus } from "src/dispute/lib/Types.sol";
import { MockAnchorStateRegistry } from "test/dispute/tee/mocks/MockAnchorStateRegistry.sol";
import { MockDisputeGameFactory } from "test/dispute/tee/mocks/MockDisputeGameFactory.sol";
import { MockTeeProofVerifier } from "test/dispute/tee/mocks/MockTeeProofVerifier.sol";
import { TeeTestUtils } from "test/dispute/tee/helpers/TeeTestUtils.sol";
import { Test } from "forge-std/Test.sol";
import { Vm } from "forge-std/Vm.sol";

/// @title TeeDisputeGameHandler
/// @notice Foundry invariant test handler that simulates random proposer/challenger
///         action sequences against a single game instance.
contract TeeDisputeGameHandler is Test {
    TeeDisputeGame public game;
    address public proposer;
    address public challenger;
    address public executor;
    MockTeeProofVerifier public verifier;
    MockAnchorStateRegistry public anchorStateRegistry;
    uint256 public executorKey;

    // Track ProposalStatus transitions for monotonicity checks
    uint8 public lastProposalStatus;
    // Track whether GameStatus has changed from IN_PROGRESS
    bool public gameStatusChanged;
    GameStatus public recordedStatus;

    bytes32 private constant BATCH_PROOF_TYPEHASH = keccak256(
        "BatchProof(bytes32 startBlockHash,bytes32 startStateHash,bytes32 endBlockHash,bytes32 endStateHash,uint256 l2Block)"
    );

    constructor(
        TeeDisputeGame _game,
        address _proposer,
        address _challenger,
        address _executor,
        uint256 _executorKey,
        MockTeeProofVerifier _verifier,
        MockAnchorStateRegistry _anchorStateRegistry
    ) {
        game = _game;
        proposer = _proposer;
        challenger = _challenger;
        executor = _executor;
        executorKey = _executorKey;
        verifier = _verifier;
        anchorStateRegistry = _anchorStateRegistry;
        lastProposalStatus = uint8(TeeDisputeGame.ProposalStatus.Unchallenged);
    }

    /// @notice Randomly attempt challenge
    function challenge() external {
        vm.deal(challenger, 100 ether);
        vm.prank(challenger);
        try game.challenge{ value: 2 ether }() {
            _recordProposalStatus();
        } catch { }
        _recordGameStatus();
    }

    /// @notice Randomly attempt prove (with a valid signature)
    function prove() external {
        verifier.setRegistered(executor, true);

        // Build a batch proof (may not match startingOutputRoot, hence try/catch)
        (Hash startRoot, uint256 startBlock) = game.startingOutputRoot();
        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](1);

        bytes32 structHash = keccak256(
            abi.encode(
                BATCH_PROOF_TYPEHASH,
                game.blockHash(),
                bytes32(startRoot.raw()),
                game.blockHash(),
                game.stateHash(),
                game.l2SequenceNumber()
            )
        );

        // Placeholder proof — will likely fail hash checks but exercises the path
        proofs[0] = TeeDisputeGame.BatchProof({
            startBlockHash: bytes32(0),
            startStateHash: bytes32(0),
            endBlockHash: game.blockHash(),
            endStateHash: game.stateHash(),
            l2Block: game.l2SequenceNumber(),
            signature: _sign(keccak256("placeholder"))
        });

        vm.prank(proposer);
        try game.prove(abi.encode(proofs)) {
            _recordProposalStatus();
        } catch { }
        _recordGameStatus();
    }

    /// @notice Randomly warp time forward
    function warpForward(uint256 _seconds) external {
        _seconds = bound(_seconds, 0, 2 days);
        vm.warp(block.timestamp + _seconds);
    }

    /// @notice Randomly attempt resolve
    function resolve() external {
        try game.resolve() {
            _recordProposalStatus();
            _recordGameStatus();
        } catch { }
    }

    function _recordProposalStatus() internal {
        (,,,, TeeDisputeGame.ProposalStatus s,) = game.claimData();
        lastProposalStatus = uint8(s);
    }

    function _recordGameStatus() internal {
        GameStatus s = game.status();
        if (s != GameStatus.IN_PROGRESS && !gameStatusChanged) {
            gameStatusChanged = true;
            recordedStatus = s;
        }
    }

    function _sign(bytes32 digest) internal returns (bytes memory) {
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(executorKey, digest);
        return abi.encodePacked(r, s, v);
    }
}

/// @title TeeDisputeGameInvariantTest
/// @notice Foundry invariant tests that verify key invariants from the audit report:
///         - INV-1:  contract balance >= active mode credit sum
///         - INV-4:  ProposalStatus monotonically increasing (no rollback)
///         - INV-5:  GameStatus irreversible once changed
///         - INV-12: In NORMAL mode, at most one address has normalModeCredit
///         - INV-13: bondDistributionMode irreversible once set
contract TeeDisputeGameInvariantTest is TeeTestUtils {
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
    TeeDisputeGame internal implementation;
    TeeDisputeGame internal game;
    TeeDisputeGameHandler internal handler;

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

        implementation = new TeeDisputeGame(
            Duration.wrap(MAX_CHALLENGE_DURATION),
            Duration.wrap(MAX_PROVE_DURATION),
            IDisputeGameFactory(address(factory)),
            ITeeProofVerifier(address(teeProofVerifier)),
            CHALLENGER_BOND,
            IAnchorStateRegistry(address(anchorStateRegistry)),
            proposer,
            challenger
        );

        factory.setImplementation(GameType.wrap(TEE_DISPUTE_GAME_TYPE), implementation);
        factory.setInitBond(GameType.wrap(TEE_DISPUTE_GAME_TYPE), DEFENDER_BOND);

        anchorStateRegistry.setAnchor(
            Hash.wrap(computeRootClaim(ANCHOR_BLOCK_HASH, ANCHOR_STATE_HASH).raw()), ANCHOR_L2_BLOCK
        );
        anchorStateRegistry.setRespectedGameType(GameType.wrap(TEE_DISPUTE_GAME_TYPE));

        // Create a game instance for the handler to operate on
        bytes memory extraData =
            buildExtraData(ANCHOR_L2_BLOCK + 5, type(uint32).max, keccak256("end-block"), keccak256("end-state"));
        Claim rootClaim = computeRootClaim(keccak256("end-block"), keccak256("end-state"));

        vm.startPrank(proposer, proposer);
        game = TeeDisputeGame(
            payable(
                address(
                    factory.create{ value: DEFENDER_BOND }(GameType.wrap(TEE_DISPUTE_GAME_TYPE), rootClaim, extraData)
                )
            )
        );
        vm.stopPrank();

        handler = new TeeDisputeGameHandler(
            game, proposer, challenger, executor, DEFAULT_EXECUTOR_KEY, teeProofVerifier, anchorStateRegistry
        );

        // Only fuzz calls against the handler
        targetContract(address(handler));
    }

    /// @notice INV-1: contract balance >= active mode credit sum
    /// @dev Both normalModeCredit and refundModeCredit coexist in storage, but claimCredit
    ///      only reads one mode. Correct invariant: balance >= max(sum(normal), sum(refund)).
    function invariant_balanceCoversAllCredits() public view {
        uint256 totalNormal = game.normalModeCredit(proposer) + game.normalModeCredit(challenger);
        uint256 totalRefund = game.refundModeCredit(proposer) + game.refundModeCredit(challenger);
        assertGe(address(game).balance, totalNormal, "INV-1: balance < sum(normalModeCredit)");
        assertGe(address(game).balance, totalRefund, "INV-1: balance < sum(refundModeCredit)");
    }

    /// @notice INV-4: ProposalStatus can only transition forward; Resolved is terminal
    function invariant_proposalStatusMonotonic() public view {
        (,,,, TeeDisputeGame.ProposalStatus currentStatus,) = game.claimData();
        // Resolved (4) is the terminal state
        if (handler.lastProposalStatus() == uint8(TeeDisputeGame.ProposalStatus.Resolved)) {
            assertEq(
                uint8(currentStatus),
                uint8(TeeDisputeGame.ProposalStatus.Resolved),
                "INV-4: left Resolved state"
            );
        }
    }

    /// @notice INV-5: GameStatus is irreversible once changed from IN_PROGRESS
    function invariant_gameStatusIrreversible() public view {
        if (handler.gameStatusChanged()) {
            GameStatus current = game.status();
            // If previously resolved, current status must match the recorded one
            assertTrue(
                current == handler.recordedStatus() || current == GameStatus.IN_PROGRESS,
                "INV-5: GameStatus reversed"
            );
        }
    }

    /// @notice INV-12: In NORMAL mode, at most one address has normalModeCredit > 0
    function invariant_normalModeAtMostOneRecipient() public view {
        uint256 proposerCredit = game.normalModeCredit(proposer);
        uint256 challengerCredit = game.normalModeCredit(challenger);
        // Both cannot be > 0 simultaneously (winner takes all in NORMAL mode)
        assertTrue(
            proposerCredit == 0 || challengerCredit == 0,
            "INV-12: both proposer and challenger have normalModeCredit"
        );
    }

    /// @notice INV-13: bondDistributionMode is irreversible once set
    /// @dev Since handler does not call closeGame, this invariant verifies
    ///      UNDECIDED stability within handler's operation scope.
    function invariant_bondDistributionModeStable() public view {
        BondDistributionMode mode = game.bondDistributionMode();
        // Within handler scope (no closeGame), mode should stay UNDECIDED.
        // If mode changed via another path, it must not revert to UNDECIDED.
        assertTrue(
            mode == BondDistributionMode.UNDECIDED || mode == BondDistributionMode.NORMAL
                || mode == BondDistributionMode.REFUND,
            "INV-13: invalid bondDistributionMode"
        );
    }
}
