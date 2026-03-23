// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import {Proxy} from "src/universal/Proxy.sol";
import {AnchorStateRegistry} from "src/dispute/AnchorStateRegistry.sol";
import {IDisputeGameFactory} from "interfaces/dispute/IDisputeGameFactory.sol";
import {IAnchorStateRegistry} from "interfaces/dispute/IAnchorStateRegistry.sol";
import {ISystemConfig} from "interfaces/L1/ISystemConfig.sol";
import {ITeeProofVerifier} from "interfaces/dispute/ITeeProofVerifier.sol";
import {TeeDisputeGame, TEE_DISPUTE_GAME_TYPE} from "src/dispute/tee/TeeDisputeGame.sol";
import {Claim, Duration, GameType, Hash, Proposal} from "src/dispute/lib/Types.sol";
import {MockDisputeGameFactory} from "test/dispute/tee/mocks/MockDisputeGameFactory.sol";
import {MockSystemConfig} from "test/dispute/tee/mocks/MockSystemConfig.sol";
import {MockTeeProofVerifier} from "test/dispute/tee/mocks/MockTeeProofVerifier.sol";
import {TeeTestUtils} from "test/dispute/tee/helpers/TeeTestUtils.sol";

contract AnchorStateRegistryCompatibilityTest is TeeTestUtils {
    uint256 internal constant DEFENDER_BOND = 1 ether;
    uint256 internal constant CHALLENGER_BOND = 2 ether;
    uint64 internal constant MAX_CHALLENGE_DURATION = 1 days;
    uint64 internal constant MAX_PROVE_DURATION = 12 hours;

    bytes32 internal constant ANCHOR_BLOCK_HASH = keccak256("anchor-block");
    bytes32 internal constant ANCHOR_STATE_HASH = keccak256("anchor-state");
    uint256 internal constant ANCHOR_L2_BLOCK = 10;

    MockDisputeGameFactory internal factory;
    MockSystemConfig internal systemConfig;
    MockTeeProofVerifier internal teeProofVerifier;
    TeeDisputeGame internal implementation;
    IAnchorStateRegistry internal anchorStateRegistry;

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
        systemConfig = new MockSystemConfig(address(this));
        teeProofVerifier = new MockTeeProofVerifier();

        AnchorStateRegistry anchorStateRegistryImpl = new AnchorStateRegistry(0);
        Proxy anchorStateRegistryProxy = new Proxy(address(this));
        anchorStateRegistryProxy.upgradeToAndCall(
            address(anchorStateRegistryImpl),
            abi.encodeCall(
                anchorStateRegistryImpl.initialize,
                (
                    ISystemConfig(address(systemConfig)),
                    IDisputeGameFactory(address(factory)),
                    Proposal({
                        root: Hash.wrap(computeRootClaim(ANCHOR_BLOCK_HASH, ANCHOR_STATE_HASH).raw()),
                        l2SequenceNumber: ANCHOR_L2_BLOCK
                    }),
                    GameType.wrap(TEE_DISPUTE_GAME_TYPE)
                )
            )
        );
        anchorStateRegistry = IAnchorStateRegistry(address(anchorStateRegistryProxy));

        implementation = new TeeDisputeGame(
            Duration.wrap(MAX_CHALLENGE_DURATION),
            Duration.wrap(MAX_PROVE_DURATION),
            IDisputeGameFactory(address(factory)),
            ITeeProofVerifier(address(teeProofVerifier)),
            CHALLENGER_BOND,
            anchorStateRegistry,
            proposer,
            challenger
        );

        factory.setImplementation(GameType.wrap(TEE_DISPUTE_GAME_TYPE), implementation);
        factory.setInitBond(GameType.wrap(TEE_DISPUTE_GAME_TYPE), DEFENDER_BOND);
    }

    function test_anchorStateRegistry_acceptsTeeDisputeGame() public {
        vm.warp(block.timestamp + 1);
        (TeeDisputeGame game,,) =
            _createGame(proposer, ANCHOR_L2_BLOCK + 5, type(uint32).max, keccak256("end-block"), keccak256("end-state"));

        assertTrue(anchorStateRegistry.isGameRegistered(game));
        assertTrue(anchorStateRegistry.isGameProper(game));

        teeProofVerifier.setRegistered(executor, true);
        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](1);
        proofs[0] = buildBatchProof(
            BatchInput({
                startBlockHash: ANCHOR_BLOCK_HASH,
                startStateHash: ANCHOR_STATE_HASH,
                endBlockHash: keccak256("end-block"),
                endStateHash: keccak256("end-state"),
                l2Block: game.l2SequenceNumber()
            }),
            DEFAULT_EXECUTOR_KEY
        );

        vm.prank(proposer);
        game.prove(abi.encode(proofs));
        game.resolve();

        vm.warp(block.timestamp + 1);
        AnchorStateRegistry(address(anchorStateRegistry)).setAnchorState(game);

        assertEq(address(AnchorStateRegistry(address(anchorStateRegistry)).anchorGame()), address(game));
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
            payable(address(factory.create{value: DEFENDER_BOND}(GameType.wrap(TEE_DISPUTE_GAME_TYPE), rootClaim, extraData)))
        );
        vm.stopPrank();
    }
}
