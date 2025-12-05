// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import { Script } from "forge-std/Script.sol";
import { console } from "forge-std/console.sol";
import { SystemConfig } from "src/L1/SystemConfig.sol";

/// @title DeploySystemConfigImpl
/// @notice Simple script to deploy only the SystemConfig implementation contract
/// @dev This script only deploys the implementation contract without proxy or initialization.
///      Use this when you only need the implementation contract for upgrades or reference.
contract DeploySystemConfigImpl is Script {
    /// @notice The deployed SystemConfig implementation
    SystemConfig public systemConfigImpl;

    /// @notice Main deployment function
    function run() external {
        console.log("=== Deploying SystemConfig Implementation ===");
        console.log("Deployer address:", msg.sender);

        vm.startBroadcast();

        // Deploy SystemConfig implementation
        systemConfigImpl = new SystemConfig();

        vm.stopBroadcast();

        // Log deployment results
        console.log("\n=== Deployment Results ===");
        console.log("SystemConfig Implementation deployed at:", address(systemConfigImpl));
        console.log("SystemConfig version:", systemConfigImpl.version());

        // Verify the deployment
        _verifyDeployment();

        console.log("\n=== SystemConfig Implementation Deployment Completed ===");
    }

    /// @notice Verify the implementation deployment
    function _verifyDeployment() internal view {
        console.log("\n--- Verifying Implementation ---");

        // Check that the contract was deployed
        require(address(systemConfigImpl).code.length > 0, "SystemConfig implementation not deployed");

        // Check that we can call version (basic functionality test)
        string memory version = systemConfigImpl.version();
        require(bytes(version).length > 0, "SystemConfig version not accessible");
        require(keccak256(bytes(version)) == keccak256(bytes("3.12.0")), "SystemConfig version not updated correctly");
        console.log("SystemConfig version:", version);
        console.log("Implementation verification passed!");
        console.log("Contract size:", address(systemConfigImpl).code.length, "bytes");
        console.log("SystemConfig version:", version);
    }
}
