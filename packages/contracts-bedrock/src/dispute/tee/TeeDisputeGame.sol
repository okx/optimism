// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

// Libraries
import {Clone} from "@solady/utils/Clone.sol";
import {
    BondDistributionMode,
    Claim,
    Duration,
    GameStatus,
    GameType,
    Hash,
    Proposal,
    Timestamp
} from "src/dispute/lib/Types.sol";
import {
    AlreadyInitialized,
    BadAuth,
    BondTransferFailed,
    ClaimAlreadyResolved,
    GameNotFinalized,
    IncorrectBondAmount,
    InvalidBondDistributionMode,
    NoCreditToClaim,
    UnexpectedRootClaim
} from "src/dispute/lib/Errors.sol";
import "src/dispute/tee/lib/Errors.sol";

// Interfaces
import {ISemver} from "interfaces/universal/ISemver.sol";
import {IDisputeGameFactory} from "interfaces/dispute/IDisputeGameFactory.sol";
import {IDisputeGame} from "interfaces/dispute/IDisputeGame.sol";
import {IAnchorStateRegistry} from "interfaces/dispute/IAnchorStateRegistry.sol";
import {ITeeProofVerifier} from "interfaces/dispute/ITeeProofVerifier.sol";

// Contracts
import {AccessManager, TEE_DISPUTE_GAME_TYPE} from "src/dispute/tee/AccessManager.sol";

/// @title TeeDisputeGame
/// @notice A dispute game that uses TEE (AWS Nitro Enclave) ECDSA signatures
///         instead of SP1 ZK proofs for batch state transition verification.
/// @dev Mirrors OPSuccinctFaultDisputeGame architecture but replaces
///      SP1_VERIFIER.verifyProof() with TEE_PROOF_VERIFIER.verifyBatch().
///      Uses the same DisputeGameFactory, AnchorStateRegistry, and AccessManager
///      infrastructure from OP Stack.
///
///      prove() accepts multiple chained batch proofs to support the scenario
///      where different TEE executors handle different sub-ranges within a single game.
///      Each batch carries (startBlockHash, startStateHash, endBlockHash, endStateHash, l2Block).
///      batchDigest = keccak256(abi.encode(startBlockHash, startStateHash, endBlockHash, endStateHash, l2Block))
///      is computed on-chain and verified via TEE ECDSA signature.
///
///      rootClaim = keccak256(abi.encode(blockHash, stateHash)) where blockHash and stateHash
///      are passed via extraData. The anchor state stores this combined hash.
contract TeeDisputeGame is Clone, ISemver, IDisputeGame {
    ////////////////////////////////////////////////////////////////
    //                         Enums                              //
    ////////////////////////////////////////////////////////////////

    enum ProposalStatus {
        Unchallenged,
        Challenged,
        UnchallengedAndValidProofProvided,
        ChallengedAndValidProofProvided,
        Resolved
    }

    ////////////////////////////////////////////////////////////////
    //                         Structs                            //
    ////////////////////////////////////////////////////////////////

    struct ClaimData {
        uint32 parentIndex;
        address counteredBy;
        address prover;
        Claim claim;
        ProposalStatus status;
        Timestamp deadline;
    }

    /// @notice A single batch proof segment within a chained prove() call.
    /// @dev Multiple BatchProofs can be submitted to cover a game's full range,
    ///      e.g. when different TEE executors handle sub-ranges.
    struct BatchProof {
        bytes32 startBlockHash;
        bytes32 startStateHash;
        bytes32 endBlockHash;
        bytes32 endStateHash;
        uint256 l2Block;
        bytes signature;    // 65 bytes ECDSA (r + s + v)
    }

    ////////////////////////////////////////////////////////////////
    //                         Events                             //
    ////////////////////////////////////////////////////////////////

    event Challenged(address indexed challenger);
    event Proved(address indexed prover);
    event GameClosed(BondDistributionMode bondDistributionMode);

    error EmptyBatchProofs();
    error StartHashMismatch(bytes32 expectedCombined, bytes32 actualCombined);
    error BatchChainBreak(uint256 index);
    error BatchBlockNotIncreasing(uint256 index, uint256 prevBlock, uint256 curBlock);
    error FinalHashMismatch(bytes32 expectedCombined, bytes32 actualCombined);
    error FinalBlockMismatch(uint256 expectedBlock, uint256 actualBlock);
    error RootClaimMismatch(bytes32 expectedRootClaim, bytes32 actualRootClaim);

    ////////////////////////////////////////////////////////////////
    //                         Immutables                         //
    ////////////////////////////////////////////////////////////////

    Duration internal immutable MAX_CHALLENGE_DURATION;
    Duration internal immutable MAX_PROVE_DURATION;
    GameType internal immutable GAME_TYPE;
    IDisputeGameFactory internal immutable DISPUTE_GAME_FACTORY;
    ITeeProofVerifier internal immutable TEE_PROOF_VERIFIER;
    uint256 internal immutable CHALLENGER_BOND;
    IAnchorStateRegistry internal immutable ANCHOR_STATE_REGISTRY;
    AccessManager internal immutable ACCESS_MANAGER;

    ////////////////////////////////////////////////////////////////
    //                         State Vars                         //
    ////////////////////////////////////////////////////////////////

    /// @custom:semver 1.0.0
    string public constant version = "1.0.0";

    Timestamp public createdAt;
    Timestamp public resolvedAt;
    GameStatus public status;
    /// @notice The proposer EOA captured during initialization, aligned with OP permissioned games.
    address public proposer;
    bool internal initialized;
    ClaimData public claimData;

    mapping(address => uint256) public normalModeCredit;
    mapping(address => uint256) public refundModeCredit;

    Proposal public startingOutputRoot;
    bool public wasRespectedGameTypeWhenCreated;
    BondDistributionMode public bondDistributionMode;

    ////////////////////////////////////////////////////////////////
    //                       Constructor                          //
    ////////////////////////////////////////////////////////////////

    constructor(
        Duration _maxChallengeDuration,
        Duration _maxProveDuration,
        IDisputeGameFactory _disputeGameFactory,
        ITeeProofVerifier _teeProofVerifier,
        uint256 _challengerBond,
        IAnchorStateRegistry _anchorStateRegistry,
        AccessManager _accessManager
    ) {
        GAME_TYPE = GameType.wrap(TEE_DISPUTE_GAME_TYPE);
        MAX_CHALLENGE_DURATION = _maxChallengeDuration;
        MAX_PROVE_DURATION = _maxProveDuration;
        DISPUTE_GAME_FACTORY = _disputeGameFactory;
        TEE_PROOF_VERIFIER = _teeProofVerifier;
        CHALLENGER_BOND = _challengerBond;
        ANCHOR_STATE_REGISTRY = _anchorStateRegistry;
        ACCESS_MANAGER = _accessManager;
    }

    ////////////////////////////////////////////////////////////////
    //                       Initialize                           //
    ////////////////////////////////////////////////////////////////

    function initialize() external payable virtual {
        if (initialized) revert AlreadyInitialized();
        if (address(DISPUTE_GAME_FACTORY) != msg.sender) revert IncorrectDisputeGameFactory();
        if (!ACCESS_MANAGER.isAllowedProposer(tx.origin)) revert BadAuth();

        assembly {
            if iszero(eq(calldatasize(), 0xBE)) {
                mstore(0x00, 0x9824bdab)
                revert(0x1C, 0x04)
            }
        }

        // Verify rootClaim == keccak256(abi.encode(blockHash, stateHash))
        bytes32 expectedRootClaim = keccak256(abi.encode(blockHash(), stateHash()));
        if (expectedRootClaim != rootClaim().raw()) {
            revert RootClaimMismatch(expectedRootClaim, rootClaim().raw());
        }

        if (parentIndex() != type(uint32).max) {
            (GameType parentGameType,, IDisputeGame proxy) = DISPUTE_GAME_FACTORY.gameAtIndex(parentIndex());
            if (GameType.unwrap(parentGameType) != GameType.unwrap(GAME_TYPE)) revert InvalidParentGame();

            if (
                !ANCHOR_STATE_REGISTRY.isGameRespected(proxy) || ANCHOR_STATE_REGISTRY.isGameBlacklisted(proxy)
                    || ANCHOR_STATE_REGISTRY.isGameRetired(proxy)
            ) {
                revert InvalidParentGame();
            }

            startingOutputRoot = Proposal({
                l2SequenceNumber: TeeDisputeGame(address(proxy)).l2SequenceNumber(),
                root: Hash.wrap(TeeDisputeGame(address(proxy)).rootClaim().raw())
            });

            if (proxy.status() == GameStatus.CHALLENGER_WINS) revert InvalidParentGame();
        } else {
            (startingOutputRoot.root, startingOutputRoot.l2SequenceNumber) =
                IAnchorStateRegistry(ANCHOR_STATE_REGISTRY).anchors(GAME_TYPE);
        }

        if (l2SequenceNumber() <= startingOutputRoot.l2SequenceNumber) {
            revert UnexpectedRootClaim(rootClaim());
        }

        claimData = ClaimData({
            parentIndex: parentIndex(),
            counteredBy: address(0),
            prover: address(0),
            claim: rootClaim(),
            status: ProposalStatus.Unchallenged,
            deadline: Timestamp.wrap(uint64(block.timestamp + MAX_CHALLENGE_DURATION.raw()))
        });

        initialized = true;
        proposer = tx.origin;
        refundModeCredit[proposer] += msg.value;
        createdAt = Timestamp.wrap(uint64(block.timestamp));
        wasRespectedGameTypeWhenCreated =
            GameType.unwrap(ANCHOR_STATE_REGISTRY.respectedGameType()) == GameType.unwrap(GAME_TYPE);
    }

    ////////////////////////////////////////////////////////////////
    //                    Core Game Logic                         //
    ////////////////////////////////////////////////////////////////

    function challenge() external payable returns (ProposalStatus) {
        if (claimData.status != ProposalStatus.Unchallenged) revert ClaimAlreadyChallenged();
        if (!ACCESS_MANAGER.isAllowedChallenger(msg.sender)) revert BadAuth();
        if (gameOver()) revert GameOver();
        if (msg.value != CHALLENGER_BOND) revert IncorrectBondAmount();

        claimData.counteredBy = msg.sender;
        claimData.status = ProposalStatus.Challenged;
        claimData.deadline = Timestamp.wrap(uint64(block.timestamp + MAX_PROVE_DURATION.raw()));
        refundModeCredit[msg.sender] += msg.value;

        emit Challenged(claimData.counteredBy);
        return claimData.status;
    }

    /// @notice Submit chained batch proofs to verify the full state transition.
    /// @dev Can be called before or after challenge(). Early proving (before any challenge) is
    ///      intentional — TEE enclaves are trusted, so a valid proof means the claim is correct.
    ///      Once proved, gameOver() returns true, which blocks further challenges. The challenge
    ///      mechanism is an economic incentive for the TEE to prove on demand, not a fraud-proof
    ///      security layer. If the TEE is compromised, the system's security relies on enclave
    ///      revocation via TeeProofVerifier.revoke(), not on the challenge window.
    ///
    ///      Each BatchProof covers a sub-range with (startBlockHash, startStateHash, endBlockHash, endStateHash).
    ///      The contract verifies:
    ///      1. keccak256(proofs[0].startBlockHash, startStateHash) == startingOutputRoot.root
    ///      2. proofs[i].end{Block,State}Hash == proofs[i+1].start{Block,State}Hash (chain continuity)
    ///      3. proofs[i].l2Block < proofs[i+1].l2Block (monotonically increasing)
    ///      4. keccak256(proofs[last].endBlockHash, endStateHash) == rootClaim
    ///      5. proofs[last].l2Block == l2SequenceNumber
    ///      6. Each batch's TEE signature is valid (via TEE_PROOF_VERIFIER)
    /// @param proofBytes ABI-encoded BatchProof[] array
    function prove(bytes calldata proofBytes) external returns (ProposalStatus) {
        if (status != GameStatus.IN_PROGRESS) revert ClaimAlreadyResolved();
        if (gameOver()) revert GameOver();

        BatchProof[] memory proofs = abi.decode(proofBytes, (BatchProof[]));
        if (proofs.length == 0) revert EmptyBatchProofs();

        // Verify first proof starts from the starting output root
        {
            bytes32 startCombined = keccak256(abi.encode(proofs[0].startBlockHash, proofs[0].startStateHash));
            bytes32 expectedStart = Hash.unwrap(startingOutputRoot.root);
            if (startCombined != expectedStart) {
                revert StartHashMismatch(expectedStart, startCombined);
            }
        }

        uint256 prevBlock = startingOutputRoot.l2SequenceNumber;

        for (uint256 i = 0; i < proofs.length; i++) {
            // Chain continuity: each batch starts where the previous ended
            if (i > 0) {
                if (
                    proofs[i].startBlockHash != proofs[i - 1].endBlockHash
                        || proofs[i].startStateHash != proofs[i - 1].endStateHash
                ) {
                    revert BatchChainBreak(i);
                }
            }

            // L2 block must be monotonically increasing
            if (proofs[i].l2Block <= prevBlock) {
                revert BatchBlockNotIncreasing(i, prevBlock, proofs[i].l2Block);
            }

            // Compute batchDigest on-chain and verify TEE signature
            bytes32 batchDigest = keccak256(
                abi.encode(
                    proofs[i].startBlockHash,
                    proofs[i].startStateHash,
                    proofs[i].endBlockHash,
                    proofs[i].endStateHash,
                    proofs[i].l2Block
                )
            );
            TEE_PROOF_VERIFIER.verifyBatch(batchDigest, proofs[i].signature);

            prevBlock = proofs[i].l2Block;
        }

        // Final endHash must match rootClaim (which is keccak256(blockHash, stateHash))
        {
            uint256 last = proofs.length - 1;
            bytes32 endCombined = keccak256(abi.encode(proofs[last].endBlockHash, proofs[last].endStateHash));
            if (endCombined != rootClaim().raw()) {
                revert FinalHashMismatch(rootClaim().raw(), endCombined);
            }
        }

        // Final l2Block must match game's l2SequenceNumber
        if (prevBlock != l2SequenceNumber()) {
            revert FinalBlockMismatch(l2SequenceNumber(), prevBlock);
        }

        claimData.prover = msg.sender;

        if (claimData.counteredBy == address(0)) {
            claimData.status = ProposalStatus.UnchallengedAndValidProofProvided;
        } else {
            claimData.status = ProposalStatus.ChallengedAndValidProofProvided;
        }

        emit Proved(claimData.prover);
        return claimData.status;
    }

    function resolve() external returns (GameStatus) {
        if (status != GameStatus.IN_PROGRESS) revert ClaimAlreadyResolved();

        GameStatus parentGameStatus = _getParentGameStatus();
        if (parentGameStatus == GameStatus.IN_PROGRESS) revert ParentGameNotResolved();

        if (parentGameStatus == GameStatus.CHALLENGER_WINS) {
            status = GameStatus.CHALLENGER_WINS;
            // If the child was challenged, the challenger gets the bonds.
            // If the child was never challenged (counteredBy == address(0)),
            // refund the proposer — they should not lose their bond due to parent invalidation.
            address recipient = claimData.counteredBy != address(0) ? claimData.counteredBy : proposer;
            normalModeCredit[recipient] = address(this).balance;
        } else {
            if (!gameOver()) revert GameNotOver();

            if (claimData.status == ProposalStatus.Unchallenged) {
                status = GameStatus.DEFENDER_WINS;
                normalModeCredit[proposer] = address(this).balance;
            } else if (claimData.status == ProposalStatus.Challenged) {
                status = GameStatus.CHALLENGER_WINS;
                normalModeCredit[claimData.counteredBy] = address(this).balance;
            } else if (claimData.status == ProposalStatus.UnchallengedAndValidProofProvided) {
                status = GameStatus.DEFENDER_WINS;
                normalModeCredit[proposer] = address(this).balance;
            } else if (claimData.status == ProposalStatus.ChallengedAndValidProofProvided) {
                status = GameStatus.DEFENDER_WINS;
                if (claimData.prover == proposer) {
                    normalModeCredit[claimData.prover] = address(this).balance;
                } else {
                    normalModeCredit[claimData.prover] = CHALLENGER_BOND;
                    normalModeCredit[proposer] = address(this).balance - CHALLENGER_BOND;
                }
            } else {
                revert InvalidProposalStatus();
            }
        }

        claimData.status = ProposalStatus.Resolved;
        resolvedAt = Timestamp.wrap(uint64(block.timestamp));
        emit Resolved(status);

        return status;
    }

    function claimCredit(address _recipient) external {
        closeGame();

        uint256 recipientCredit;
        if (bondDistributionMode == BondDistributionMode.REFUND) {
            recipientCredit = refundModeCredit[_recipient];
        } else if (bondDistributionMode == BondDistributionMode.NORMAL) {
            recipientCredit = normalModeCredit[_recipient];
        } else {
            revert InvalidBondDistributionMode();
        }

        if (recipientCredit == 0) revert NoCreditToClaim();

        refundModeCredit[_recipient] = 0;
        normalModeCredit[_recipient] = 0;

        (bool success,) = _recipient.call{value: recipientCredit}(hex"");
        if (!success) revert BondTransferFailed();
    }

    function closeGame() public {
        if (bondDistributionMode == BondDistributionMode.REFUND || bondDistributionMode == BondDistributionMode.NORMAL)
        {
            return;
        } else if (bondDistributionMode != BondDistributionMode.UNDECIDED) {
            revert InvalidBondDistributionMode();
        }

        bool finalized = ANCHOR_STATE_REGISTRY.isGameFinalized(IDisputeGame(address(this)));
        if (!finalized) {
            revert GameNotFinalized();
        }

        try ANCHOR_STATE_REGISTRY.setAnchorState(IDisputeGame(address(this))) {} catch {}

        bool properGame = ANCHOR_STATE_REGISTRY.isGameProper(IDisputeGame(address(this)));

        if (properGame) {
            bondDistributionMode = BondDistributionMode.NORMAL;
        } else {
            bondDistributionMode = BondDistributionMode.REFUND;
        }

        emit GameClosed(bondDistributionMode);
    }

    ////////////////////////////////////////////////////////////////
    //                    View Functions                          //
    ////////////////////////////////////////////////////////////////

    function gameOver() public view returns (bool gameOver_) {
        gameOver_ = claimData.deadline.raw() < uint64(block.timestamp) || claimData.prover != address(0);
    }

    function credit(address _recipient) external view returns (uint256 credit_) {
        if (bondDistributionMode == BondDistributionMode.REFUND) {
            credit_ = refundModeCredit[_recipient];
        } else {
            credit_ = normalModeCredit[_recipient];
        }
    }

    ////////////////////////////////////////////////////////////////
    //                    IDisputeGame Impl                       //
    ////////////////////////////////////////////////////////////////

    function gameType() public view returns (GameType gameType_) { gameType_ = GAME_TYPE; }
    function gameCreator() public pure returns (address creator_) { creator_ = _getArgAddress(0x00); }
    function rootClaim() public pure returns (Claim rootClaim_) { rootClaim_ = Claim.wrap(_getArgBytes32(0x14)); }
    function l1Head() public pure returns (Hash l1Head_) { l1Head_ = Hash.wrap(_getArgBytes32(0x34)); }
    function l2SequenceNumber() public pure returns (uint256 l2SequenceNumber_) { l2SequenceNumber_ = _getArgUint256(0x54); }
    function l2BlockNumber() public pure returns (uint256 l2BlockNumber_) { l2BlockNumber_ = l2SequenceNumber(); }
    function parentIndex() public pure returns (uint32 parentIndex_) { parentIndex_ = _getArgUint32(0x74); }
    function blockHash() public pure returns (bytes32 blockHash_) { blockHash_ = _getArgBytes32(0x78); }
    function stateHash() public pure returns (bytes32 stateHash_) { stateHash_ = _getArgBytes32(0x98); }
    function startingBlockNumber() external view returns (uint256) { return startingOutputRoot.l2SequenceNumber; }
    function startingRootHash() external view returns (Hash) { return startingOutputRoot.root; }
    function extraData() public pure returns (bytes memory extraData_) { extraData_ = _getArgBytes(0x54, 0x64); }

    function gameData() external view returns (GameType gameType_, Claim rootClaim_, bytes memory extraData_) {
        gameType_ = gameType();
        rootClaim_ = rootClaim();
        extraData_ = extraData();
    }

    ////////////////////////////////////////////////////////////////
    //                    Immutable Getters                       //
    ////////////////////////////////////////////////////////////////

    function maxChallengeDuration() external view returns (Duration) { return MAX_CHALLENGE_DURATION; }
    function maxProveDuration() external view returns (Duration) { return MAX_PROVE_DURATION; }
    function disputeGameFactory() external view returns (IDisputeGameFactory) { return DISPUTE_GAME_FACTORY; }
    function teeProofVerifier() external view returns (ITeeProofVerifier) { return TEE_PROOF_VERIFIER; }
    function challengerBond() external view returns (uint256) { return CHALLENGER_BOND; }
    function anchorStateRegistry() external view returns (IAnchorStateRegistry) { return ANCHOR_STATE_REGISTRY; }
    function accessManager() external view returns (AccessManager) { return ACCESS_MANAGER; }

    ////////////////////////////////////////////////////////////////
    //                    Internal Functions                      //
    ////////////////////////////////////////////////////////////////

    function _getParentGameStatus() private view returns (GameStatus) {
        if (parentIndex() != type(uint32).max) {
            (,, IDisputeGame parentGame) = DISPUTE_GAME_FACTORY.gameAtIndex(parentIndex());
            return parentGame.status();
        } else {
            return GameStatus.DEFENDER_WINS;
        }
    }
}
