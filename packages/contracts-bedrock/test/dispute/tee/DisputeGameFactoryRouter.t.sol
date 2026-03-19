// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import {Test} from "forge-std/Test.sol";
import {DisputeGameFactoryRouter} from "src/dispute/DisputeGameFactoryRouter.sol";
import {IDisputeGameFactoryRouter} from "interfaces/dispute/IDisputeGameFactoryRouter.sol";
import {GameType, Claim} from "src/dispute/lib/Types.sol";

contract DisputeGameFactoryRouterTest is Test {
    DisputeGameFactoryRouter public router;

    // XLayer factory on ETH mainnet
    address constant XLAYER_FACTORY = 0x9D4c8FAEadDdDeeE1Ed0c92dAbAD815c2484f675;

    address owner;
    address alice;

    uint256 constant ZONE_XLAYER = 1;
    uint256 constant ZONE_OTHER = 2;

    function setUp() public {
        owner = address(this);
        alice = makeAddr("alice");
        router = new DisputeGameFactoryRouter();
    }

    ////////////////////////////////////////////////////////////////
    //                    Zone Management Tests                    //
    ////////////////////////////////////////////////////////////////

    function test_setZone_register() public {
        router.setZone(ZONE_XLAYER, XLAYER_FACTORY);
        assertEq(router.factories(ZONE_XLAYER), XLAYER_FACTORY);
    }

    function test_setZone_update() public {
        router.setZone(ZONE_XLAYER, XLAYER_FACTORY);
        address newFactory = makeAddr("newFactory");
        router.setZone(ZONE_XLAYER, newFactory);
        assertEq(router.factories(ZONE_XLAYER), newFactory);
    }

    function test_setZone_remove() public {
        router.setZone(ZONE_XLAYER, XLAYER_FACTORY);
        router.setZone(ZONE_XLAYER, address(0));
        assertEq(router.factories(ZONE_XLAYER), address(0));
    }

    function test_setZone_emitsEvent() public {
        vm.expectEmit(true, true, true, true);
        emit IDisputeGameFactoryRouter.ZoneSet(ZONE_XLAYER, address(0), XLAYER_FACTORY);
        router.setZone(ZONE_XLAYER, XLAYER_FACTORY);
    }

    function test_setZone_revertNotOwner() public {
        vm.prank(alice);
        vm.expectRevert("Ownable: caller is not the owner");
        router.setZone(ZONE_XLAYER, XLAYER_FACTORY);
    }

    ////////////////////////////////////////////////////////////////
    //                    Create Tests                             //
    ////////////////////////////////////////////////////////////////

    function test_create_revertZoneNotRegistered() public {
        vm.expectRevert(abi.encodeWithSelector(IDisputeGameFactoryRouter.ZoneNotRegistered.selector, ZONE_XLAYER));
        router.create(ZONE_XLAYER, GameType.wrap(0), Claim.wrap(bytes32(0)), "");
    }

    function test_createBatch_revertEmpty() public {
        IDisputeGameFactoryRouter.CreateParams[] memory params = new IDisputeGameFactoryRouter.CreateParams[](0);
        vm.expectRevert(IDisputeGameFactoryRouter.BatchEmpty.selector);
        router.createBatch(params);
    }

    function test_createBatch_revertBondMismatch() public {
        router.setZone(ZONE_XLAYER, XLAYER_FACTORY);

        IDisputeGameFactoryRouter.CreateParams[] memory params = new IDisputeGameFactoryRouter.CreateParams[](1);
        params[0] = IDisputeGameFactoryRouter.CreateParams({
            zoneId: ZONE_XLAYER,
            gameType: GameType.wrap(0),
            rootClaim: Claim.wrap(bytes32(0)),
            extraData: "",
            bond: 1 ether
        });

        vm.expectRevert(abi.encodeWithSelector(IDisputeGameFactoryRouter.BatchBondMismatch.selector, 1 ether, 0));
        router.createBatch(params);
    }

    ////////////////////////////////////////////////////////////////
    //                    View Function Tests                      //
    ////////////////////////////////////////////////////////////////

    function test_factories_unregistered() public view {
        assertEq(router.factories(999), address(0));
    }

    function test_factories_mapping() public {
        router.setZone(ZONE_XLAYER, XLAYER_FACTORY);
        assertEq(router.factories(ZONE_XLAYER), XLAYER_FACTORY);
    }

    function test_version() public view {
        assertEq(router.version(), "1.0.0");
    }
}
