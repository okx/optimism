// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import { Script } from "forge-std/Script.sol";
import { console } from "forge-std/console.sol";
import { ProxyAdmin } from "../../src/universal/ProxyAdmin.sol";
import { IL2StandardBridge } from "../../interfaces/L2/IL2StandardBridge.sol";
import { Predeploys } from "../../src/libraries/Predeploys.sol";
import { L2StandardBridgeNew } from "../../src/L2/L2StandardBridgeNew.sol";

/// @title L2UpgradeBridge
/// @notice Script to upgrade L2StandardBridge implementation
contract L2UpgradeBridge is Script {
    // L2 Predeploy addresses
    address constant L2_STANDARD_BRIDGE_PROXY = 0x4200000000000000000000000000000000000010;
    address constant L2_PROXY_ADMIN = 0x4200000000000000000000000000000000000018;

    // Private key will be loaded from environment
    uint256 private deployerPrivateKey;

    /// @notice Load private key from environment variable
    function loadPrivateKey() internal {
        deployerPrivateKey = vm.envUint("TRANSACTOR_PRIVATE_KEY");
        require(deployerPrivateKey != 0, "TRANSACTOR_PRIVATE_KEY not found in environment");
    }

    function run() external {
        console.log("=== L2StandardBridge Upgrade Script ===");

        // Load private key from environment
        loadPrivateKey();

        address deployer = vm.addr(deployerPrivateKey);
        console.log("Deployer address:", deployer);

        // Start broadcasting with the deployer private key
        vm.startBroadcast(deployerPrivateKey);

        // 1. Deploy new L2StandardBridge implementation
        console.log("Deploying new L2StandardBridgeNew implementation...");
        L2StandardBridgeNew newImplementation = new L2StandardBridgeNew();
        console.log("New implementation deployed at:", address(newImplementation));

        // 2. Get current implementation for comparison
        ProxyAdmin proxyAdmin = ProxyAdmin(L2_PROXY_ADMIN);
        address currentImpl = proxyAdmin.getProxyImplementation(L2_STANDARD_BRIDGE_PROXY);
        console.log("Current implementation:", currentImpl);

        // 3. Verify proxy admin ownership
        address proxyAdminOwner = proxyAdmin.owner();
        console.log("L2 ProxyAdmin owner:", proxyAdminOwner);

        // Check if we have permission to upgrade
        if (proxyAdminOwner == deployer) {
            console.log("Direct ownership - can upgrade directly");

            // 4. Verify proxy type
            ProxyAdmin.ProxyType ptype = proxyAdmin.proxyType(L2_STANDARD_BRIDGE_PROXY);
            console.log("Proxy type:", uint256(ptype));

            // 5. Perform the upgrade
            console.log("Upgrading L2StandardBridge...");
            proxyAdmin.upgrade(
                payable(L2_STANDARD_BRIDGE_PROXY),
                address(newImplementation)
            );

            // 6. Verify the upgrade
            address newImpl = proxyAdmin.getProxyImplementation(L2_STANDARD_BRIDGE_PROXY);
            console.log("New implementation after upgrade:", newImpl);
            require(newImpl == address(newImplementation), "Upgrade failed");

            // 7. Test basic functionality (optional)
            IL2StandardBridge bridge = IL2StandardBridge(payable(L2_STANDARD_BRIDGE_PROXY));
            string memory version = bridge.version();
            console.log("Bridge version:", version);

        } else {
            console.log("No direct ownership - need to use owner's mechanism");
            console.log("ProxyAdmin owner is:", proxyAdminOwner);
            revert("Cannot upgrade: deployer is not the ProxyAdmin owner");
        }

        vm.stopBroadcast();

        console.log("=== L2 Upgrade completed successfully! ===");
        console.log("Old implementation:", currentImpl);
        console.log("New implementation:", address(newImplementation));
    }

    /// @notice Helper function to upgrade with initialization data
    /// @param _initData Initialization data to call on the new implementation
    function upgradeAndCall(bytes memory _initData) external {
        console.log("=== L2StandardBridge Upgrade with Call Script ===");

        // Load private key from environment
        loadPrivateKey();

        vm.startBroadcast(deployerPrivateKey);

        // Deploy new implementation
        L2StandardBridgeNew newImplementation = new L2StandardBridgeNew();
        console.log("New implementation deployed at:", address(newImplementation));

        // Upgrade and call
        ProxyAdmin proxyAdmin = ProxyAdmin(L2_PROXY_ADMIN);
        proxyAdmin.upgradeAndCall(
            payable(L2_STANDARD_BRIDGE_PROXY),
            address(newImplementation),
            _initData
        );

        vm.stopBroadcast();

        console.log("=== L2 Upgrade with call completed! ===");
    }

    /// @notice View function to check current state
    function checkCurrentState() external view {
        console.log("=== L2 Current State Check ===");

        ProxyAdmin proxyAdmin = ProxyAdmin(L2_PROXY_ADMIN);

        // Check proxy admin owner
        address proxyAdminOwner = proxyAdmin.owner();
        console.log("L2 ProxyAdmin owner:", proxyAdminOwner);

        // Check current implementation
        address currentImpl = proxyAdmin.getProxyImplementation(L2_STANDARD_BRIDGE_PROXY);
        console.log("Current L2StandardBridge implementation:", currentImpl);

        // Check proxy type
        ProxyAdmin.ProxyType ptype = proxyAdmin.proxyType(L2_STANDARD_BRIDGE_PROXY);
        console.log("Proxy type:", uint256(ptype));

        // Check bridge version
        IL2StandardBridge bridge = IL2StandardBridge(payable(L2_STANDARD_BRIDGE_PROXY));
        try bridge.version() returns (string memory version) {
            console.log("Bridge version:", version);
        } catch {
            console.log("Could not get bridge version");
        }

        // Check bridge addresses
        console.log("L2StandardBridge proxy:", L2_STANDARD_BRIDGE_PROXY);
        console.log("L2 ProxyAdmin:", L2_PROXY_ADMIN);
    }

    /// @notice Check if we can upgrade (useful for pre-flight checks)
    function canUpgrade() external view returns (bool) {
        ProxyAdmin proxyAdmin = ProxyAdmin(L2_PROXY_ADMIN);
        address proxyAdminOwner = proxyAdmin.owner();

        // Load the deployer address from private key
        uint256 pk = vm.envUint("TRANSACTOR_PRIVATE_KEY");
        address deployer = vm.addr(pk);

        return proxyAdminOwner == deployer;
    }
}
