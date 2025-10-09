// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import {IL1CrossDomainMessenger} from "../../interfaces/L1/IL1CrossDomainMessenger.sol";
import {IL1StandardBridge} from "../../interfaces/L1/IL1StandardBridge.sol";
import {CommonBase} from "../../lib/forge-std/src/Base.sol";
import {StdAssertions} from "../../lib/forge-std/src/StdAssertions.sol";
import {StdChains} from "../../lib/forge-std/src/StdChains.sol";
import {StdCheats, StdCheatsSafe} from "../../lib/forge-std/src/StdCheats.sol";
import {StdUtils} from "../../lib/forge-std/src/StdUtils.sol";
import {Test} from "../../lib/forge-std/src/Test.sol";
import {console} from "../../lib/forge-std/src/console.sol";
import {TestERC20} from "../mocks/TestERC20.sol";

// Testing

// Libraries

// Interfaces

/// @title CrossChainERC20_Test
/// @notice Test contract for cross-chain ERC20 token bridging functionality
contract CrossChainERC20_Test is Test {
    TestERC20 internal l1Token;
    TestERC20 internal l2Token;

    // L1 contracts
    IL1StandardBridge internal l1StandardBridge;
    IL1CrossDomainMessenger internal l1CrossDomainMessenger;

    address internal testAlice;
    address internal testBob;

    uint256 internal constant INITIAL_SUPPLY = 1000000 * 1e18;
    uint256 internal constant BRIDGE_AMOUNT = 100 * 1e18;

    // Fork configuration
    uint256 internal l1Fork;
    string internal l1RpcUrl = "http://localhost:8545";

    event TestERC20BridgeInitiated(
        address indexed localToken,
        address indexed remoteToken,
        address indexed from,
        address to,
        uint256 amount,
        bytes extraData
    );

    event TestERC20BridgeFinalized(
        address indexed localToken,
        address indexed remoteToken,
        address indexed from,
        address to,
        uint256 amount,
        bytes extraData
    );

    /// @notice Sets up the testing environment
    function setUp() public {
        // Create test accounts
        testAlice = makeAddr("testAlice");
        testBob = makeAddr("testBob");

        // Setup L1 fork
        setupL1Fork();

        // Fund accounts with ETH
        vm.deal(testAlice, 10 ether);
        vm.deal(testBob, 10 ether);

        // Setup L1 contracts (these should be deployed on the forked network)
        setupL1Contracts();

        // Deploy test ERC20 tokens
        l1Token = new TestERC20();
        l2Token = new TestERC20();

        // Mint initial supply to testAlice
        l1Token.mint(testAlice, INITIAL_SUPPLY);
        l2Token.mint(testAlice, INITIAL_SUPPLY);

        // Label addresses for better debugging
        labelAddresses();

        console.log("=== L1 Setup Complete ===");
        console.log("L1 Standard Bridge:", address(l1StandardBridge));
        console.log("L1 Cross Domain Messenger:", address(l1CrossDomainMessenger));
        console.log("L1 Token:", address(l1Token));
        console.log("L2 Token:", address(l2Token));
    }

    /// @notice Setup L1 fork environment
    function setupL1Fork() internal {
        try vm.createSelectFork(l1RpcUrl) returns (uint256 forkId) {
            l1Fork = forkId;
            console.log("L1 fork created and selected with RPC:", l1RpcUrl);
        } catch {
            console.log("Failed to create L1 fork, using local environment");
        }
    }

    /// @notice Setup L1 contracts (assuming they're deployed on the forked network)
    function setupL1Contracts() internal {

        l1StandardBridge = IL1StandardBridge(payable(vm.parseAddress("0xf209c8f3cb9872bf49c3bbdb0948ed059b806c6c")));
        l1CrossDomainMessenger = IL1CrossDomainMessenger(vm.parseAddress("0x504a0094f9492894243a8ea14b33ed7eeb619e84"));

    }

    /// @notice Label addresses for better debugging
    function labelAddresses() internal {
        vm.label(address(l1Token), "L1Token");
        vm.label(address(l2Token), "L2Token");
        vm.label(testAlice, "TestAlice");
        vm.label(testBob, "TestBob");
        vm.label(address(l1StandardBridge), "L1StandardBridge");
        vm.label(address(l1CrossDomainMessenger), "L1CrossDomainMessenger");
    }

    /// @notice Test that bridgeETHTo reverts with "not allow bridge"
    function test_bridgeETHTo_reverts() public {
        vm.prank(testAlice);
        vm.expectRevert("not allow bridge");
        l1StandardBridge.bridgeETHTo{value: 1 ether}(
            testBob,
            200000, // minGasLimit
            ""      // extraData
        );
    }
    /// @notice Test that bridgeETH reverts with "not allow bridge"
    function test_bridgeETH_reverts() public {
        vm.prank(testAlice, testAlice);
        vm.expectRevert("not allow bridge");
        l1StandardBridge.bridgeETH{value: 1 ether}(
            200000, // minGasLimit
            ""      // extraData
        );
    }

    /// @notice Test that depositERC20To reverts with "not allow bridge"
    function test_depositERC20To_reverts() public {
        // Approve bridge to spend tokens
        vm.prank(testAlice);
        l1Token.approve(address(l1StandardBridge), BRIDGE_AMOUNT);

        vm.prank(testAlice);
        vm.expectRevert("not allow bridge");
        l1StandardBridge.depositERC20To(
            address(l1Token),
            address(l2Token),
            testBob,
            BRIDGE_AMOUNT,
            200000, // minGasLimit
            ""      // extraData
        );
    }
    /// @notice Test that bridgeERC20To reverts with "not allow bridge"
    function test_bridgeERC20To_reverts() public {
        // Approve bridge to spend tokens
        vm.prank(testAlice, testAlice);
        l1Token.approve(address(l1StandardBridge), BRIDGE_AMOUNT);

        vm.prank(testAlice, testAlice);
        vm.expectRevert("not allow bridge");
        l1StandardBridge.bridgeERC20To(
            address(l1Token),
            address(l2Token),
            testBob,
            BRIDGE_AMOUNT,
            200000, // minGasLimit
            ""      // extraData
        );
    }

    /// @notice Test that bridgeERC20 reverts with "not allow bridge"
    function test_bridgeERC20_reverts() public {
        // Approve bridge to spend tokens
        vm.prank(testAlice, testAlice);
        l1Token.approve(address(l1StandardBridge), BRIDGE_AMOUNT);

        vm.prank(testAlice, testAlice);
        vm.expectRevert("not allow bridge");
        l1StandardBridge.bridgeERC20(
            address(l1Token),
            address(l2Token),
            BRIDGE_AMOUNT,
            200000, // minGasLimit
            ""      // extraData
        );
    }

    /// @notice Test that finalizeETHWithdrawal reverts with "not allow bridge"
    function test_finalizeETHWithdrawal_reverts() public {
        vm.expectRevert("not allow bridge");
        l1StandardBridge.finalizeETHWithdrawal{value: 1 ether}(
            testAlice,
            testBob,
            1 ether,
            ""
        );
    }

    /// @notice Test that finalizeERC20Withdrawal reverts with "not allow bridge"
    function test_finalizeERC20Withdrawal_reverts() public {
        vm.expectRevert("not allow bridge");
        l1StandardBridge.finalizeERC20Withdrawal(
            address(l1Token),
            address(l2Token),
            testAlice,
            testBob,
            BRIDGE_AMOUNT,
            ""
        );
    }


}
