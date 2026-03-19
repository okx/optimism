// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import {Test} from "forge-std/Test.sol";
import {AccessManager, TEE_DISPUTE_GAME_TYPE} from "src/dispute/tee/AccessManager.sol";
import {IDisputeGame} from "interfaces/dispute/IDisputeGame.sol";
import {IDisputeGameFactory} from "interfaces/dispute/IDisputeGameFactory.sol";
import {IAnchorStateRegistry} from "interfaces/dispute/IAnchorStateRegistry.sol";
import {GameType, Claim, GameStatus} from "src/dispute/lib/Types.sol";
import {MockDisputeGameFactory} from "test/dispute/tee/mocks/MockDisputeGameFactory.sol";
import {MockStatusDisputeGame} from "test/dispute/tee/mocks/MockStatusDisputeGame.sol";

contract AccessManagerTest is Test {
    uint256 internal constant FALLBACK_TIMEOUT = 7 days;

    MockDisputeGameFactory internal factory;
    AccessManager internal accessManager;

    function setUp() public {
        factory = new MockDisputeGameFactory();
        accessManager = new AccessManager(FALLBACK_TIMEOUT, IDisputeGameFactory(address(factory)));
    }

    function test_getLastProposalTimestamp_returnsDeploymentTimestampWhenNoGames() public view {
        assertEq(accessManager.getLastProposalTimestamp(), accessManager.DEPLOYMENT_TIMESTAMP());
    }

    function test_getLastProposalTimestamp_scansBackwardForLatestTeeGame() public {
        factory.pushGame(
            GameType.wrap(100),
            uint64(block.timestamp + 1),
            IDisputeGame(address(_mockGame(GameType.wrap(100), 1))),
            Claim.wrap(bytes32(uint256(1))),
            bytes("")
        );
        factory.pushGame(
            GameType.wrap(TEE_DISPUTE_GAME_TYPE),
            uint64(block.timestamp + 2),
            IDisputeGame(address(_mockGame(GameType.wrap(TEE_DISPUTE_GAME_TYPE), 2))),
            Claim.wrap(bytes32(uint256(2))),
            bytes("")
        );
        factory.pushGame(
            GameType.wrap(200),
            uint64(block.timestamp + 3),
            IDisputeGame(address(_mockGame(GameType.wrap(200), 3))),
            Claim.wrap(bytes32(uint256(3))),
            bytes("")
        );

        assertEq(accessManager.getLastProposalTimestamp(), block.timestamp + 2);
    }

    function test_getLastProposalTimestamp_returnsDeploymentTimestampWhenLatestTeeGameIsOlderThanDeployment()
        public
    {
        factory.pushGame(
            GameType.wrap(TEE_DISPUTE_GAME_TYPE),
            uint64(accessManager.DEPLOYMENT_TIMESTAMP() - 1),
            IDisputeGame(address(_mockGame(GameType.wrap(TEE_DISPUTE_GAME_TYPE), 1))),
            Claim.wrap(bytes32(uint256(1))),
            bytes("")
        );

        assertEq(accessManager.getLastProposalTimestamp(), accessManager.DEPLOYMENT_TIMESTAMP());
    }

    function test_isProposalPermissionlessMode_activatesAfterFallbackTimeout() public {
        vm.warp(block.timestamp + FALLBACK_TIMEOUT + 1);
        assertTrue(accessManager.isProposalPermissionlessMode());
    }

    function test_isProposalPermissionlessMode_zeroAddressOverride() public {
        accessManager.setProposer(address(0), true);
        assertTrue(accessManager.isProposalPermissionlessMode());
    }

    function test_isAllowedProposer_returnsTrueForWhitelistedProposer() public {
        address proposer = makeAddr("proposer");
        accessManager.setProposer(proposer, true);
        assertTrue(accessManager.isAllowedProposer(proposer));
    }

    function test_isAllowedChallenger_respectsZeroAddressWildcard() public {
        address challenger = makeAddr("challenger");
        accessManager.setChallenger(address(0), true);
        assertTrue(accessManager.isAllowedChallenger(challenger));
    }

    function test_isAllowedChallenger_returnsFalseForUnlistedChallenger() public {
        assertFalse(accessManager.isAllowedChallenger(makeAddr("challenger")));
    }

    function _mockGame(GameType gameType_, uint256 nonce) internal returns (MockStatusDisputeGame) {
        return new MockStatusDisputeGame({
            creator_: vm.addr(nonce + 1),
            gameType_: gameType_,
            rootClaim_: Claim.wrap(bytes32(nonce)),
            l2SequenceNumber_: nonce,
            extraData_: bytes(""),
            status_: GameStatus.IN_PROGRESS,
            createdAt_: uint64(block.timestamp),
            resolvedAt_: 0,
            respected_: true,
            anchorStateRegistry_: IAnchorStateRegistry(address(0))
        });
    }
}
