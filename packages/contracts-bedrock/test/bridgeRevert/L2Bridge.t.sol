// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.10;

import {CommonBase} from "../../lib/forge-std/src/Base.sol";
import {StdAssertions} from "../../lib/forge-std/src/StdAssertions.sol";
import {StdChains} from "../../lib/forge-std/src/StdChains.sol";
import {StdCheats, StdCheatsSafe} from "../../lib/forge-std/src/StdCheats.sol";
import {StdUtils} from "../../lib/forge-std/src/StdUtils.sol";
import {Test} from "../../lib/forge-std/src/Test.sol";
import {TestERC20} from "../mocks/TestERC20.sol";
import {TestERC721} from "../mocks/TestERC721.sol";
import {IL2ERC721Bridge} from "../../interfaces/L2/IL2ERC721Bridge.sol";
import {IL2StandardBridge} from "../../interfaces/L2/IL2StandardBridge.sol";
import {IL2CrossDomainMessenger} from "../../interfaces/L2/IL2CrossDomainMessenger.sol";
contract L2Bridge_Test is Test {
    TestERC20 token;
    TestERC721 token721;
    uint256 l2Fork;
    address testAlice;
    address testBob;
    IL2StandardBridge internal l2StandardBridge;
    IL2ERC721Bridge internal l2ERC721Bridge;
    IL2CrossDomainMessenger internal l2CrossDomainMessenger;
    function setUp() public {
        testAlice = makeAddr("testAlice");
        testBob = makeAddr("testBob");
        l2Fork = vm.createSelectFork("http://localhost:8123");
        token = new TestERC20();
        token721 = new TestERC721();
        token.mint(testAlice, 1000);
        token721.mint(testAlice, 1);
        vm.deal(testAlice, 1000 ether);
        vm.deal(testBob, 1000 ether);
        vm.label(testAlice, "testAlice");
        vm.label(testBob, "testBob");

        address l2StandardBridgeProxy = 0x4200000000000000000000000000000000000010;
         address l2CrossDomainMessengerProxy=0x4200000000000000000000000000000000000007;
        address l2ERC721BridgeProxy = 0x4200000000000000000000000000000000000014;
        l2StandardBridge = IL2StandardBridge(payable(l2StandardBridgeProxy));
        l2CrossDomainMessenger = IL2CrossDomainMessenger(l2CrossDomainMessengerProxy);
        l2ERC721Bridge = IL2ERC721Bridge(l2ERC721BridgeProxy);
    }


    function test_sendETH_reverts() public {
        vm.prank(testAlice,testAlice);
        vm.expectRevert("not allow bridge");
        address(l2StandardBridge).call{value: 1 ether}("");
    }
    function test_withdraw_reverts() public {
        vm.prank(testAlice,testAlice);
        vm.expectRevert("not allow bridge");
        l2StandardBridge.withdraw(address(token), 1, 100000, hex"");
    }

    function test_withdrawTo_reverts() public {
        vm.prank(testAlice,testAlice);
        vm.expectRevert("not allow bridge");
        l2StandardBridge.withdrawTo(address(token), testBob, 1, 100000, hex"");
    }

    function test_bridgeETH_reverts() public {
        vm.prank(testAlice,testAlice);
        vm.expectRevert("not allow bridge");
        l2StandardBridge.bridgeETH{value: 1 ether}(100000, hex"");
    }

    function test_bridgeETHTo_reverts() public {
        vm.prank(testAlice,testAlice);
        vm.expectRevert("not allow bridge");
        l2StandardBridge.bridgeETHTo(testBob, 100000, hex"");
    }

    function test_bridgeERC20_reverts() public {
        vm.prank(testAlice,testAlice);
        vm.expectRevert("not allow bridge");
        l2StandardBridge.bridgeERC20(address(token), address(token), 1, 100000, hex"");
    }

    function test_bridgeERC20To_reverts() public {
        vm.prank(testAlice,testAlice);
        vm.expectRevert("not allow bridge");
        l2StandardBridge.bridgeERC20To(address(token), address(token), testBob, 1, 100000, hex"");
    }

    function test_bridgeERC721_reverts() public {
        vm.prank(testAlice,testAlice);
        vm.expectRevert("not allow bridge");
        l2ERC721Bridge.bridgeERC721(address(token721), address(token721), 1, 100000, hex"");
    }

    function test_bridgeERC721To_reverts() public {
        vm.prank(testAlice,testAlice);
        vm.expectRevert("not allow bridge");
        l2ERC721Bridge.bridgeERC721To(address(token721), address(token721), testBob, 1, 100000, hex"");
    }

     function test_finalizeBridgeERC20_reverts() public {
        address messenger = address(l2StandardBridge.messenger());

        // Mock
        vm.mockCall(
            messenger,
            abi.encodeCall(l2CrossDomainMessenger.xDomainMessageSender, ()),
            abi.encode(address(l2StandardBridge.OTHER_BRIDGE()))
        );
        vm.expectRevert("not allow bridge");
        vm.prank(messenger,messenger);
        l2StandardBridge.finalizeBridgeERC20(address(token), address(token), testAlice, testBob, 1, hex"");
    }

    function test_finalizeBridgeETH_reverts() public {
        address messenger = address(l2StandardBridge.messenger());

        vm.mockCall(
            messenger,
            abi.encodeCall(l2CrossDomainMessenger.xDomainMessageSender, ()),
            abi.encode(address(l2StandardBridge.OTHER_BRIDGE()))
        );

        vm.startPrank(messenger);
        vm.expectRevert("not allow bridge");
        l2StandardBridge.finalizeBridgeETH(testAlice, testBob, 1 ether, hex"");
        vm.stopPrank();
    }


}
