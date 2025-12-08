// SPDX-License-Identifier: MIT
pragma solidity 0.8.15;

import { Script } from "forge-std/Script.sol";
import { console2 as console } from "forge-std/console2.sol";
import { Ownable } from "@openzeppelin/contracts/access/Ownable.sol";

/// @notice Interface for the Transactor contract
interface ITransactor {
    function CALL(
        address _target,
        bytes memory _data,
        uint256 _value
    )
        external
        payable
        returns (bool success_, bytes memory data_);

    function owner() external view returns (address);
}

/// @title TransferProxyAdminOwnership
/// @notice Fork simulation script to transfer ProxyAdmin ownership from Transactor to Safe.
///         This script simulates the complete flow:
///         1. Impersonate the Transactor owner (EOA) using vm.prank
///         2. Call Transactor.CALL() to execute ProxyAdmin.transferOwnership(Safe)
///         3. Verify ownership transfer was successful
/// @dev Usage:
///      export PROXY_ADMIN=<ProxyAdmin address>
///      export TRANSACTOR=<Transactor address>
///      export NEW_OWNER=<New owner address (Safe)>
///      export RPC_URL=<Network RPC URL>
///      forge script scripts/TransferProxyAdminOwnership.s.sol \
///        --fork-url $RPC_URL \
///        -vvvv
contract TransferProxyAdminOwnership is Script {
    function run() external {
        console.log("=== Transfer ProxyAdmin Ownership Simulation ===");
        console.log("");

        // Read addresses from environment
        address proxyAdmin = vm.envAddress("PROXY_ADMIN");
        address transactor = vm.envAddress("TRANSACTOR");
        address newOwner = vm.envAddress("NEW_OWNER");

        console.log("Addresses:");
        console.log("  ProxyAdmin:", proxyAdmin);
        console.log("  Transactor:", transactor);
        console.log("  New Owner (Safe):", newOwner);
        console.log("");

        // Verify current state
        console.log("Current State:");
        address currentProxyAdminOwner = Ownable(proxyAdmin).owner();
        address transactorOwner = ITransactor(transactor).owner();
        console.log("  ProxyAdmin owner:", currentProxyAdminOwner);
        console.log("  Transactor owner (EOA):", transactorOwner);
        console.log("");

        // Verify that ProxyAdmin is owned by Transactor
        require(
            currentProxyAdminOwner == transactor,
            "ProxyAdmin is not owned by Transactor"
        );

        console.log("Pre-conditions verified");
        console.log("");

        // Prepare the call data: ProxyAdmin.transferOwnership(newOwner)
        bytes memory proxyAdminCallData = abi.encodeWithSelector(
            Ownable.transferOwnership.selector,
            newOwner
        );

        console.log("Transaction Details:");
        console.log("  Caller: Transactor Owner (EOA)");
        console.log("  Target: Transactor.CALL()");
        console.log("  Inner Target: ProxyAdmin");
        console.log("  Inner Function: transferOwnership(address)");
        console.log("  New Owner:", newOwner);
        console.log("");

        // Execute the transaction by impersonating the Transactor owner
        console.log("Executing transaction...");
        console.log("  Impersonating:", transactorOwner);

        vm.startPrank(transactorOwner);

        (bool success, bytes memory returnData) = ITransactor(transactor).CALL(
            proxyAdmin,
            proxyAdminCallData,
            0 // value
        );

        vm.stopPrank();

        require(success, "Transactor.CALL failed");
        console.log("Transaction executed successfully");
        console.log("");

        // Verify the ownership transfer
        console.log("=== Verification ===");
        address newProxyAdminOwner = Ownable(proxyAdmin).owner();
        console.log("New ProxyAdmin owner:", newProxyAdminOwner);
        console.log("Expected owner:", newOwner);

        require(
            newProxyAdminOwner == newOwner,
            "ProxyAdmin ownership transfer failed"
        );

        console.log("");
        console.log("ProxyAdmin ownership successfully transferred!");
        console.log("");
        console.log("Summary:");
        console.log("  ProxyAdmin:", proxyAdmin);
        console.log("  Old Owner: Transactor (", transactor, ")");
        console.log("  New Owner: Safe (", newOwner, ")");
        console.log("  Executed by:", transactorOwner);
    }
}

