// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import { Test } from "forge-std/Test.sol";
import { console2 as console } from "forge-std/console2.sol";

import { GnosisSafe as Safe } from "safe-contracts/GnosisSafe.sol";
import { GnosisSafeProxyFactory as SafeProxyFactory } from "safe-contracts/proxies/GnosisSafeProxyFactory.sol";
import { Enum as SafeOps } from "safe-contracts/common/Enum.sol";

/// @title NestedSafePreApprovedHashTest
/// @notice Test case demonstrating nested Safe multisig using pre-approved hashes.
///         Scenario:
///         - Safe A (3/3): Owners are Alice, Safe B, and Carol
///         - Safe B (2/2): Owners are Bob1 and Bob2
///         - Goal: Safe A transfers 10 ETH using pre-approved hashes from all owners
///         - Optimization: Carol executes the transaction, so she doesn't need to pre-approve
///                         (saves 1 transaction and ~45k gas)
contract NestedSafePreApprovedHashTest is Test {
    // Safe contracts
    SafeProxyFactory safeProxyFactory;
    Safe safeSingleton;
    Safe safeA;
    Safe safeB;

    // Test accounts
    address alice;
    address bob1;
    address bob2;
    address carol;
    address recipient;

    // Constants
    uint256 constant TRANSFER_AMOUNT = 10 ether;
    uint256 constant INITIAL_SAFE_A_BALANCE = 20 ether;

    function setUp() public {
        // Create test accounts
        alice = makeAddr("alice");
        bob1 = makeAddr("bob1");
        bob2 = makeAddr("bob2");
        carol = makeAddr("carol");
        recipient = makeAddr("recipient");

        // Deploy Safe infrastructure
        safeSingleton = new Safe();
        safeProxyFactory = new SafeProxyFactory();

        // Deploy Safe B (2/2 multisig: Bob1, Bob2)
        safeB = _deploySafe(_createOwners(bob1, bob2), 2);
        console.log("Safe B deployed at:", address(safeB));

        // Deploy Safe A (3/3 multisig: Alice, Safe B, Carol)
        // Note: Addresses must be in ascending order
        safeA = _deploySafe(_createOwners(alice, address(safeB), carol), 3);
        console.log("Safe A deployed at:", address(safeA));

        // Fund Safe A with ETH
        vm.deal(address(safeA), INITIAL_SAFE_A_BALANCE);
        console.log("Safe A funded with:", INITIAL_SAFE_A_BALANCE / 1 ether, "ETH");
    }

    /// @notice Main test: Verify nested Safe with pre-approved hashes
    function test_nestedSafe_preApprovedHashes_succeeds() public {
        console.log("\n=== Starting Nested Safe Pre-Approved Hash Test ===\n");

        // Initial balances
        uint256 recipientBalanceBefore = recipient.balance;
        uint256 safeABalanceBefore = address(safeA).balance;

        console.log("Initial Safe A balance:", safeABalanceBefore / 1 ether, "ETH");
        console.log("Initial recipient balance:", recipientBalanceBefore / 1 ether, "ETH");

        // ====================================================================
        // PHASE 1: Calculate Safe A's transaction hash
        // ====================================================================
        console.log("\n--- Phase 1: Calculate Safe A Transaction Hash ---");

        bytes memory txData = "";
        bytes32 txHashA = safeA.getTransactionHash(
            recipient, // to
            TRANSFER_AMOUNT, // value
            txData, // data
            SafeOps.Operation.Call, // operation
            0, // safeTxGas
            0, // baseGas
            0, // gasPrice
            address(0), // gasToken
            payable(address(0)), // refundReceiver
            safeA.nonce() // _nonce
        );

        console.log("Safe A transaction hash:", vm.toString(txHashA));

        // ====================================================================
        // PHASE 2: Safe B approves Safe A's transaction hash
        // ====================================================================
        console.log("\n--- Phase 2: Safe B Approval Process ---");

        // Step 2.1: Calculate Safe B's transaction (calling safeA.approveHash)
        bytes memory safeBCallData = abi.encodeWithSignature("approveHash(bytes32)", txHashA);

        bytes32 txHashB = safeB.getTransactionHash(
            address(safeA), // to
            0, // value
            safeBCallData, // data
            SafeOps.Operation.Call, // operation
            0, // safeTxGas
            0, // baseGas
            0, // gasPrice
            address(0), // gasToken
            payable(address(0)), // refundReceiver
            safeB.nonce() // _nonce
        );

        console.log("Safe B transaction hash:", vm.toString(txHashB));

        // Step 2.2: Bob1 and Bob2 approve Safe B's transaction
        vm.prank(bob1);
        safeB.approveHash(txHashB);
        console.log("Bob1 approved Safe B transaction");

        vm.prank(bob2);
        safeB.approveHash(txHashB);
        console.log("Bob2 approved Safe B transaction");

        // Verify approvals
        assertEq(safeB.approvedHashes(bob1, txHashB), 1, "Bob1 approval not recorded");
        assertEq(safeB.approvedHashes(bob2, txHashB), 1, "Bob2 approval not recorded");

        // Step 2.3: Execute Safe B's transaction to approve Safe A's hash
        bytes memory safeBSignatures = _buildPreApprovedSignatures(_createOwners(bob1, bob2));

        vm.prank(bob1); // Bob1 executes
        bool successB = safeB.execTransaction({
            to: address(safeA),
            value: 0,
            data: safeBCallData,
            operation: SafeOps.Operation.Call,
            safeTxGas: 0,
            baseGas: 0,
            gasPrice: 0,
            gasToken: address(0),
            refundReceiver: payable(address(0)),
            signatures: safeBSignatures
        });

        assertTrue(successB, "Safe B transaction failed");
        console.log("Safe B executed transaction: safeA.approveHash(txHashA)");

        // Verify Safe B's approval was recorded in Safe A
        assertEq(safeA.approvedHashes(address(safeB), txHashA), 1, "Safe B approval not recorded in Safe A");
        console.log("Safe A recorded Safe B's approval");

        // ====================================================================
        // PHASE 3: Alice approves Safe A's transaction
        // ====================================================================
        console.log("\n--- Phase 3: Alice Approves Safe A Transaction ---");

        vm.prank(alice);
        safeA.approveHash(txHashA);
        console.log("Alice approved Safe A transaction");

        // Note: Carol does NOT need to call approveHash because she will be the executor
        // The check: msg.sender == currentOwner will pass automatically
        console.log("Carol will execute (no pre-approval needed)");

        // Verify approvals
        assertEq(safeA.approvedHashes(alice, txHashA), 1, "Alice approval not recorded");

        // ====================================================================
        // PHASE 4: Execute Safe A's transaction
        // ====================================================================
        console.log("\n--- Phase 4: Carol Executes Safe A Transaction ---");

        // Build signatures for Safe A (all using pre-approved, v=1)
        // Addresses must be in ascending order: Alice < Safe B < Carol
        bytes memory safeASignatures = _buildPreApprovedSignatures(_createOwners(alice, address(safeB), carol));

        vm.prank(carol); // Carol executes (no pre-approval needed!)
        bool successA = safeA.execTransaction({
            to: recipient,
            value: TRANSFER_AMOUNT,
            data: txData,
            operation: SafeOps.Operation.Call,
            safeTxGas: 0,
            baseGas: 0,
            gasPrice: 0,
            gasToken: address(0),
            refundReceiver: payable(address(0)),
            signatures: safeASignatures
        });

        assertTrue(successA, "Safe A transaction failed");
        console.log("Safe A transaction executed successfully!");

        // ====================================================================
        // VERIFY FINAL STATE
        // ====================================================================
        console.log("\n--- Final State Verification ---");

        uint256 recipientBalanceAfter = recipient.balance;
        uint256 safeABalanceAfter = address(safeA).balance;

        assertEq(recipientBalanceAfter, recipientBalanceBefore + TRANSFER_AMOUNT, "Recipient did not receive funds");
        assertEq(safeABalanceAfter, safeABalanceBefore - TRANSFER_AMOUNT, "Safe A balance incorrect");

        console.log("Final Safe A balance:", safeABalanceAfter / 1 ether, "ETH");
        console.log("Final recipient balance:", recipientBalanceAfter / 1 ether, "ETH");
        console.log("Transfer amount:", TRANSFER_AMOUNT / 1 ether, "ETH");

        console.log("\n=== Test Passed! All owners used pre-approved hashes ===");
    }

    /// @notice Test that transaction fails without sufficient approvals
    function test_nestedSafe_insufficientApprovals_reverts() public {
        // Calculate Safe A's transaction hash
        bytes32 txHashA = safeA.getTransactionHash(
            recipient, // to
            TRANSFER_AMOUNT, // value
            "", // data
            SafeOps.Operation.Call, // operation
            0, // safeTxGas
            0, // baseGas
            0, // gasPrice
            address(0), // gasToken
            payable(address(0)), // refundReceiver
            safeA.nonce() // _nonce
        );

        // Only Alice approves (not enough for 3/3)
        vm.prank(alice);
        safeA.approveHash(txHashA);

        // Try to execute without sufficient approvals
        bytes memory signatures = _buildPreApprovedSignatures(_createOwners(alice, address(safeB), carol));

        vm.prank(alice);
        vm.expectRevert(); // Should fail due to insufficient approvals
        safeA.execTransaction({
            to: recipient,
            value: TRANSFER_AMOUNT,
            data: "",
            operation: SafeOps.Operation.Call,
            safeTxGas: 0,
            baseGas: 0,
            gasPrice: 0,
            gasToken: address(0),
            refundReceiver: payable(address(0)),
            signatures: signatures
        });
    }

    /// @notice Test that signatures must be in correct order
    function test_nestedSafe_wrongOrder_reverts() public {
        bytes32 txHashA = safeA.getTransactionHash(
            recipient, // to
            TRANSFER_AMOUNT, // value
            "", // data
            SafeOps.Operation.Call, // operation
            0, // safeTxGas
            0, // baseGas
            0, // gasPrice
            address(0), // gasToken
            payable(address(0)), // refundReceiver
            safeA.nonce() // _nonce
        );

        // All approve
        vm.prank(alice);
        safeA.approveHash(txHashA);
        vm.prank(carol);
        safeA.approveHash(txHashA);

        // Build signatures in WRONG order (Carol before Safe B)
        bytes memory wrongOrderSignatures = abi.encodePacked(
            bytes32(uint256(uint160(alice))),
            bytes32(0),
            uint8(1),
            bytes32(uint256(uint160(carol))), // Wrong: Carol before Safe B
            bytes32(0),
            uint8(1),
            bytes32(uint256(uint160(address(safeB)))),
            bytes32(0),
            uint8(1)
        );

        vm.prank(alice);
        vm.expectRevert(); // Should fail due to wrong order
        safeA.execTransaction({
            to: recipient,
            value: TRANSFER_AMOUNT,
            data: "",
            operation: SafeOps.Operation.Call,
            safeTxGas: 0,
            baseGas: 0,
            gasPrice: 0,
            gasToken: address(0),
            refundReceiver: payable(address(0)),
            signatures: wrongOrderSignatures
        });
    }

    // ====================================================================
    // HELPER FUNCTIONS
    // ====================================================================

    /// @notice Deploy a new Safe with given owners and threshold
    function _deploySafe(address[] memory _owners, uint256 _threshold) internal returns (Safe) {
        bytes memory initData = abi.encodeCall(
            Safe.setup, (_owners, _threshold, address(0), hex"", address(0), address(0), 0, payable(address(0)))
        );

        Safe safe = Safe(
            payable(
                address(
                    safeProxyFactory.createProxyWithNonce(
                        address(safeSingleton), initData, uint256(keccak256(abi.encodePacked(_owners, _threshold)))
                    )
                )
            )
        );

        return safe;
    }

    /// @notice Create an array of owners (helper for testing)
    function _createOwners(address owner1, address owner2) internal pure returns (address[] memory) {
        address[] memory owners = new address[](2);
        // Sort addresses in ascending order
        if (owner1 < owner2) {
            owners[0] = owner1;
            owners[1] = owner2;
        } else {
            owners[0] = owner2;
            owners[1] = owner1;
        }
        return owners;
    }

    /// @notice Create an array of owners (helper for testing)
    function _createOwners(address owner1, address owner2, address owner3) internal pure returns (address[] memory) {
        address[] memory owners = new address[](3);
        owners[0] = owner1;
        owners[1] = owner2;
        owners[2] = owner3;

        // Bubble sort to ensure ascending order
        for (uint256 i = 0; i < 3; i++) {
            for (uint256 j = i + 1; j < 3; j++) {
                if (owners[i] > owners[j]) {
                    address temp = owners[i];
                    owners[i] = owners[j];
                    owners[j] = temp;
                }
            }
        }

        return owners;
    }

    /// @notice Build pre-approved signatures (v=1) for given owners
    /// @dev Each signature is 65 bytes: r=ownerAddress, s=0, v=1
    function _buildPreApprovedSignatures(address[] memory _owners) internal pure returns (bytes memory) {
        bytes memory signatures;
        for (uint256 i = 0; i < _owners.length; i++) {
            signatures = abi.encodePacked(
                signatures,
                bytes32(uint256(uint160(_owners[i]))), // r = owner address
                bytes32(0), // s = 0
                uint8(1) // v = 1 (pre-approved hash)
            );
        }
        return signatures;
    }

    /// @notice Test helper to print Safe state
    function _printSafeState(Safe _safe, string memory _name) internal view {
        console.log("\n--- %s State ---", _name);
        console.log("Address:", address(_safe));
        console.log("Threshold:", _safe.getThreshold());
        console.log("Nonce:", _safe.nonce());
        console.log("Balance:", address(_safe).balance / 1 ether, "ETH");
    }
}
