// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import { Script } from "forge-std/Script.sol";
import { console } from "forge-std/console.sol";
import { SystemConfig } from "src/L1/SystemConfig.sol";
import { IProxyAdmin } from "interfaces/universal/IProxyAdmin.sol";
import { Transactor } from "src/periphery/Transactor.sol";
import { ISystemConfig } from "interfaces/L1/ISystemConfig.sol";

/// @title UpgradeSystemConfigLocal
/// @notice Custom script to upgrade SystemConfig to V3.12, deployer is transactor owner
/// @dev This script:
///      1. Reads environment variables
///      2. Validates ownership chain
///      3. Deploys SystemConfig implementation
///      4. Upgrades SystemConfig to V3.12 via ProxyAdmin.upgrade()
///      5. Verifies upgrade
contract UpgradeSystemConfigLocal is Script {
    // Environment variable names
    string constant SYSTEM_CONFIG_PROXY = "SYSTEM_CONFIG_PROXY_ADDRESS";
    string constant PROXY_ADMIN = "OP_PROXY_ADMIN";
    string constant TRANSACTOR = "TRANSACTOR";

    // State variables for configuration
    address systemConfigProxy;
    address proxyAdmin;
    address transactorAddress;
    address deployerAddress;

    // Deployed contracts
    Transactor transactor;

    /// @notice Main upgrade function
    function run() external {
        _loadConfiguration();
        _validateConfiguration();
        _performUpgrade();
    }

    /// @notice Load configuration from environment variables
    function _loadConfiguration() internal {
        // Get deployer address from msg.sender (set by forge script --private-key)
        deployerAddress = msg.sender;

        // Load addresses from environment
        systemConfigProxy = vm.envAddress(SYSTEM_CONFIG_PROXY);
        proxyAdmin = vm.envAddress(PROXY_ADMIN);
        transactorAddress = vm.envAddress(TRANSACTOR);

        console.log("=== Upgrade Configuration ===");
        console.log("Deployer address:", deployerAddress);
        console.log("SystemConfig Proxy:", systemConfigProxy);
        console.log("ProxyAdmin:", proxyAdmin);
        console.log("Transactor:", transactorAddress);

        // Initialize contract interfaces
        transactor = Transactor(transactorAddress);
    }

    /// @notice Validate the configuration before proceeding
    function _validateConfiguration() internal view {
        require(systemConfigProxy != address(0), "SystemConfig proxy address cannot be zero");
        require(proxyAdmin != address(0), "ProxyAdmin address cannot be zero");
        require(transactorAddress != address(0), "Transactor address cannot be zero");
        require(deployerAddress != address(0), "Deployer address cannot be zero");

        // Verify contracts have code
        require(systemConfigProxy.code.length > 0, "SystemConfig proxy must have code (not an EOA)");
        require(proxyAdmin.code.length > 0, "ProxyAdmin must have code (not an EOA)");
        require(transactorAddress.code.length > 0, "Transactor must have code (not an EOA)");

        // Verify ownership chain for upgrade permissions
        _validateOwnershipChain();
    }

    /// @notice Validate the ownership chain for upgrade permissions
    function _validateOwnershipChain() internal view {
        console.log("\n--- Validating Ownership Chain ---");

        // Check ProxyAdmin owner
        IProxyAdmin admin = IProxyAdmin(proxyAdmin);
        address proxyAdminOwner = admin.owner();
        console.log("ProxyAdmin owner:", proxyAdminOwner);
        console.log("Transactor address:", transactorAddress);

        // Verify Transactor owns ProxyAdmin
        require(proxyAdminOwner == transactorAddress, "Transactor must be the owner of ProxyAdmin");

        // Check SystemConfig proxy admin
        ISystemConfig systemConfigContract = ISystemConfig(systemConfigProxy);
        address systemConfigAdmin = address(systemConfigContract.proxyAdmin());
        console.log("SystemConfig proxy admin:", systemConfigAdmin);
        console.log("Expected ProxyAdmin:", proxyAdmin);

        // Verify ProxyAdmin controls SystemConfig proxy
        require(systemConfigAdmin == proxyAdmin, "ProxyAdmin must be the admin of SystemConfig proxy");

        console.log("Ownership chain validated successfully");
    }

    /// @notice Perform the complete upgrade process
    function _performUpgrade() internal {
        console.log("\n=== Starting SystemConfig V3.12 Upgrade ===");

        // Step 1: Check current state
        _logCurrentState();

        vm.startBroadcast();

        // Step 2: Deploy new SystemConfig implementation
        console.log("\n--- Deploying SystemConfig Implementation ---");
        SystemConfig newImplementation = new SystemConfig();
        console.log("SystemConfig deployed at:", address(newImplementation));
        console.log("New implementation version:", newImplementation.version());

        // Step 3: Upgrade proxy via Transactor
        console.log("\n--- Upgrading SystemConfig V3.12 via Transactor ---");

        // Encode the ProxyAdmin.upgrade() call
        bytes memory upgradeData =
            abi.encodeWithSelector(IProxyAdmin.upgrade.selector, payable(systemConfigProxy), address(newImplementation));

        console.log("Calling ProxyAdmin.upgrade() via Transactor.CALL()");

        // Call ProxyAdmin.upgrade() through Transactor.CALL()
        console.log("Transactor owner:", transactor.owner());
        (bool success,) = transactor.CALL(
            proxyAdmin,
            upgradeData,
            0 // no ETH value
        );
        require(success, "Transactor CALL to ProxyAdmin.upgrade failed");
        console.log("Upgrade completed successfully");
        console.log("transactor.Call() params:");
        console.log("target: ", proxyAdmin);
        console.log("data:");
        console.logBytes(upgradeData);
        console.log("value: 0");

        SystemConfig upgradedSystemConfig = SystemConfig(systemConfigProxy);

        vm.stopBroadcast();

        // Step 4: Verify the upgrade
        _verifyUpgrade(upgradedSystemConfig);

        console.log("\n=== SystemConfig V3.12 Upgrade Completed Successfully ===");
    }

    /// @notice Log the current state before upgrade
    function _logCurrentState() internal view {
        ISystemConfig currentSystemConfig = ISystemConfig(systemConfigProxy);

        console.log("\n--- Current SystemConfig State ---");
        console.log("Current version:", currentSystemConfig.version());
        console.log("Current init version:", currentSystemConfig.initVersion());

        (address currentGasToken, uint8 currentDecimals) = currentSystemConfig.gasPayingToken();
        console.log("Current gas token:", currentGasToken);
        console.log("Current gas token decimals:", currentDecimals);
    }

    /// @notice Verify the upgrade was successful
    function _verifyUpgrade(SystemConfig upgradedSystemConfig) internal view {
        console.log("\n--- Verifying Upgrade Results ---");

        // Check version updated
        string memory newVersion = upgradedSystemConfig.version();
        console.log("New version:", newVersion);
        require(keccak256(bytes(newVersion)) == keccak256(bytes("3.12.0")), "Version not updated correctly");

        console.log("All upgrade verifications passed!");
    }
}
