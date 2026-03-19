// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import {Test} from "forge-std/Test.sol";
import {DisputeGameFactoryRouter} from "src/dispute/DisputeGameFactoryRouter.sol";
import {IDisputeGameFactoryRouter} from "interfaces/dispute/IDisputeGameFactoryRouter.sol";
import {GameType, Claim} from "src/dispute/lib/Types.sol";
import {MockCloneableDisputeGame} from "test/dispute/tee/mocks/MockCloneableDisputeGame.sol";
import {MockDisputeGameFactory} from "test/dispute/tee/mocks/MockDisputeGameFactory.sol";

contract DisputeGameFactoryRouterCreateTest is Test {
    uint256 internal constant ZONE_ONE = 1;
    uint256 internal constant ZONE_TWO = 2;
    GameType internal constant GAME_TYPE = GameType.wrap(1960);

    DisputeGameFactoryRouter internal router;
    MockDisputeGameFactory internal factoryOne;
    MockDisputeGameFactory internal factoryTwo;
    MockCloneableDisputeGame internal gameImpl;

    function setUp() public {
        router = new DisputeGameFactoryRouter(address(this));
        factoryOne = new MockDisputeGameFactory();
        factoryTwo = new MockDisputeGameFactory();
        gameImpl = new MockCloneableDisputeGame();

        factoryOne.setImplementation(GAME_TYPE, gameImpl);
        factoryTwo.setImplementation(GAME_TYPE, gameImpl);
        factoryOne.setInitBond(GAME_TYPE, 1 ether);
        factoryTwo.setInitBond(GAME_TYPE, 2 ether);

        router.setZone(ZONE_ONE, address(factoryOne));
        router.setZone(ZONE_TWO, address(factoryTwo));
    }

    function test_create_routesToZoneFactory() public {
        Claim rootClaim = Claim.wrap(keccak256("zone-one"));
        bytes memory extraData = abi.encodePacked(uint256(1));

        address proxy = router.create{value: 1 ether}(ZONE_ONE, GAME_TYPE, rootClaim, extraData);

        assertTrue(proxy != address(0));
        assertEq(factoryOne.gameCount(), 1);
        assertEq(factoryTwo.gameCount(), 0);
    }

    function test_createBatch_routesAcrossZones() public {
        IDisputeGameFactoryRouter.CreateParams[] memory params = new IDisputeGameFactoryRouter.CreateParams[](2);
        params[0] = IDisputeGameFactoryRouter.CreateParams({
            zoneId: ZONE_ONE,
            gameType: GAME_TYPE,
            rootClaim: Claim.wrap(keccak256("zone-one")),
            extraData: abi.encodePacked(uint256(11)),
            bond: 1 ether
        });
        params[1] = IDisputeGameFactoryRouter.CreateParams({
            zoneId: ZONE_TWO,
            gameType: GAME_TYPE,
            rootClaim: Claim.wrap(keccak256("zone-two")),
            extraData: abi.encodePacked(uint256(22)),
            bond: 2 ether
        });

        address[] memory proxies = router.createBatch{value: 3 ether}(params);

        assertEq(proxies.length, 2);
        assertTrue(proxies[0] != address(0));
        assertTrue(proxies[1] != address(0));
        assertEq(factoryOne.gameCount(), 1);
        assertEq(factoryTwo.gameCount(), 1);
    }
}
