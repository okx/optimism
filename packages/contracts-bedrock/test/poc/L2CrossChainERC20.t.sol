// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import {IL2CrossDomainMessenger} from "../../interfaces/L2/IL2CrossDomainMessenger.sol";
import {IL2StandardBridge} from "../../interfaces/L2/IL2StandardBridge.sol";
import {CommonBase} from "../../lib/forge-std/src/Base.sol";
import {StdAssertions} from "../../lib/forge-std/src/StdAssertions.sol";
import {StdChains} from "../../lib/forge-std/src/StdChains.sol";
import {StdCheats, StdCheatsSafe} from "../../lib/forge-std/src/StdCheats.sol";
import {StdUtils} from "../../lib/forge-std/src/StdUtils.sol";
import {Test} from "../../lib/forge-std/src/Test.sol";
import {console} from "../../lib/forge-std/src/console.sol";
import {Predeploys} from "../../src/libraries/Predeploys.sol";
import {TestERC20} from "../mocks/TestERC20.sol";

// Testing

// Libraries

// Interfaces

/// @title L2CrossChainERC20_Test
/// @notice Test contract for L2 cross-chain ERC20 token bridging functionality
contract L2CrossChainERC20_Test is Test {
    TestERC20 internal l1Token;
    TestERC20 internal l2Token;

    // L2 contracts (using Predeploys addresses)
    IL2StandardBridge internal l2StandardBridge;
    IL2CrossDomainMessenger internal l2CrossDomainMessenger;

    address internal testAlice;
    address internal testBob;

    uint256 internal constant INITIAL_SUPPLY = 1000000 * 1e18;
    uint256 internal constant BRIDGE_AMOUNT = 100 * 1e18;

    // Fork configuration
    uint256 internal l2Fork;
    string internal l2RpcUrl = "http://localhost:8123";

    /// @notice Sets up the L2 testing environment
    function setUp() public {
        // Create test accounts
        testAlice = makeAddr("testAlice");
        testBob = makeAddr("testBob");

        // Try to set up L2 fork if RPC URL is available
        setupL2Fork();

        // Fund accounts with ETH (after fork setup)
        vm.deal(testAlice, 100 ether);
        vm.deal(testBob, 100 ether);

        // Set up L2 contracts using Predeploys
        l2StandardBridge = IL2StandardBridge(payable(Predeploys.L2_STANDARD_BRIDGE));
        l2CrossDomainMessenger = IL2CrossDomainMessenger(Predeploys.L2_CROSS_DOMAIN_MESSENGER);

        // Deploy test tokens
        deployTestTokens();

        // Label addresses for debugging
        labelAddresses();

        console.log("=== L2 Setup Complete ===");
        console.log("L2 Standard Bridge:", address(l2StandardBridge));
        console.log("L2 Cross Domain Messenger:", address(l2CrossDomainMessenger));
        console.log("L1 Token:", address(l1Token));
        console.log("L2 Token:", address(l2Token));
        console.log("Using L2 fork:", l2Fork != 0);
    }

    /// @notice Setup L2 fork environment
    function setupL2Fork() internal {
        try vm.createSelectFork(l2RpcUrl) returns (uint256 forkId) {
            l2Fork = forkId;
            console.log("L2 fork created and selected with RPC:", l2RpcUrl);
        } catch {
            console.log("Failed to create L2 fork, using local environment");
        }
    }

    /// @notice Deploy test ERC20 tokens
    function deployTestTokens() internal {
        vm.startPrank(testAlice);

        // Deploy tokens
        l1Token = new TestERC20();
        l2Token = new TestERC20();

        // Mint initial supply
        l1Token.mint(testAlice, INITIAL_SUPPLY);
        l1Token.mint(testBob, INITIAL_SUPPLY / 2);

        l2Token.mint(testAlice, INITIAL_SUPPLY);
        l2Token.mint(testBob, INITIAL_SUPPLY / 2);

        vm.stopPrank();
    }

    /// @notice Label addresses for better debugging
    function labelAddresses() internal {
        vm.label(address(l1Token), "L1Token");
        vm.label(address(l2Token), "L2Token");
        vm.label(testAlice, "TestAlice");
        vm.label(testBob, "TestBob");
        vm.label(address(l2StandardBridge), "L2StandardBridge");
        vm.label(address(l2CrossDomainMessenger), "L2CrossDomainMessenger");
    }

    /// @notice Test that withdraw reverts with "not allow bridge"
    function test_withdraw_reverts() public {
        vm.prank(testAlice, testAlice);
        vm.expectRevert("not allow bridge");
        l2StandardBridge.withdraw(
            address(l2Token),
            1,
            50000,
            ""
        );
    }

    /// @notice Test that withdrawTo reverts with "not allow bridge"
    function test_withdrawTo_reverts() public {
        vm.prank(testAlice);
        vm.expectRevert("not allow bridge");
        l2StandardBridge.withdrawTo(
            address(l2Token),
            testAlice,
            1,
            50000,
            ""
        );
    }

    /// @notice Test that L2 bridgeERC20To reverts with "not allow bridge" (fork only)
    function test_l2_bridgeERC20To_reverts() public {
        // Skip if not on fork or if contract doesn't exist
        if (bytes(l2RpcUrl).length == 0 || address(l2StandardBridge).code.length == 0) {
            vm.skip(true);
            return;
        }

        // Approve bridge to spend tokens
        vm.prank(testAlice);
        l2Token.approve(address(l2StandardBridge), BRIDGE_AMOUNT);

        // Attempt to bridge tokens from L2 to L1 - should revert with "not allow bridge"
        vm.prank(testAlice);
        vm.expectRevert("not allow bridge");
        l2StandardBridge.bridgeERC20To(
            address(l2Token),  // L2 token
            address(l1Token),  // L1 token
            testBob,
            BRIDGE_AMOUNT,
            200000, // minGasLimit
            ""      // extraData
        );
    }
/// @notice Test that bridgeETHTo reverts with "not allow bridge"
    function test_bridgeETHTo_reverts() public {
        vm.prank(testAlice, testAlice); // Set both msg.sender and tx.origin to testAlice
        vm.expectRevert("not allow bridge");
        l2StandardBridge.bridgeETHTo{value: 1}(
            testBob,
            200000, // minGasLimit
            ""      // extraData
        );
    }
    /// @notice Test that bridgeETH reverts with "not allow bridge"
    function test_bridgeETH_reverts() public {
        vm.prank(testAlice, testAlice);
        vm.expectRevert("not allow bridge");
        l2StandardBridge.bridgeETH{value: 1 ether}(
            200000, // minGasLimit
            ""      // extraData
        );
    }
    /// @notice Test that bridgeERC20To reverts with "not allow bridge"
    function test_bridgeERC20To_reverts() public {
        // Approve bridge to spend tokens
        vm.prank(testAlice);
        l2Token.approve(address(l2StandardBridge), BRIDGE_AMOUNT);

        vm.prank(testAlice);
        vm.expectRevert("not allow bridge");
        l2StandardBridge.bridgeERC20To(
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
        l2Token.approve(address(l2StandardBridge), BRIDGE_AMOUNT);

        vm.prank(testAlice, testAlice);
        vm.expectRevert("not allow bridge");
        l2StandardBridge.bridgeERC20(
            address(l1Token),
            address(l2Token),
            BRIDGE_AMOUNT,
            200000, // minGasLimit
            ""      // extraData
        );
    }

}
