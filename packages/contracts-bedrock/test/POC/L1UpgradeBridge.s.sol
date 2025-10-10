// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import {IL1StandardBridge} from "../../interfaces/L1/IL1StandardBridge.sol";
import {CommonBase} from "../../lib/forge-std/src/Base.sol";
import {Script} from "../../lib/forge-std/src/Script.sol";
import {StdChains} from "../../lib/forge-std/src/StdChains.sol";
import {StdCheatsSafe} from "../../lib/forge-std/src/StdCheats.sol";
import {StdUtils} from "../../lib/forge-std/src/StdUtils.sol";
import {console} from "../../lib/forge-std/src/console.sol";
import {ProxyAdmin} from "../../src/universal/ProxyAdmin.sol";
import {L1StandardBridgeNew} from "../../src/L1/L1StandardBridgeNew.sol";

/// @title L1UpgradeBridge
/// @notice Script to upgrade L1StandardBridge implementation
contract L1UpgradeBridge is Script {
    // Contract addresses will be loaded from state.json
    address public L1_STANDARD_BRIDGE_PROXY;
    address public OP_CHAIN_PROXY_ADMIN;
    address constant TRANSACTOR = 0x5FbDB2315678afecb367f032d93F642f64180aa3;
    address constant TRANSACTOR_OWNER = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;

    // Private key will be loaded from environment
    uint256 private transactorPrivateKey;

    /// @notice Load addresses from state.json file
    function loadAddresses() internal {
        string memory stateJson = vm.readFile("../../test/config-op/state.json");
        L1_STANDARD_BRIDGE_PROXY = vm.parseJsonAddress(stateJson, ".opChainDeployments[0].L1StandardBridgeProxy");
        OP_CHAIN_PROXY_ADMIN = vm.parseJsonAddress(stateJson, ".opChainDeployments[0].OpChainProxyAdminImpl");

        console.log("Loaded L1StandardBridge proxy:", L1_STANDARD_BRIDGE_PROXY);
        console.log("Loaded ProxyAdmin:", OP_CHAIN_PROXY_ADMIN);
    }

    /// @notice Load private key from environment variable
    function loadPrivateKey() internal {
        transactorPrivateKey = vm.envUint("TRANSACTOR_PRIVATE_KEY");
        require(transactorPrivateKey != 0, "TRANSACTOR_PRIVATE_KEY not found in environment");
    }

    function run() external {
        console.log("=== L1StandardBridge Upgrade Script ===");

        // Load addresses from state.json
        loadAddresses();

        // Load private key from environment
        loadPrivateKey();

        // Start broadcasting with the transactor private key
        vm.startBroadcast(transactorPrivateKey);

        // 1. Deploy new L1StandardBridge implementation
        console.log("Deploying new L1StandardBridge implementation...");
        L1StandardBridgeNew newImplementation = new L1StandardBridgeNew();
        console.log("New implementation deployed at:", address(newImplementation));

        // 2. Get current implementation for comparison
        ProxyAdmin proxyAdmin = ProxyAdmin(OP_CHAIN_PROXY_ADMIN);
        address currentImpl = proxyAdmin.getProxyImplementation(L1_STANDARD_BRIDGE_PROXY);
        console.log("Current implementation:", currentImpl);

        // 3. Verify Transactor ownership
        address transactorOwner = TRANSACTOR_OWNER; // We know this should be our address
        console.log("Transactor owner (expected):", transactorOwner);
        require(transactorOwner == vm.addr(transactorPrivateKey), "Private key does not match expected Transactor owner");

        // 4. Verify proxy admin ownership (should be TRANSACTOR contract)
        address proxyAdminOwner = proxyAdmin.owner();
        console.log("ProxyAdmin owner:", proxyAdminOwner);
        require(proxyAdminOwner == TRANSACTOR, "TRANSACTOR is not the ProxyAdmin owner");

        // 4. Verify proxy type
        ProxyAdmin.ProxyType ptype = proxyAdmin.proxyType(L1_STANDARD_BRIDGE_PROXY);
        console.log("Proxy type:", uint256(ptype));

        // 5. Perform the upgrade through Transactor contract
        console.log("Upgrading L1StandardBridge through Transactor contract...");

        // Prepare the call data for ProxyAdmin.upgrade()
        bytes memory upgradeCalldata = abi.encodeWithSignature(
            "upgrade(address,address)",
            L1_STANDARD_BRIDGE_PROXY,
            address(newImplementation)
        );

        // Call Transactor.CALL() to execute ProxyAdmin.upgrade()
        (bool success, bytes memory returnData) = TRANSACTOR.call(
            abi.encodeWithSignature(
                "CALL(address,bytes,uint256)",
                OP_CHAIN_PROXY_ADMIN,
                upgradeCalldata,
                0 // no ETH value
            )
        );

        require(success, "Upgrade through Transactor failed");

        // Decode the return data from Transactor.CALL()
        (bool upgradeSuccess, bytes memory upgradeReturnData) = abi.decode(returnData, (bool, bytes));
        require(upgradeSuccess, string(abi.encodePacked("ProxyAdmin upgrade failed: ", upgradeReturnData)));

        // 6. Verify the upgrade
        address newImpl = proxyAdmin.getProxyImplementation(L1_STANDARD_BRIDGE_PROXY);
        console.log("New implementation after upgrade:", newImpl);
        require(newImpl == address(newImplementation), "Upgrade failed");

        // 7. Test basic functionality (optional)
        IL1StandardBridge bridge = IL1StandardBridge(payable(L1_STANDARD_BRIDGE_PROXY));
        string memory version = bridge.version();
        console.log("Bridge version:", version);

        vm.stopBroadcast();

        console.log("=== Upgrade completed successfully! ===");
        console.log("Old implementation:", currentImpl);
        console.log("New implementation:", address(newImplementation));
    }

    /// @notice Helper function to upgrade with initialization data
    /// @param _initData Initialization data to call on the new implementation
    function upgradeAndCall(bytes memory _initData) external {
        console.log("=== L1StandardBridge Upgrade with Call Script ===");

        // Load private key from environment
        loadPrivateKey();

        vm.startBroadcast(transactorPrivateKey);

        // Deploy new implementation
        L1StandardBridgeNew newImplementation = new L1StandardBridgeNew();
        console.log("New implementation deployed at:", address(newImplementation));

        // Upgrade and call through Transactor
        bytes memory upgradeAndCallCalldata = abi.encodeWithSignature(
            "upgradeAndCall(address,address,bytes)",
            L1_STANDARD_BRIDGE_PROXY,
            address(newImplementation),
            _initData
        );

        (bool success, bytes memory returnData) = TRANSACTOR.call(
            abi.encodeWithSignature(
                "CALL(address,bytes,uint256)",
                OP_CHAIN_PROXY_ADMIN,
                upgradeAndCallCalldata,
                0
            )
        );

        require(success, "UpgradeAndCall through Transactor failed");

        (bool upgradeSuccess, bytes memory upgradeReturnData) = abi.decode(returnData, (bool, bytes));
        require(upgradeSuccess, string(abi.encodePacked("ProxyAdmin upgradeAndCall failed: ", upgradeReturnData)));

        vm.stopBroadcast();

        console.log("=== Upgrade with call completed! ===");
    }

    /// @notice View function to check current state
    function checkCurrentState() external view {
        console.log("=== Current State Check ===");

        ProxyAdmin proxyAdmin = ProxyAdmin(OP_CHAIN_PROXY_ADMIN);

        // Check proxy admin owner
        address proxyAdminOwner = proxyAdmin.owner();
        console.log("ProxyAdmin owner:", proxyAdminOwner);

        // Check current implementation
        address currentImpl = proxyAdmin.getProxyImplementation(L1_STANDARD_BRIDGE_PROXY);
        console.log("Current L1StandardBridge implementation:", currentImpl);

        // Check proxy type
        ProxyAdmin.ProxyType ptype = proxyAdmin.proxyType(L1_STANDARD_BRIDGE_PROXY);
        console.log("Proxy type:", uint256(ptype));

        // Check bridge version
        IL1StandardBridge bridge = IL1StandardBridge(payable(L1_STANDARD_BRIDGE_PROXY));
        try bridge.version() returns (string memory version) {
            console.log("Bridge version:", version);
        } catch {
            console.log("Could not get bridge version");
        }
    }
}
