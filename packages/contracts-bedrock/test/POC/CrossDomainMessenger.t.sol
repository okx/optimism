// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import {IL1CrossDomainMessenger} from "../../interfaces/L1/IL1CrossDomainMessenger.sol";
import {IL2CrossDomainMessenger} from "../../interfaces/L2/IL2CrossDomainMessenger.sol";
import {CommonBase} from "../../lib/forge-std/src/Base.sol";
import {StdAssertions} from "../../lib/forge-std/src/StdAssertions.sol";
import {StdChains} from "../../lib/forge-std/src/StdChains.sol";
import {StdCheats, StdCheatsSafe} from "../../lib/forge-std/src/StdCheats.sol";
import {StdUtils} from "../../lib/forge-std/src/StdUtils.sol";
import {Test} from "../../lib/forge-std/src/Test.sol";
import {console} from "../../lib/forge-std/src/console.sol";
import {Predeploys} from "../../src/libraries/Predeploys.sol";

/// @title SimpleReceiver
/// @notice A simple contract to receive cross-domain messages
contract SimpleReceiver {
    event MessageReceived(address sender, bytes data);

    string public lastMessage;
    address public lastSender;
    uint256 public messageCount;

    function receiveMessage(string memory _message) external {
        lastMessage = _message;
        lastSender = msg.sender;
        messageCount++;
        emit MessageReceived(msg.sender, abi.encode(_message));
    }

    function getMessageInfo() external view returns (string memory, address, uint256) {
        return (lastMessage, lastSender, messageCount);
    }
}

/// @title POC_CrossDomainMessenger_Test
/// @notice Test contract for cross-domain messaging functionality
contract POC_CrossDomainMessenger_Test is Test {
    // L1 contracts
    IL1CrossDomainMessenger internal l1CrossDomainMessenger;

    // L2 contracts
    IL2CrossDomainMessenger internal l2CrossDomainMessenger;

    // Test contracts
    SimpleReceiver internal l1Receiver;
    SimpleReceiver internal l2Receiver;

    // Test accounts
    address internal testAlice;
    address internal testBob;

    // Fork configuration
    uint256 internal l1Fork;
    uint256 internal l2Fork;
    bool internal l1ForkSuccess;
    bool internal l2ForkSuccess;
    string internal l1RpcUrl = "http://localhost:8545";
    string internal l2RpcUrl = "http://localhost:8123";

    // Test constants
    uint32 internal constant MIN_GAS_LIMIT = 200000;
    string internal constant TEST_MESSAGE = "Hello Cross Domain!";

    /// @notice Sets up the testing environment
    function setUp() public {
        // Create test accounts
        testAlice = makeAddr("testAlice");
        testBob = makeAddr("testBob");

        // Setup L1 fork
        setupL1Fork();

        // Fund accounts with ETH on L1
        vm.deal(testAlice, 100 ether);
        vm.deal(testBob, 100 ether);

        // Setup L1 contracts
        setupL1Contracts();

        // Deploy L1 receiver
        l1Receiver = new SimpleReceiver();

        // Setup L2 fork
        setupL2Fork();

        // Fund accounts with ETH on L2
        vm.deal(testAlice, 100 ether);
        vm.deal(testBob, 100 ether);

        // Setup L2 contracts
        setupL2Contracts();

        // Deploy L2 receiver
        l2Receiver = new SimpleReceiver();

        // Label addresses for debugging
        labelAddresses();

        console.log("=== CrossDomain Setup Complete ===");
        console.log("L1 CrossDomain Messenger:", address(l1CrossDomainMessenger));
        console.log("L2 CrossDomain Messenger:", address(l2CrossDomainMessenger));
        console.log("L1 Receiver:", address(l1Receiver));
        console.log("L2 Receiver:", address(l2Receiver));
        console.log("Using L1 fork:", l1ForkSuccess);
        console.log("Using L2 fork:", l2ForkSuccess);
    }

    /// @notice Setup L1 fork environment
    function setupL1Fork() internal {
        try vm.createFork(l1RpcUrl) returns (uint256 forkId) {
            l1Fork = forkId;
            l1ForkSuccess = true;
            vm.selectFork(l1Fork);
            console.log("L1 fork created with RPC:", l1RpcUrl);
            console.log("L1 fork ID:", l1Fork);
        } catch {
            console.log("Failed to create L1 fork, using local environment");
            l1ForkSuccess = false;
        }
    }

    /// @notice Setup L2 fork environment
    function setupL2Fork() internal {
        try vm.createFork(l2RpcUrl) returns (uint256 forkId) {
            l2Fork = forkId;
            l2ForkSuccess = true;
            vm.selectFork(l2Fork);
            console.log("L2 fork created with RPC:", l2RpcUrl);
            console.log("L2 fork ID:", l2Fork);
        } catch {
            console.log("Failed to create L2 fork, using local environment");
            l2ForkSuccess = false;
        }
    }

    /// @notice Setup L1 contracts (placeholder addresses for now)
    function setupL1Contracts() internal {
        // In a real fork test, these would be the actual deployed contract addresses
        // For now, we'll use placeholder addresses
        l1CrossDomainMessenger = IL1CrossDomainMessenger(address(vm.parseAddress("0x504a0094f9492894243a8ea14b33ed7eeb619e84")));

        // If we're on a real fork, we should use the actual deployed addresses
        // These would typically be retrieved from a deployment registry or config
    }

    /// @notice Setup L2 contracts using Predeploys
    function setupL2Contracts() internal {
        l2CrossDomainMessenger = IL2CrossDomainMessenger(Predeploys.L2_CROSS_DOMAIN_MESSENGER);
    }

    /// @notice Label addresses for better debugging
    function labelAddresses() internal {
        vm.label(testAlice, "TestAlice");
        vm.label(testBob, "TestBob");
        vm.label(address(l1CrossDomainMessenger), "L1CrossDomainMessenger");
        vm.label(address(l2CrossDomainMessenger), "L2CrossDomainMessenger");
        vm.label(address(l1Receiver), "L1Receiver");
        vm.label(address(l2Receiver), "L2Receiver");
    }

    /// @notice Test L1 to L2 message sending
    function test_l1_to_l2_sendMessage_succeeds() public {

        // Switch to L1 fork
        vm.selectFork(l1Fork);

        // Prepare message data
        bytes memory messageData = abi.encodeWithSignature(
            "receiveMessage(string)",
            TEST_MESSAGE
        );

        // Get initial nonce
        uint256 initialNonce = l1CrossDomainMessenger.messageNonce();

        // Send message from L1 to L2
        vm.prank(testAlice, testAlice);
        l1CrossDomainMessenger.sendMessage(
            address(l2Receiver),
            messageData,
            MIN_GAS_LIMIT
        );

        // Verify nonce increased
        uint256 finalNonce = l1CrossDomainMessenger.messageNonce();
        assertEq(finalNonce, initialNonce + 1, "Message nonce should increase");

        console.log("L1 to L2 message sent successfully");
        console.log("Initial nonce:", initialNonce);
        console.log("Final nonce:", finalNonce);
    }

    /// @notice Test L2 to L1 message sending
    function test_l2_to_l1_sendMessage_succeeds() public {

        vm.selectFork(l2Fork);

        // Prepare message data
        bytes memory messageData = abi.encodeWithSignature(
            "receiveMessage(string)",
            TEST_MESSAGE
        );

        // Get initial nonce
        uint256 initialNonce = l2CrossDomainMessenger.messageNonce();

        // Send message from L2 to L1
        vm.prank(testAlice, testAlice);
        l2CrossDomainMessenger.sendMessage(
            address(l1Receiver),
            messageData,
            MIN_GAS_LIMIT
        );

        // Verify nonce increased
        uint256 finalNonce = l2CrossDomainMessenger.messageNonce();
        assertEq(finalNonce, initialNonce + 1, "Message nonce should increase");

        console.log("L2 to L1 message sent successfully");
        console.log("Initial nonce:", initialNonce);
        console.log("Final nonce:", finalNonce);
    }

}
