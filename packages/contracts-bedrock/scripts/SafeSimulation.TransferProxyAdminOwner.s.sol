// SPDX-License-Identifier: MIT
pragma solidity 0.8.15;

import { Script } from "forge-std/Script.sol";
import { console2 as console } from "forge-std/console2.sol";
import { GnosisSafe } from "safe-contracts/GnosisSafe.sol";
import { Enum } from "safe-contracts/common/Enum.sol";
import { Ownable } from "@openzeppelin/contracts/access/Ownable.sol";

/// @title SafeSimulation_TransferProxyAdminOwner
/// @notice Script to simulate transferring ProxyAdmin ownership via Safe multisig.
///         This script demonstrates the full flow of a multisig ownership transfer:
///         1. Prepare the transferOwnership transaction
///         2. Owner 1 approves the transaction hash
///         3. Owner 2 executes the transaction with combined signatures
/// @dev Usage:
///      export PROXY_ADMIN=
///      export RPC_URL=
///      forge script scripts/SafeSimulation.TransferProxyAdminOwner.s.sol \
///        --fork-url $RPC_URL \
///        -vvvv
contract SafeSimulation_TransferProxyAdminOwner is Script {
    /// @notice Safe multisig address
    address constant SAFE = 0xC290bE56089BCC83c6993583ce2cF51a7951D45A;

    /// @notice Safe owner addresses
    address constant OWNER1 = 0x6eE7BDa7AF04F61ccf93aB4b8DB2289aBe76C6aA;
    address constant OWNER2 = 0xD3C6821DE67A5c0345EC5A41e4C83739f7043972;

    /// @notice New owner address for the ProxyAdmin
    address constant NEW_OWNER = OWNER1;

    function run() external {
        console.log("Simulating Transfer ProxyAdmin Owner from Safe multisig to Safe Owner 1");
        // Read ProxyAdmin address from environment
        address proxyAdmin = vm.envAddress("PROXY_ADMIN");
        console.log("ProxyAdmin address:", proxyAdmin);
        console.log("Current ProxyAdmin owner:", Ownable(proxyAdmin).owner());

        // 1. Prepare transferOwnership transaction data
        bytes memory data = abi.encodeWithSelector(Ownable.transferOwnership.selector, NEW_OWNER);

        GnosisSafe safe = GnosisSafe(payable(SAFE));
        uint256 nonce = safe.nonce();

        // Get the transaction hash from the Safe
        // getTransactionHash(address to, uint256 value, bytes data, Enum.Operation operation,
        // uint256 safeTxGas, uint256 baseGas, uint256 gasPrice, address gasToken,
        // address refundReceiver, uint256 _nonce)
        bytes32 txHash = safe.getTransactionHash(
            proxyAdmin,
            0, // value
            data,
            Enum.Operation.Call,
            0, // safeTxGas
            0, // baseGas
            0, // gasPrice
            address(0), // gasToken
            address(0), // refundReceiver
            nonce
        );

        console.log("Safe Transaction Hash:");
        console.logBytes32(txHash);

        // 2. Owner 1 approves hash
        console.log("Impersonating Owner 1:", OWNER1);
        vm.startPrank(OWNER1);
        safe.approveHash(txHash);
        vm.stopPrank();
        console.log("Hash approved by Owner 1");

        // 3. Owner 2 executes transaction
        // Prepare signatures
        // Signatures must be sorted by owner address.
        // OWNER1 < OWNER2 check:
        // 0x6eE7... < 0xD3C6... -> True.

        // Signature format for approved hash:
        // r = owner address (padded to 32 bytes)
        // s = 0 (padded to 32 bytes)
        // v = 1 (1 byte)

        bytes memory sig1 = abi.encodePacked(bytes32(uint256(uint160(OWNER1))), bytes32(0), uint8(1));

        // For Owner 2 (msg.sender), we can also use the same format (v=1, r=owner)
        // because GnosisSafe checkNSignatures allows v=1 if msg.sender == owner
        bytes memory sig2 = abi.encodePacked(bytes32(uint256(uint160(OWNER2))), bytes32(0), uint8(1));

        bytes memory signatures = abi.encodePacked(sig1, sig2);

        console.log("Impersonating Owner 2:", OWNER2);
        vm.startPrank(OWNER2);

        bool success = safe.execTransaction(
            proxyAdmin,
            0, // value
            data,
            Enum.Operation.Call,
            0, // safeTxGas
            0, // baseGas
            0, // gasPrice
            address(0), // gasToken
            payable(address(0)), // refundReceiver
            signatures
        );

        vm.stopPrank();

        if (success) {
            console.log("Transaction executed successfully");
        } else {
            console.log("Transaction failed");
        }

        // Verify the ownership transfer
        address newOwner = Ownable(proxyAdmin).owner();
        console.log("New ProxyAdmin owner:", newOwner);
        require(newOwner == NEW_OWNER, "Owner not updated");
        console.log("ProxyAdmin ownership successfully transferred!");
    }
}

