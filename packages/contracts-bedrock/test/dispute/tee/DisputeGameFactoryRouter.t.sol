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
    //                    Zone CRUD Tests                         //
    ////////////////////////////////////////////////////////////////

    function test_registerZone() public {
        router.registerZone(ZONE_XLAYER, XLAYER_FACTORY);
        assertEq(router.getFactory(ZONE_XLAYER), XLAYER_FACTORY);
    }

    function test_registerZone_revertDuplicate() public {
        router.registerZone(ZONE_XLAYER, XLAYER_FACTORY);
        vm.expectRevert(abi.encodeWithSelector(IDisputeGameFactoryRouter.ZoneAlreadyRegistered.selector, ZONE_XLAYER));
        router.registerZone(ZONE_XLAYER, XLAYER_FACTORY);
    }

    function test_registerZone_revertZeroAddress() public {
        vm.expectRevert(IDisputeGameFactoryRouter.ZeroAddress.selector);
        router.registerZone(ZONE_XLAYER, address(0));
    }

    function test_updateZone() public {
        router.registerZone(ZONE_XLAYER, XLAYER_FACTORY);
        address newFactory = makeAddr("newFactory");
        router.updateZone(ZONE_XLAYER, newFactory);
        assertEq(router.getFactory(ZONE_XLAYER), newFactory);
    }

    function test_updateZone_revertNotRegistered() public {
        vm.expectRevert(abi.encodeWithSelector(IDisputeGameFactoryRouter.ZoneNotRegistered.selector, ZONE_XLAYER));
        router.updateZone(ZONE_XLAYER, XLAYER_FACTORY);
    }

    function test_removeZone() public {
        router.registerZone(ZONE_XLAYER, XLAYER_FACTORY);
        router.removeZone(ZONE_XLAYER);
        assertEq(router.getFactory(ZONE_XLAYER), address(0));
    }

    function test_removeZone_revertNotRegistered() public {
        vm.expectRevert(abi.encodeWithSelector(IDisputeGameFactoryRouter.ZoneNotRegistered.selector, ZONE_XLAYER));
        router.removeZone(ZONE_XLAYER);
    }

    ////////////////////////////////////////////////////////////////
    //                    Access Control Tests                    //
    ////////////////////////////////////////////////////////////////

    function test_registerZone_revertNotOwner() public {
        vm.prank(alice);
        vm.expectRevert("Ownable: caller is not the owner");
        router.registerZone(ZONE_XLAYER, XLAYER_FACTORY);
    }

    function test_updateZone_revertNotOwner() public {
        router.registerZone(ZONE_XLAYER, XLAYER_FACTORY);
        vm.prank(alice);
        vm.expectRevert("Ownable: caller is not the owner");
        router.updateZone(ZONE_XLAYER, makeAddr("newFactory"));
    }

    function test_removeZone_revertNotOwner() public {
        router.registerZone(ZONE_XLAYER, XLAYER_FACTORY);
        vm.prank(alice);
        vm.expectRevert("Ownable: caller is not the owner");
        router.removeZone(ZONE_XLAYER);
    }

    ////////////////////////////////////////////////////////////////
    //                    Create Tests (Fork)                     //
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
        router.registerZone(ZONE_XLAYER, XLAYER_FACTORY);

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
    //                    View Function Tests                     //
    ////////////////////////////////////////////////////////////////

    function test_getFactory_unregistered() public view {
        assertEq(router.getFactory(999), address(0));
    }

    function test_factories_mapping() public {
        router.registerZone(ZONE_XLAYER, XLAYER_FACTORY);
        assertEq(router.factories(ZONE_XLAYER), XLAYER_FACTORY);
    }

    function test_version() public view {
        assertEq(router.version(), "1.0.0");
    }
}
