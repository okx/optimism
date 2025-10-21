// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import { Script } from "forge-std/Script.sol";
import { console } from "forge-std/console.sol";
import { SystemConfig } from "src/L1/SystemConfig.sol";
import { SystemConfigV4 } from "scripts/SystemConfigV4.sol";
import { IProxyAdmin } from "interfaces/universal/IProxyAdmin.sol";
import { IOKB } from "interfaces/L1/IOKB.sol";
import { DepositedOKBAdapter } from "src/L1/DepositedOKBAdapter.sol";

/// @title UpgradeSystemConfigToV4
/// @notice Custom script to upgrade SystemConfig to V4 and set OKB adapter as gas paying token
/// @dev This script:
///      1. Reads OKB token address from environment variable
///      2. Deploys DepositedOKBAdapter that handles OKB burning internally
///      3. Adds deployer address to whitelist for deposits
///      4. Upgrades SystemConfig to V4
///      5. Sets OKB adapter as gas paying token in SystemConfig storage
///      6. Verifies all configurations on L1
contract UpgradeSystemConfigToV4 is Script {

    // Environment variable names
    string constant SYSTEM_CONFIG_PROXY = "SYSTEM_CONFIG_PROXY_ADDRESS";
    string constant PROXY_ADMIN = "PROXY_ADMIN";
    string constant OKB_TOKEN_ADDRESS = "OKB_TOKEN_ADDRESS";
    string constant OPTIMISM_PORTAL_PROXY = "OPTIMISM_PORTAL_PROXY_ADDRESS";

    // State variables for configuration
    address systemConfigProxy;
    address proxyAdmin;
    address okbTokenAddress;
    address optimismPortalProxy;
    address deployerAddress;

    // Deployed contracts
    IOKB okbToken;
    DepositedOKBAdapter adapter;

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
        okbTokenAddress = vm.envAddress(OKB_TOKEN_ADDRESS);
        optimismPortalProxy = vm.envAddress(OPTIMISM_PORTAL_PROXY);

        console.log("=== Upgrade Configuration ===");
        console.log("Deployer address:", deployerAddress);
        console.log("SystemConfig Proxy:", systemConfigProxy);
        console.log("ProxyAdmin:", proxyAdmin);
        console.log("OKB Token Address:", okbTokenAddress);
        console.log("OptimismPortal Proxy:", optimismPortalProxy);

        // Initialize OKB token interface
        okbToken = IOKB(okbTokenAddress);
    }

    /// @notice Validate the configuration before proceeding
    function _validateConfiguration() internal view {
        require(systemConfigProxy != address(0), "SystemConfig proxy address cannot be zero");
        require(proxyAdmin != address(0), "ProxyAdmin address cannot be zero");
        require(okbTokenAddress != address(0), "OKB token address cannot be zero");
        require(optimismPortalProxy != address(0), "OptimismPortal proxy address cannot be zero");
        require(deployerAddress != address(0), "Deployer address cannot be zero");

        // Verify contracts have code
        require(
            systemConfigProxy.code.length > 0,
            "SystemConfig proxy must have code (not an EOA)"
        );
        require(
            proxyAdmin.code.length > 0,
            "ProxyAdmin must have code (not an EOA)"
        );
        require(
            okbTokenAddress.code.length > 0,
            "OKB token must have code (not an EOA)"
        );
        require(
            optimismPortalProxy.code.length > 0,
            "OptimismPortal proxy must have code (not an EOA)"
        );
    }

    /// @notice Deploy DepositedOKBAdapter
    function _deployAdapter() internal {
        console.log("\n--- Deploying DepositedOKBAdapter ---");
        adapter = new DepositedOKBAdapter(okbTokenAddress, payable(optimismPortalProxy), deployerAddress);
        console.log("DepositedOKBAdapter deployed at:", address(adapter));
    }

    /// @notice Set up whitelist for authorized depositors
    function _setupWhitelist() internal {
        console.log("\n--- Setting up Whitelist ---");
        console.log("Adding deployer to whitelist...");
        address[] memory addresses = new address[](1);
        addresses[0] = deployerAddress;
        adapter.addToWhitelistBatch(addresses);
        console.log("Deployer whitelisted successfully:", deployerAddress);
    }

    /// @notice Perform the complete upgrade process
    function _performUpgrade() internal {
        console.log("\n=== Starting SystemConfig V4 Upgrade ===");

        // Step 1: Check current state
        _logCurrentState();

        // Step 2: Deploy new SystemConfigV4 implementation
        console.log("\n--- Deploying SystemConfigV4 Implementation ---");
        SystemConfigV4 newImplementation = new SystemConfigV4();
        console.log("SystemConfigV4 deployed at:", address(newImplementation));
        console.log("New implementation version:", newImplementation.version());
        console.log("New init version:", newImplementation.initVersion());

        vm.startBroadcast();

        // Step 3: Deploy DepositedOKBAdapter
        _deployAdapter();

        // Step 4: Setup whitelist for deployer
        _setupWhitelist();

        // Step 5: Upgrade the proxy to point to new implementation
        console.log("\n--- Upgrading Proxy Implementation ---");
        IProxyAdmin admin = IProxyAdmin(proxyAdmin);
        admin.upgrade(payable(systemConfigProxy), address(newImplementation));
        console.log("Proxy upgraded successfully");

        // Step 6: Call the upgrade function to set OKB adapter as gas paying token
        console.log("\n--- Setting OKB Adapter as Gas Paying Token ---");
        SystemConfigV4 upgradedSystemConfig = SystemConfigV4(systemConfigProxy);

        // Convert string to bytes32 for SystemConfig function
        bytes32 nameBytes32 = bytes32(bytes(okbToken.name()));
        bytes32 symbolBytes32 = bytes32(bytes(okbToken.symbol()));

        try upgradedSystemConfig.upgradeAndSetGasPayingToken(address(adapter), okbToken.decimals(), nameBytes32, symbolBytes32) {
            console.log("upgradeAndSetGasPayingToken() called successfully");
        } catch Error(string memory reason) {
            console.log("upgradeAndSetGasPayingToken() failed:", reason);
            revert("Failed to call upgradeAndSetGasPayingToken");
        }

        // Step 7: Verify the upgrade
        _verifyUpgrade(upgradedSystemConfig);

        vm.stopBroadcast();

        console.log("\n=== SystemConfig V4 Upgrade Completed Successfully ===");
    }

    /// @notice Log the current state before upgrade
    function _logCurrentState() internal view {
        SystemConfig currentSystemConfig = SystemConfig(systemConfigProxy);

        console.log("\n--- Current SystemConfig State ---");
        console.log("Current version:", currentSystemConfig.version());
        console.log("Current init version:", currentSystemConfig.initVersion());

        (address currentGasToken, uint8 currentDecimals) = currentSystemConfig.gasPayingToken();
        console.log("Current gas token:", currentGasToken);
        console.log("Current gas token decimals:", currentDecimals);

        string memory tokenName = currentSystemConfig.gasPayingTokenName();
        string memory tokenSymbol = currentSystemConfig.gasPayingTokenSymbol();
        console.log("Current gas token name:", tokenName);
        console.log("Current gas token symbol:", tokenSymbol);

        bool isCustomGasToken = currentSystemConfig.isCustomGasToken();
        console.log("Is custom gas token:", isCustomGasToken);
    }

    /// @notice Verify the upgrade was successful
    function _verifyUpgrade(SystemConfigV4 upgradedSystemConfig) internal view {
        console.log("\n--- Verifying Upgrade Results ---");

        // Check version updated
        string memory newVersion = upgradedSystemConfig.version();
        require(
            keccak256(bytes(newVersion)) == keccak256(bytes("3.12.0")),
            "Version not updated correctly"
        );

        // Check init version updated
        uint8 newInitVersion = upgradedSystemConfig.initVersion();
        require(newInitVersion == 4, "Init version not updated correctly");

        // Check gas paying token is set to adapter
        (address newGasToken, uint8 newDecimals) = upgradedSystemConfig.gasPayingToken();
        console.log("New gas token:", newGasToken);
        console.log("New gas token decimals:", newDecimals);
        require(newGasToken == address(adapter), "FAILED: Token address should be adapter");
        require(newDecimals == okbToken.decimals(), "FAILED: Token decimals mismatch");

        // Check token metadata matches OKB
        string memory newTokenName = upgradedSystemConfig.gasPayingTokenName();
        string memory newTokenSymbol = upgradedSystemConfig.gasPayingTokenSymbol();
        console.log("New gas token name:", newTokenName);
        console.log("New gas token symbol:", newTokenSymbol);
        require(
            keccak256(abi.encodePacked(newTokenName)) == keccak256(abi.encodePacked(okbToken.name())),
            "FAILED: Token name mismatch"
        );
        require(
            keccak256(abi.encodePacked(newTokenSymbol)) == keccak256(abi.encodePacked(okbToken.symbol())),
            "FAILED: Token symbol mismatch"
        );

        // Verify custom gas token flag is true
        bool isCustomGasToken = upgradedSystemConfig.isCustomGasToken();
        require(isCustomGasToken, "Custom gas token flag should be true");

        // Check DepositedOKBAdapter configuration
        require(address(adapter.OKB()) == okbTokenAddress, "FAILED: Adapter OKB mismatch");
        require(address(adapter.PORTAL()) == optimismPortalProxy, "FAILED: Adapter portal mismatch");
        require(adapter.owner() == deployerAddress, "FAILED: Adapter owner mismatch");

        // Check adapter has preminted total supply
        uint256 adapterBalance = adapter.balanceOf(address(adapter));
        uint256 expectedBalance = okbToken.totalSupply();
        console.log("Adapter balance:", adapterBalance);
        console.log("Expected balance (OKB total supply):", expectedBalance);
        require(adapterBalance == expectedBalance, "FAILED: Adapter balance should equal OKB total supply");

        // Check whitelist configuration
        console.log("Verifying deployer whitelist...");
        require(adapter.whitelist(deployerAddress), "FAILED: Deployer address not whitelisted");
        console.log("Deployer whitelist verified:", deployerAddress);

        // Check Adapter approval to portal (should be zero initially)
        uint256 allowance = adapter.allowance(address(adapter), optimismPortalProxy);
        console.log("Adapter approval to Portal:", allowance);
        require(allowance == 0, "FAILED: Adapter should not pre-approve portal");

        console.log("All upgrade verifications passed!");
    }
}
