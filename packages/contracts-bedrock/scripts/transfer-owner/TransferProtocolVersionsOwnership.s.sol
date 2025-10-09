// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import { Script } from "forge-std/Script.sol";
import { console2 as console } from "forge-std/console2.sol";

import { IProtocolVersions } from "interfaces/L1/IProtocolVersions.sol";
import { OwnableUpgradeable } from "@openzeppelin/contracts-upgradeable/access/OwnableUpgradeable.sol";

/// @title TransferProtocolVersionsOwnership
/// @notice Foundry script to transfer ownership of the ProtocolVersions contract.
///         This script performs the following:
///         1. Validates the current owner matches the deployer
///         2. Transfers ownership to the specified new owner
///         3. Verifies the transfer was successful
///
/// @dev Usage:
///      PROTOCOL_VERSIONS=<contract_address> \
///      NEW_OWNER=<new_owner_address> \
///      forge script scripts/transfer-owner/TransferProtocolVersionsOwnership.s.sol:TransferProtocolVersionsOwnership \
///        --rpc-url $RPC_URL \
///        --private-key $PRIVATE_KEY \
///        --broadcast
contract TransferProtocolVersionsOwnership is Script {
    /// @notice Modifier that wraps a function in broadcasting.
    modifier broadcast() {
        vm.startBroadcast(msg.sender);
        _;
        vm.stopBroadcast();
    }

    /// @notice Main execution function.
    function run() public {
        console.log("====================================================");
        console.log("ProtocolVersions Ownership Transfer");
        console.log("====================================================\n");

        // Read addresses from environment variables
        address protocolVersionsAddr = vm.envAddress("PROTOCOL_VERSIONS");
        address newOwner = vm.envAddress("NEW_OWNER");

        console.log("Configuration:");
        console.log("  ProtocolVersions:  %s", protocolVersionsAddr);
        console.log("  New Owner:         %s", newOwner);
        console.log("  Deployer:          %s", msg.sender);
        console.log();

        // Transfer ownership
        _transferOwnership(protocolVersionsAddr, newOwner);

        console.log("====================================================");
        console.log("ProtocolVersions Ownership Transfer Complete");
        console.log("====================================================\n");
    }

    /// @notice Transfer ownership of ProtocolVersions contract.
    /// @param _protocolVersionsAddr Address of the ProtocolVersions contract.
    /// @param _newOwner Address of the new owner.
    function _transferOwnership(address _protocolVersionsAddr, address _newOwner) internal broadcast {
        console.log("Step 1: Validating current ownership...");

        OwnableUpgradeable protocolVersions = OwnableUpgradeable(_protocolVersionsAddr);

        // Get current owner
        address currentOwner = protocolVersions.owner();
        console.log("  Current owner:     %s", currentOwner);
        console.log("  Deployer address:  %s", msg.sender);

        // Validate the deployer is the current owner
        require(currentOwner == msg.sender, "TransferProtocolVersionsOwnership: deployer is not current owner");

        // Ensure we're not transferring to the same address
        require(currentOwner != _newOwner, "TransferProtocolVersionsOwnership: new owner is already the owner");

        // Validate new owner is not zero address
        require(_newOwner != address(0), "TransferProtocolVersionsOwnership: new owner cannot be zero address");

        console.log("  Current owner validation passed");
        console.log();

        console.log("Step 2: Transferring ownership...");
        console.log("  From: %s", currentOwner);
        console.log("  To:   %s", _newOwner);

        // Transfer ownership
        protocolVersions.transferOwnership(_newOwner);

        console.log("  transferOwnership() called successfully");
        console.log();

        console.log("Step 3: Verifying ownership transfer...");

        // Verify the transfer was successful
        address actualNewOwner = protocolVersions.owner();
        console.log("  Actual new owner:  %s", actualNewOwner);
        console.log("  Expected owner:    %s", _newOwner);

        require(
            actualNewOwner == _newOwner, "TransferProtocolVersionsOwnership: ownership transfer verification failed"
        );

        console.log("  Ownership transfer verified successfully");
        console.log();
        console.log("SUCCESS: ProtocolVersions ownership transferred from %s to %s", currentOwner, _newOwner);
    }

    /// @notice Helper function to check current owner without broadcasting.
    /// @param _protocolVersionsAddr Address of the ProtocolVersions contract.
    /// @return Current owner address.
    function checkOwner(address _protocolVersionsAddr) public view returns (address) {
        OwnableUpgradeable protocolVersions = OwnableUpgradeable(_protocolVersionsAddr);
        return protocolVersions.owner();
    }
}
