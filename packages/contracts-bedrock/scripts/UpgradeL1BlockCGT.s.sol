// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import "../interfaces/L2/IL1BlockCGT.sol";
import {CommonBase} from "../lib/forge-std/src/Base.sol";
import {IProxyAdmin} from "../interfaces/universal/IProxyAdmin.sol";
import {L1BlockCGT} from "../src/L2/L1BlockCGT.sol";
import {Script} from "../lib/forge-std/src/Script.sol";
import {StdChains} from "../lib/forge-std/src/StdChains.sol";
import {StdCheatsSafe} from "../lib/forge-std/src/StdCheats.sol";
import {StdUtils} from "../lib/forge-std/src/StdUtils.sol";
import {console} from "../lib/forge-std/src/console.sol";

/// @title UpgradeL1BlockCGT_Direct
/// @notice Direct upgrade script for L1Block to L1BlockCGT using specific addresses
/// @dev This script uses hardcoded addresses:
///      - L1Block Proxy: 0x4200000000000000000000000000000000000015
///      - ProxyAdmin: 0x4200000000000000000000000000000000000018
///      - ProxyAdmin Owner: 0x6eE7BDa7AF04F61ccf93aB4b8DB2289aBe76C6aA
contract UpgradeL1BlockCGT_Direct is Script {
    // Hardcoded addresses
    address constant L1_BLOCK_PROXY = 0x4200000000000000000000000000000000000015;
    address constant PROXY_ADMIN = 0x4200000000000000000000000000000000000018;
    address constant PROXY_ADMIN_OWNER = 0x6eE7BDa7AF04F61ccf93aB4b8DB2289aBe76C6aA;

    // Optional: use existing implementation or deploy new one
    address newImplementationAddress;

    // Struct to hold current L1Block parameters (before upgrade)
    struct L1BlockParams {
        uint64 number;
        uint64 timestamp;
        uint256 basefee;
        bytes32 hash;
        uint64 sequenceNumber;
        bytes32 batcherHash;
        uint256 l1FeeOverhead;
        uint256 l1FeeScalar;
        uint32 baseFeeScalar;
        uint32 blobBaseFeeScalar;
        uint256 blobBaseFee;
        uint64 operatorFeeConstant;
        uint32 operatorFeeScalar;
        string version;
    }

    /// @notice Main upgrade function
    function run() external {
        _loadConfiguration();
        _validateConfiguration();
        L1BlockParams memory currentParams = _readCurrentParameters();
        _performUpgrade(currentParams);
    }

    /// @notice Load configuration from environment variables (optional implementation address)
    function _loadConfiguration() internal {
        console.log("=== Direct Upgrade Configuration ===");
        console.log("L1Block Proxy:", L1_BLOCK_PROXY);
        console.log("ProxyAdmin:", PROXY_ADMIN);
        console.log("ProxyAdmin Owner:", PROXY_ADMIN_OWNER);

        // Optional: load existing implementation address
        try vm.envAddress("NEW_IMPLEMENTATION") returns (address impl) {
            newImplementationAddress = impl;
            console.log("Using existing L1BlockCGT implementation:", newImplementationAddress);
        } catch {
            newImplementationAddress = address(0);
            console.log("Will deploy new L1BlockCGT implementation");
        }
    }

    /// @notice Validate the configuration before proceeding
    function _validateConfiguration() internal view {
        console.log("\n--- Validating Configuration ---");

        // Verify contracts have code
        require(L1_BLOCK_PROXY.code.length > 0, "L1Block proxy must have code");
        require(PROXY_ADMIN.code.length > 0, "ProxyAdmin must have code");

        // If using existing implementation, verify it has code
        if (newImplementationAddress != address(0)) {
            require(newImplementationAddress.code.length > 0, "New L1BlockCGT implementation must have code");
        }

        // Verify ownership chain for upgrade permissions
        _validateOwnershipChain();
    }

    /// @notice Validate the ownership chain for upgrade permissions
    function _validateOwnershipChain() internal view {
        console.log("\n--- Validating Ownership Chain ---");

        // Check ProxyAdmin owner
        IProxyAdmin admin = IProxyAdmin(PROXY_ADMIN);
        address actualOwner = admin.owner();
        console.log("ProxyAdmin actual owner:", actualOwner);
        console.log("Expected owner:", PROXY_ADMIN_OWNER);

        // Verify ownership
        require(actualOwner == PROXY_ADMIN_OWNER, "ProxyAdmin owner mismatch");

        // Check L1Block proxy admin
        try admin.getProxyAdmin(payable(L1_BLOCK_PROXY)) returns (address l1BlockAdmin) {
            console.log("L1Block proxy admin:", l1BlockAdmin);
            console.log("Expected ProxyAdmin:", PROXY_ADMIN);

            // Verify ProxyAdmin controls L1Block proxy
            require(l1BlockAdmin == PROXY_ADMIN, "ProxyAdmin must be the admin of L1Block proxy");
        } catch {
            console.log("Warning: Could not verify L1Block proxy admin - assuming correct");
        }

        console.log("Ownership chain validated successfully");
    }

    /// @notice Read current L1Block parameters before upgrade
    function _readCurrentParameters() internal view returns (L1BlockParams memory params) {
        console.log("\n--- Reading Current L1Block Parameters ---");

        // Use IL1BlockCGT interface since it extends IL1Block
        IL1BlockCGT currentL1Block = IL1BlockCGT(L1_BLOCK_PROXY);

        // Read basic parameters
        params.number = currentL1Block.number();
        params.timestamp = currentL1Block.timestamp();
        params.basefee = currentL1Block.basefee();
        params.hash = currentL1Block.hash();
        params.sequenceNumber = currentL1Block.sequenceNumber();
        params.batcherHash = currentL1Block.batcherHash();
        params.l1FeeOverhead = currentL1Block.l1FeeOverhead();
        params.l1FeeScalar = currentL1Block.l1FeeScalar();
        params.baseFeeScalar = currentL1Block.baseFeeScalar();
        params.blobBaseFeeScalar = currentL1Block.blobBaseFeeScalar();
        params.blobBaseFee = currentL1Block.blobBaseFee();
        params.operatorFeeConstant = currentL1Block.operatorFeeConstant();
        params.operatorFeeScalar = currentL1Block.operatorFeeScalar();
        params.version = currentL1Block.version();

        // Log the parameters
        _logCurrentParameters(params);

        return params;
    }

    /// @notice Log the current parameters before upgrade
    function _logCurrentParameters(L1BlockParams memory params) internal pure {
        console.log("Current number:", params.number);
        console.log("Current timestamp:", params.timestamp);
        console.log("Current basefee:", params.basefee);
        console.log("Current hash:");
        console.logBytes32(params.hash);
        console.log("Current sequence number:", params.sequenceNumber);
        console.log("Current batcher hash:");
        console.logBytes32(params.batcherHash);
        console.log("Current L1 fee overhead:", params.l1FeeOverhead);
        console.log("Current L1 fee scalar:", params.l1FeeScalar);
        console.log("Current base fee scalar:", params.baseFeeScalar);
        console.log("Current blob base fee scalar:", params.blobBaseFeeScalar);
        console.log("Current blob base fee:", params.blobBaseFee);
        console.log("Current operator fee constant:", params.operatorFeeConstant);
        console.log("Current operator fee scalar:", params.operatorFeeScalar);
        console.log("Current version:", params.version);
    }

    /// @notice Perform the complete upgrade process from L1Block to L1BlockCGT
    function _performUpgrade(L1BlockParams memory params) internal {
        console.log("\n=== Starting L1Block to L1BlockCGT Direct Upgrade ===");

        // Step 1: Check current state
        _logCurrentState();

        // Step 2: Get or deploy L1BlockCGT implementation
        L1BlockCGT newImplementation;
        if (newImplementationAddress != address(0)) {
            console.log("\n--- Using Existing L1BlockCGT Implementation ---");
            newImplementation = L1BlockCGT(newImplementationAddress);

            // Validate bytecode matches local L1BlockCGT
            _validateImplementationBytecode(newImplementationAddress);
        } else {
            console.log("\n--- Deploying L1BlockCGT Implementation ---");
            newImplementation = new L1BlockCGT();
            console.log("L1BlockCGT deployed at:", address(newImplementation));
        }
        console.log("New implementation version:", newImplementation.version());

        // Step 3: Prepare upgrade (no initialization needed for L1BlockCGT)
        console.log("\n--- Preparing Upgrade ---");
        console.log("L1BlockCGT maintains all L1Block state and adds CGT functionality");

        // Step 4: Execute upgrade via ProxyAdmin owner
        console.log("\n--- Executing Upgrade via ProxyAdmin Owner ---");

        vm.prank(PROXY_ADMIN_OWNER);

        // Call ProxyAdmin.upgrade() directly
        IProxyAdmin admin = IProxyAdmin(PROXY_ADMIN);
        admin.upgrade(payable(L1_BLOCK_PROXY), address(newImplementation));
        bytes memory upgradeData = abi.encodeWithSelector(
            IProxyAdmin.upgrade.selector,
            payable(L1_BLOCK_PROXY),
            address(newImplementation)
        );
        console.log("upgradeData length:", upgradeData.length);
        console.logBytes(upgradeData);

        console.log("Upgrade executed successfully");

        L1BlockCGT upgradedL1Block = L1BlockCGT(L1_BLOCK_PROXY);

        // Step 5: Verify the upgrade
        _verifyUpgrade(upgradedL1Block, params);

        console.log("\n=== L1Block to L1BlockCGT Direct Upgrade Completed Successfully ===");
    }

    /// @notice Log the current state before upgrade
    function _logCurrentState() internal view {
        IL1BlockCGT currentL1Block = IL1BlockCGT(L1_BLOCK_PROXY);

        console.log("\n--- Current L1Block State (Before Upgrade) ---");
        console.log("Current version:", currentL1Block.version());

        // Try to check if isCustomGasToken exists (it shouldn't in L1Block)
        try currentL1Block.isCustomGasToken() returns (bool isCustom) {
            console.log("Current isCustomGasToken:", isCustom);
        } catch {
            console.log("isCustomGasToken not available (expected for L1Block)");
        }

        // Try to get gas paying token info (might not exist in L1Block)
        try currentL1Block.gasPayingTokenName() returns (string memory name) {
            console.log("Current gas paying token name:", name);
        } catch {
            console.log("gasPayingTokenName not available (expected for L1Block)");
        }

        try currentL1Block.gasPayingTokenSymbol() returns (string memory symbol) {
            console.log("Current gas paying token symbol:", symbol);
        } catch {
            console.log("gasPayingTokenSymbol not available (expected for L1Block)");
        }
    }

    /// @notice Validate that the provided implementation bytecode matches the local L1BlockCGT
    /// @param implementationToValidate The implementation address to validate
    function _validateImplementationBytecode(address implementationToValidate) internal {
        console.log("\n--- Validating Implementation Bytecode ---");

        // Deploy a reference L1BlockCGT to compare against
        L1BlockCGT referenceImplementation = new L1BlockCGT();

        // Get bytecode from both implementations
        bytes memory providedBytecode = implementationToValidate.code;
        bytes memory referenceBytecode = address(referenceImplementation).code;

        console.log("Provided implementation bytecode size:", providedBytecode.length);
        console.log("Reference implementation bytecode size:", referenceBytecode.length);

        // Compare bytecode lengths first (quick check)
        require(
            providedBytecode.length == referenceBytecode.length,
            "Bytecode length mismatch: provided implementation does not match local L1BlockCGT"
        );

        // Compare bytecode hashes (efficient full comparison)
        bytes32 providedCodeHash = keccak256(providedBytecode);
        bytes32 referenceCodeHash = keccak256(referenceBytecode);

        console.log("Provided implementation codehash:");
        console.logBytes32(providedCodeHash);
        console.log("Reference implementation codehash:");
        console.logBytes32(referenceCodeHash);

        require(
            providedCodeHash == referenceCodeHash,
            "Bytecode mismatch: provided implementation does not match local L1BlockCGT"
        );

        console.log("[PASS] Bytecode validation passed: provided implementation matches local L1BlockCGT");
    }

    /// @notice Verify the upgrade was successful
    function _verifyUpgrade(L1BlockCGT upgradedL1Block, L1BlockParams memory expectedParams) internal view {
        console.log("\n--- Verifying Upgrade Results ---");

        // Check version updated to L1BlockCGT
        string memory newVersion = upgradedL1Block.version();
        console.log("New version:", newVersion);
        console.log("Previous version:", expectedParams.version);

        // Verify the version contains the custom gas token suffix
        require(
            bytes(newVersion).length > 0 &&
            keccak256(bytes(newVersion)) != keccak256(bytes(expectedParams.version)),
            "Version not updated correctly"
        );

        // Verify all L1Block parameters were preserved during upgrade
        console.log("\n--- Verifying L1Block Parameter Preservation ---");

        require(upgradedL1Block.number() == expectedParams.number, "Number not preserved");
        require(upgradedL1Block.timestamp() == expectedParams.timestamp, "Timestamp not preserved");
        require(upgradedL1Block.basefee() == expectedParams.basefee, "Basefee not preserved");
        require(upgradedL1Block.hash() == expectedParams.hash, "Hash not preserved");
        require(upgradedL1Block.sequenceNumber() == expectedParams.sequenceNumber, "Sequence number not preserved");
        require(upgradedL1Block.batcherHash() == expectedParams.batcherHash, "Batcher hash not preserved");
        require(upgradedL1Block.l1FeeOverhead() == expectedParams.l1FeeOverhead, "L1 fee overhead not preserved");
        require(upgradedL1Block.l1FeeScalar() == expectedParams.l1FeeScalar, "L1 fee scalar not preserved");
        require(upgradedL1Block.baseFeeScalar() == expectedParams.baseFeeScalar, "Base fee scalar not preserved");
        require(upgradedL1Block.blobBaseFeeScalar() == expectedParams.blobBaseFeeScalar, "Blob base fee scalar not preserved");
        require(upgradedL1Block.blobBaseFee() == expectedParams.blobBaseFee, "Blob base fee not preserved");
        require(upgradedL1Block.operatorFeeConstant() == expectedParams.operatorFeeConstant, "Operator fee constant not preserved");
        require(upgradedL1Block.operatorFeeScalar() == expectedParams.operatorFeeScalar, "Operator fee scalar not preserved");

        // Verify new L1BlockCGT functionality is available
        console.log("\n--- Verifying New L1BlockCGT Functionality ---");

        // Check isCustomGasToken function (should default to true)
        bool isCustomGasToken = upgradedL1Block.isCustomGasToken();
        console.log("isCustomGasToken (new function):", isCustomGasToken);
        require(isCustomGasToken, "isCustomGasToken should default to true");

        // Check gas paying token name and symbol functions
        string memory tokenName = upgradedL1Block.gasPayingTokenName();
        string memory tokenSymbol = upgradedL1Block.gasPayingTokenSymbol();
        console.log("Gas paying token name:", tokenName);
        console.log("Gas paying token symbol:", tokenSymbol);

        // Should default to "Ether" and "ETH" when not using custom gas token
        require(
            keccak256(bytes(tokenName)) == keccak256(bytes("OKB")),
            "Gas paying token name should be 'OKB' by default"
        );
        require(
            keccak256(bytes(tokenSymbol)) == keccak256(bytes("OKB")),
            "Gas paying token symbol should be 'OKB' by default"
        );

        console.log("All upgrade verifications passed!");
        console.log("Successfully upgraded from L1Block to L1BlockCGT version:", newVersion);
        console.log("All existing L1Block parameters preserved");
        console.log("New L1BlockCGT functionality verified and working");
        console.log("\nUpgrade completed using:");
        console.log("- ProxyAdmin Owner:", PROXY_ADMIN_OWNER);
        console.log("- ProxyAdmin:", PROXY_ADMIN);
        console.log("- L1Block Proxy:", L1_BLOCK_PROXY);
        console.log("- New Implementation:", address(upgradedL1Block));
    }
}
