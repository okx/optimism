// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import { Script } from "forge-std/Script.sol";
import { console } from "forge-std/console.sol";
import { SystemConfig } from "src/L1/SystemConfig.sol";
import { IProxyAdmin } from "interfaces/universal/IProxyAdmin.sol";
import { Transactor } from "src/periphery/Transactor.sol";
import { ISystemConfig } from "interfaces/L1/ISystemConfig.sol";
import { IResourceMetering } from "interfaces/L1/IResourceMetering.sol";
import { ISuperchainConfig } from "interfaces/L1/ISuperchainConfig.sol";

/// @title UpgradeSystemConfig
/// @notice Simulation script to upgrade SystemConfig and reinitialize with existing parameters
/// @dev This script:
///      1. Reads environment variables
///      2. Validates ownership chain
///      3. Reads current SystemConfig parameters
///      4. Deploys new SystemConfig implementation
///      5. Simulates upgrade and reinitialization via ProxyAdmin.upgradeAndCall()
///      6. Verifies upgrade (simulation only - no actual broadcast)
contract UpgradeSystemConfig is Script {
    // Environment variable names
    string constant SYSTEM_CONFIG_PROXY = "SYSTEM_CONFIG_PROXY_ADDRESS";
    string constant PROXY_ADMIN = "OP_PROXY_ADMIN";
    string constant TRANSACTOR = "TRANSACTOR";
    string constant DELAYED_WETH = "DELAYED_WETH";
    string constant NEW_IMPLEMENTATION = "NEW_IMPLEMENTATION"; // Optional: if provided, use existing implementation

    // State variables for configuration
    address systemConfigProxy;
    address proxyAdmin;
    address transactorAddress;
    address deployerAddress;
    address delayedWETHAddress;
    address newImplementationAddress; // Optional: existing implementation to use

    // Deployed contracts
    Transactor transactor;

    // Struct to hold current SystemConfig parameters
    struct SystemConfigParams {
        address owner;
        uint32 basefeeScalar;
        uint32 blobbasefeeScalar;
        bytes32 batcherHash;
        uint64 gasLimit;
        address unsafeBlockSigner;
        IResourceMetering.ResourceConfig resourceConfig;
        address batchInbox;
        SystemConfig.Addresses addresses;
        uint256 l2ChainId;
        ISuperchainConfig superchainConfig;
    }

    /// @notice Main upgrade function
    function run() external {
        _loadConfiguration();
        _validateConfiguration();
        SystemConfigParams memory currentParams = _readCurrentParameters();
        _performUpgradeWithReinit(currentParams);
    }

    /// @notice Load configuration from environment variables
    function _loadConfiguration() internal {
        // Get deployer address from msg.sender (set by forge script --private-key)
        deployerAddress = msg.sender;

        // Load addresses from environment
        systemConfigProxy = vm.envAddress(SYSTEM_CONFIG_PROXY);
        proxyAdmin = vm.envAddress(PROXY_ADMIN);
        transactorAddress = vm.envAddress(TRANSACTOR);
        delayedWETHAddress = vm.envAddress(DELAYED_WETH);
        
        // Optional: load existing implementation address
        try vm.envAddress(NEW_IMPLEMENTATION) returns (address impl) {
            newImplementationAddress = impl;
        } catch {
            newImplementationAddress = address(0);
        }

        console.log("=== Upgrade Configuration ===");
        console.log("Deployer address:", deployerAddress);
        console.log("SystemConfig Proxy:", systemConfigProxy);
        console.log("ProxyAdmin:", proxyAdmin);
        console.log("Transactor:", transactorAddress);
        console.log("DelayedWETH:", delayedWETHAddress);
        if (newImplementationAddress != address(0)) {
            console.log("Using existing implementation:", newImplementationAddress);
        } else {
            console.log("Will deploy new implementation");
        }

        // Initialize contract interfaces
        transactor = Transactor(transactorAddress);
    }

    /// @notice Validate the configuration before proceeding
    function _validateConfiguration() internal view {
        require(systemConfigProxy != address(0), "SystemConfig proxy address cannot be zero");
        require(proxyAdmin != address(0), "ProxyAdmin address cannot be zero");
        require(transactorAddress != address(0), "Transactor address cannot be zero");
        require(deployerAddress != address(0), "Deployer address cannot be zero");
        require(delayedWETHAddress != address(0), "DelayedWETH address cannot be zero");

        // Verify contracts have code
        require(systemConfigProxy.code.length > 0, "SystemConfig proxy must have code (not an EOA)");
        require(proxyAdmin.code.length > 0, "ProxyAdmin must have code (not an EOA)");
        require(transactorAddress.code.length > 0, "Transactor must have code (not an EOA)");
        
        // If using existing implementation, verify it has code
        if (newImplementationAddress != address(0)) {
            require(newImplementationAddress.code.length > 0, "New implementation must have code (not an EOA)");
        }

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

    /// @notice Read current SystemConfig parameters for reinitialization
    function _readCurrentParameters() internal view returns (SystemConfigParams memory params) {
        console.log("\n--- Reading Current SystemConfig Parameters ---");

        ISystemConfig currentSystemConfig = ISystemConfig(systemConfigProxy);

        // Read basic parameters
        params.owner = currentSystemConfig.owner();
        params.basefeeScalar = currentSystemConfig.basefeeScalar();
        params.blobbasefeeScalar = currentSystemConfig.blobbasefeeScalar();
        params.batcherHash = currentSystemConfig.batcherHash();
        params.gasLimit = currentSystemConfig.gasLimit();
        params.unsafeBlockSigner = currentSystemConfig.unsafeBlockSigner();
        params.batchInbox = currentSystemConfig.batchInbox();
        params.l2ChainId = currentSystemConfig.l2ChainId();

        // Read resource config
        params.resourceConfig = currentSystemConfig.resourceConfig();

        // Read system contract addresses individually (old version may not have delayedWETH field)
        params.addresses = SystemConfig.Addresses({
            l1CrossDomainMessenger: currentSystemConfig.l1CrossDomainMessenger(),
            l1ERC721Bridge: currentSystemConfig.l1ERC721Bridge(),
            l1StandardBridge: currentSystemConfig.l1StandardBridge(),
            optimismPortal: currentSystemConfig.optimismPortal(),
            optimismMintableERC20Factory: currentSystemConfig.optimismMintableERC20Factory(),
            delayedWETH: delayedWETHAddress  // New field added in this upgrade
        });

        // Read superchain config
        params.superchainConfig = currentSystemConfig.superchainConfig();

        // Log the parameters
        _logCurrentParameters(params);

        return params;
    }

    /// @notice Log the current parameters that will be used for reinitialization
    function _logCurrentParameters(SystemConfigParams memory params) internal pure {
        console.log("Current owner:", params.owner);
        console.log("Current basefee scalar:", params.basefeeScalar);
        console.log("Current blobbasefee scalar:", params.blobbasefeeScalar);
        console.log("Current batcher hash:");
        console.logBytes32(params.batcherHash);
        console.log("Current gas limit:", params.gasLimit);
        console.log("Current unsafe block signer:", params.unsafeBlockSigner);
        console.log("Current batch inbox:", params.batchInbox);
        console.log("Current L2 chain ID:", params.l2ChainId);
        console.log("Current superchain config:", address(params.superchainConfig));

        console.log("\n--- Current System Contract Addresses ---");
        console.log("L1CrossDomainMessenger:", params.addresses.l1CrossDomainMessenger);
        console.log("L1ERC721Bridge:", params.addresses.l1ERC721Bridge);
        console.log("L1StandardBridge:", params.addresses.l1StandardBridge);
        console.log("OptimismPortal:", params.addresses.optimismPortal);
        console.log("OptimismMintableERC20Factory:", params.addresses.optimismMintableERC20Factory);
        console.log("DelayedWETH (NEW FIELD):", params.addresses.delayedWETH);

        console.log("\n--- Current Resource Config ---");
        console.log("Max resource limit:", params.resourceConfig.maxResourceLimit);
        console.log("Elasticity multiplier:", params.resourceConfig.elasticityMultiplier);
        console.log("Base fee max change denominator:", params.resourceConfig.baseFeeMaxChangeDenominator);
        console.log("Minimum base fee:", params.resourceConfig.minimumBaseFee);
        console.log("System tx max gas:", params.resourceConfig.systemTxMaxGas);
        console.log("Maximum base fee:", params.resourceConfig.maximumBaseFee);
    }

    /// @notice Perform the complete upgrade process with reinitialization (simulation)
    function _performUpgradeWithReinit(SystemConfigParams memory params) internal {
        console.log("\n=== Starting SystemConfig Upgrade with Reinitialization (SIMULATION) ===");

        // Step 1: Check current state
        _logCurrentState();

        // Step 2: Get or deploy SystemConfig implementation
        SystemConfig newImplementation;
        if (newImplementationAddress != address(0)) {
            console.log("\n--- Using Existing SystemConfig Implementation ---");
            newImplementation = SystemConfig(newImplementationAddress);
            console.log("Using existing SystemConfig at:", address(newImplementation));
        } else {
            console.log("\n--- Deploying SystemConfig Implementation ---");
            newImplementation = new SystemConfig();
            console.log("SystemConfig deployed at:", address(newImplementation));
        }
        console.log("Implementation version:", newImplementation.version());

        // Step 3: Prepare initialization data
        console.log("\n--- Preparing Reinitialization Data ---");
        bytes memory initData = abi.encodeWithSelector(
            ISystemConfig.initialize.selector,
            params.owner,
            params.basefeeScalar,
            params.blobbasefeeScalar,
            params.batcherHash,
            params.gasLimit,
            params.unsafeBlockSigner,
            params.resourceConfig,
            params.batchInbox,
            params.addresses,
            params.l2ChainId,
            params.superchainConfig
        );

        // Step 4: Simulate upgrade and reinitialize via Transactor
        console.log("\n--- Simulating Upgrade and Reinitialization via Transactor ---");

        // Encode the ProxyAdmin.upgradeAndCall() call
        bytes memory upgradeAndCallData = abi.encodeWithSelector(
            IProxyAdmin.upgradeAndCall.selector,
            payable(systemConfigProxy),
            address(newImplementation),
            initData
        );

        console.log("Simulating ProxyAdmin.upgradeAndCall() via Transactor.CALL()");

        // Simulate ProxyAdmin.upgradeAndCall() through Transactor.CALL()
        console.log("Transactor owner:", transactor.owner());
        console.log("Simulate transactor owner to upgrade and reinitialize SystemConfig");
        vm.prank(transactor.owner());
        (bool success,) = transactor.CALL(
            proxyAdmin,
            upgradeAndCallData,
            0 // no ETH value
        );
        require(success, "Transactor CALL to ProxyAdmin.upgradeAndCall failed");
        console.log("Upgrade and reinitialization simulation completed successfully");
        console.log("transactor.Call() params:");
        console.log("target: ", proxyAdmin);
        console.log("data length:", upgradeAndCallData.length);
        console.log("value: 0");

        SystemConfig upgradedSystemConfig = SystemConfig(systemConfigProxy);

        // Step 5: Verify the upgrade and reinitialization
        _verifyUpgradeAndReinit(upgradedSystemConfig, params);

        console.log("\n=== SystemConfig Upgrade with Reinitialization Simulation Completed Successfully ===");
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

    /// @notice Verify the upgrade and reinitialization was successful
    function _verifyUpgradeAndReinit(SystemConfig upgradedSystemConfig, SystemConfigParams memory expectedParams) internal view {
        console.log("\n--- Verifying Upgrade and Reinitialization Results ---");

        // Check version updated
        string memory newVersion = upgradedSystemConfig.version();
        console.log("New version:", newVersion);
        require(keccak256(bytes(newVersion)) == keccak256(bytes("3.12.0")), "Version not updated correctly");
        require(upgradedSystemConfig.initVersion() == 4, "initVersion not currect");
        // Verify all parameters were preserved during reinitialization
        console.log("\n--- Verifying Parameter Preservation ---");

        require(upgradedSystemConfig.owner() == expectedParams.owner, "Owner not preserved");
        require(upgradedSystemConfig.basefeeScalar() == expectedParams.basefeeScalar, "Basefee scalar not preserved");
        require(upgradedSystemConfig.blobbasefeeScalar() == expectedParams.blobbasefeeScalar, "Blobbasefee scalar not preserved");
        require(upgradedSystemConfig.batcherHash() == expectedParams.batcherHash, "Batcher hash not preserved");
        require(upgradedSystemConfig.gasLimit() == expectedParams.gasLimit, "Gas limit not preserved");
        require(upgradedSystemConfig.unsafeBlockSigner() == expectedParams.unsafeBlockSigner, "Unsafe block signer not preserved");
        require(upgradedSystemConfig.batchInbox() == expectedParams.batchInbox, "Batch inbox not preserved");
        require(upgradedSystemConfig.l2ChainId() == expectedParams.l2ChainId, "L2 chain ID not preserved");
        require(address(upgradedSystemConfig.superchainConfig()) == address(expectedParams.superchainConfig), "Superchain config not preserved");

        // Verify system contract addresses
        SystemConfig.Addresses memory newAddresses = upgradedSystemConfig.getAddresses();
        require(newAddresses.l1CrossDomainMessenger == expectedParams.addresses.l1CrossDomainMessenger, "L1CrossDomainMessenger not preserved");
        require(newAddresses.l1ERC721Bridge == expectedParams.addresses.l1ERC721Bridge, "L1ERC721Bridge not preserved");
        require(newAddresses.l1StandardBridge == expectedParams.addresses.l1StandardBridge, "L1StandardBridge not preserved");
        require(newAddresses.optimismPortal == expectedParams.addresses.optimismPortal, "OptimismPortal not preserved");
        require(newAddresses.optimismMintableERC20Factory == expectedParams.addresses.optimismMintableERC20Factory, "OptimismMintableERC20Factory not preserved");
        require(newAddresses.delayedWETH == expectedParams.addresses.delayedWETH, "DelayedWETH not set correctly");

        console.log("DelayedWETH successfully added:", newAddresses.delayedWETH);

        // Verify resource config
        IResourceMetering.ResourceConfig memory newResourceConfig = upgradedSystemConfig.resourceConfig();
        require(newResourceConfig.maxResourceLimit == expectedParams.resourceConfig.maxResourceLimit, "Max resource limit not preserved");
        require(newResourceConfig.elasticityMultiplier == expectedParams.resourceConfig.elasticityMultiplier, "Elasticity multiplier not preserved");
        require(newResourceConfig.baseFeeMaxChangeDenominator == expectedParams.resourceConfig.baseFeeMaxChangeDenominator, "Base fee max change denominator not preserved");
        require(newResourceConfig.minimumBaseFee == expectedParams.resourceConfig.minimumBaseFee, "Minimum base fee not preserved");
        require(newResourceConfig.systemTxMaxGas == expectedParams.resourceConfig.systemTxMaxGas, "System tx max gas not preserved");
        require(newResourceConfig.maximumBaseFee == expectedParams.resourceConfig.maximumBaseFee, "Maximum base fee not preserved");

        console.log("All upgrade and reinitialization verifications passed!");
        console.log("SystemConfig successfully simulated upgrade to version:", newVersion);
        console.log("All existing parameters preserved during reinitialization simulation");
        console.log("DelayedWETH field successfully added to SystemConfig");
        console.log("\n*** NOTE: This was a SIMULATION - no actual transactions were broadcast ***");
    }
}
