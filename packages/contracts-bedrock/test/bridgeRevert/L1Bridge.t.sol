// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.10;

import {IL1CrossDomainMessenger} from "../../interfaces/L1/IL1CrossDomainMessenger.sol";
import {IL1StandardBridge} from "../../interfaces/L1/IL1StandardBridge.sol";
import {CommonBase} from "../../lib/forge-std/src/Base.sol";
import {StdAssertions} from "../../lib/forge-std/src/StdAssertions.sol";
import {StdChains} from "../../lib/forge-std/src/StdChains.sol";
import {StdCheats, StdCheatsSafe} from "../../lib/forge-std/src/StdCheats.sol";
import {StdUtils} from "../../lib/forge-std/src/StdUtils.sol";
import {Test} from "../../lib/forge-std/src/Test.sol";
import {TestERC20} from "../mocks/TestERC20.sol";
import {TestERC721} from "../mocks/TestERC721.sol";
import {IL1ERC721Bridge} from "../../interfaces/L1/IL1ERC721Bridge.sol";

contract L1Bridge_Test is Test {
    TestERC20 token;
    TestERC721 token721;
    uint256 l1Fork;
    address testAlice;
    address testBob;
    IL1StandardBridge internal l1StandardBridge;
    IL1CrossDomainMessenger internal l1CrossDomainMessenger;
    IL1ERC721Bridge internal l1ERC721Bridge;

    function setUp() public {
        testAlice = makeAddr("testAlice");
        testBob = makeAddr("testBob");
        l1Fork = vm.createSelectFork("http://localhost:8545");
        token = new TestERC20();
        token721 = new TestERC721();
        token.mint(testAlice, 1000);
        token721.mint(testAlice, 1);
        vm.deal(testAlice, 1000 ether);
        vm.deal(testBob, 1000 ether);
        vm.label(testAlice, "testAlice");
        vm.label(testBob, "testBob");
        string memory stateJson = vm.readFile("../../test/config-op/state.json");

        address l1StandardBridgeProxy = vm.parseJsonAddress(stateJson, ".opChainDeployments[0].L1StandardBridgeProxy");
        address l1CrossDomainMessengerProxy = vm.parseJsonAddress(stateJson, ".opChainDeployments[0].L1CrossDomainMessengerProxy");
        address l1ERC721BridgeProxy = vm.parseJsonAddress(stateJson, ".opChainDeployments[0].L1Erc721BridgeProxy");
        l1StandardBridge = IL1StandardBridge(payable(l1StandardBridgeProxy));
        l1CrossDomainMessenger = IL1CrossDomainMessenger(l1CrossDomainMessengerProxy);
        l1ERC721Bridge = IL1ERC721Bridge(l1ERC721BridgeProxy);
    }


    function test_sendETH_reverts() public {
        vm.prank(testAlice,testAlice);
        vm.expectRevert("not allow bridge");
        address(l1StandardBridge).call{value: 1 ether}("");
    }
    function test_depositETH_reverts() public {
        vm.prank(testAlice,testAlice);
        vm.expectRevert("not allow bridge");
        l1StandardBridge.depositETH(100000, hex"");
    }
    function test_depositETHTo_reverts() public {
        vm.prank(testAlice,testAlice);
        vm.expectRevert("not allow bridge");
        l1StandardBridge.depositETHTo(testBob, 100000, hex"");
    }

    function test_depositERC20_reverts() public {
        vm.prank(testAlice,testAlice);
        token.approve(address(l1StandardBridge), 1);

        vm.prank(testAlice,testAlice);
        vm.expectRevert("not allow bridge");
        l1StandardBridge.depositERC20(address(token), address(token), 1000, 100000, hex"");
    }

    function test_depositERC20To_reverts() public {
        // Approve bridge to spend tokens
        vm.prank(testAlice,testAlice);
        token.approve(address(l1StandardBridge), 1);

        vm.prank(testAlice,testAlice);
        vm.expectRevert("not allow bridge");
        l1StandardBridge.depositERC20To(
            address(token),
            address(token),
            testBob,
            1,
            200000, // minGasLimit
            ""      // extraData
        );
    }


    function test_finalizeETHWithdrawal_reverts() public {
        vm.expectRevert("not allow bridge");
        l1StandardBridge.finalizeETHWithdrawal{value: 1 ether}(
            testAlice,
            testBob,
            1 ether,
            ""
        );
    }

    function test_finalizeERC20Withdrawal_reverts() public {
        vm.expectRevert("not allow bridge");
        l1StandardBridge.finalizeERC20Withdrawal(
            address(token),
            address(token),
            testAlice,
            testBob,
            1,
            ""
        );
    }

    function test_bridgeETH_reverts() public {
        vm.prank(testAlice,testAlice);
        vm.expectRevert("not allow bridge");
        l1StandardBridge.bridgeETH{value: 1 ether}(
            200000, // minGasLimit
            ""      // extraData
        );
    }

    function test_bridgeETHTo_reverts() public {
        vm.prank(testAlice,testAlice);
        vm.expectRevert("not allow bridge");
        l1StandardBridge.bridgeETHTo(testBob, 100000, hex"");
    }

    function test_bridgeERC20_reverts() public {
        vm.prank(testAlice,testAlice);
        vm.expectRevert("not allow bridge");
        l1StandardBridge.bridgeERC20(address(token), address(token), 1, 200000, hex"");
    }

    function test_bridgeERC20To_reverts() public {
        vm.prank(testAlice,testAlice);
        vm.expectRevert("not allow bridge");
        l1StandardBridge.bridgeERC20To(address(token), address(token), testBob, 1, 200000, hex"");
    }

    function test_bridgeERC721_reverts() public {
        vm.prank(testAlice,testAlice);
        vm.expectRevert("not allow bridge");
        l1ERC721Bridge.bridgeERC721(address(token721), address(token721), 1, 200000, hex"");
    }

    function test_bridgeERC721To_reverts() public {
        vm.prank(testAlice,testAlice);
        vm.expectRevert("not allow bridge");
        l1ERC721Bridge.bridgeERC721To(address(token721), address(token721), testBob, 1, 200000, hex"");
    }


    function test_finalizeBridgeERC20_reverts() public {
        address messenger = address(l1StandardBridge.messenger());

        // Mock
        vm.mockCall(
            messenger,
            abi.encodeCall(l1CrossDomainMessenger.xDomainMessageSender, ()),
            abi.encode(address(l1StandardBridge.OTHER_BRIDGE()))
        );
        vm.expectRevert("not allow bridge");
        vm.prank(messenger,messenger);
        l1StandardBridge.finalizeBridgeERC20(address(token), address(token), testAlice, testBob, 1, hex"");
    }

    function test_finalizeBridgeETH_reverts() public {
        address messenger = address(l1StandardBridge.messenger());

        vm.mockCall(
            messenger,
            abi.encodeCall(l1CrossDomainMessenger.xDomainMessageSender, ()),
            abi.encode(address(l1StandardBridge.OTHER_BRIDGE()))
        );

        vm.startPrank(messenger);
        vm.expectRevert("not allow bridge");
        l1StandardBridge.finalizeBridgeETH(testAlice, testBob, 1 ether, hex"");
        vm.stopPrank();
    }


}
