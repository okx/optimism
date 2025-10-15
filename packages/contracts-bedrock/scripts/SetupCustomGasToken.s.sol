// SPDX-License-Identifier: MIT
pragma solidity 0.8.15;

import {Script} from "forge-std/Script.sol";
import {console2 as console} from "forge-std/console2.sol";
import {stdJson} from "forge-std/StdJson.sol";

// Contracts
import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {DepositedOKBAdapter} from "src/L1/DepositedOKBAdapter.sol";
import {OKBBurner} from "src/L1/OKBBurner.sol";

// Interfaces
import {IOKB} from "interfaces/L1/IOKB.sol";
import {ISystemConfig} from "interfaces/L1/ISystemConfig.sol";
import {Constants} from "src/libraries/Constants.sol";
import {IOptimismPortal2} from "interfaces/L1/IOptimismPortal2.sol";
import {IL1Block} from "interfaces/L2/IL1Block.sol";

// Libraries
import {Features} from "src/libraries/Features.sol";
import {GasPayingToken} from "src/libraries/GasPayingToken.sol";
import {LibString} from "@solady/utils/LibString.sol";
import {Predeploys} from "src/libraries/Predeploys.sol";


/// @title SetupCustomGasToken
/// @notice Foundry script to set up and verify custom gas token configuration
/// @dev This script:
///      1. Reads OKB token address from environment variable
///      2. Deploys OKBBurner implementation contract for minimal proxy pattern
///      3. Deploys DepositedOKBAdapter with burner implementation reference
///      4. Sets gas paying token in SystemConfig storage
///      5. Verifies all configurations on L1
contract SetupCustomGasToken is Script {
    using stdJson for string;

    // Addresses to be loaded from deployment artifacts
    address systemConfigProxy;
    address optimismPortalProxy;
    address deployerAddress;
    address okbTokenAddress;

    // Deployed contracts
    IOKB okbToken;
    OKBBurner burnerImplementation;
    DepositedOKBAdapter adapter;

    function setUp() public {
        // Get deployer address from msg.sender (set by forge script --private-key)
        deployerAddress = msg.sender;
        console.log("Deployer address:", deployerAddress);

        // Parse addresses from environment variables
        systemConfigProxy = vm.envAddress("SYSTEM_CONFIG_PROXY_ADDRESS");
        optimismPortalProxy = vm.envAddress("OPTIMISM_PORTAL_PROXY_ADDRESS");
        okbTokenAddress = vm.envAddress("OKB_TOKEN_ADDRESS");

        console.log("SystemConfig Proxy:", systemConfigProxy);
        console.log("OptimismPortal Proxy:", optimismPortalProxy);
        console.log("OKB Token Address:", okbTokenAddress);

        // Initialize OKB token interface
        okbToken = IOKB(okbTokenAddress);
    }

    function run() public {
        console.log("\n=== Starting Custom Gas Token Setup ===\n");

        preCheck();

        vm.startBroadcast(msg.sender);

        deployBurnerImplementation();

        deployAdapter();

        setGasPayingToken();

        vm.stopBroadcast();

        postCheck();
    }

    /// @notice Pre-check L1 Configuration
    function preCheck() internal view {
        console.log("Pre-check L1 Configuration...\n");
        ISystemConfig systemConfig = ISystemConfig(systemConfigProxy);
        bool isCustomGasToken = systemConfig.isCustomGasToken();
        require(isCustomGasToken, "FAILED: SystemConfig custom gas token not enabled");

        (address tokenAddr, uint8 decimals) = systemConfig.gasPayingToken();
        string memory name = systemConfig.gasPayingTokenName();
        string memory symbol = systemConfig.gasPayingTokenSymbol();
        console.log("SystemConfig.gasPayingToken():");
        console.log("    Address:", tokenAddr);
        console.log("    Decimals:", decimals);
        console.log("    Name:", name);
        console.log("    Symbol:", symbol);
        require(tokenAddr == Constants.ETHER, "FAILED: GasPayingToken already set");
    }

    /// @notice Deploy OKBBurner implementation contract
    function deployBurnerImplementation() internal {
        burnerImplementation = new OKBBurner(okbTokenAddress); // adapter address will be set later
        console.log("  OKBBurner Implementation deployed at:", address(burnerImplementation));
    }

    /// @notice Deploy DepositedOKBAdapter
    function deployAdapter() internal {
        adapter = new DepositedOKBAdapter(
            okbTokenAddress,
            payable(optimismPortalProxy),
            address(burnerImplementation)
        );
        console.log("  DepositedOKBAdapter deployed at:", address(adapter));
    }

    /// @notice Set gas paying token in SystemConfig storage
    /// @dev This writes to the GasPayingToken storage slots directly
    function setGasPayingToken() internal {
        ISystemConfig systemConfig = ISystemConfig(systemConfigProxy);
        // adapter is the gas paying token
        // Convert string to bytes32 for SystemConfig function
        bytes32 nameBytes32 = bytes32(bytes(okbToken.name()));
        bytes32 symbolBytes32 = bytes32(bytes(okbToken.symbol()));
        systemConfig.setGasPayingToken(address(adapter), okbToken.decimals(), nameBytes32, symbolBytes32);
    }

    /// @notice Post-check L1 configuration
    function postCheck() internal view {
        console.log("\nPost-check L1 Configuration...\n");

        ISystemConfig systemConfig = ISystemConfig(systemConfigProxy);
        // Check SystemConfig gasPayingToken
        (address tokenAddr, uint8 decimals) = systemConfig.gasPayingToken();
        string memory name = systemConfig.gasPayingTokenName();
        string memory symbol = systemConfig.gasPayingTokenSymbol();
        console.log("SystemConfig.gasPayingToken():");
        console.log("    Address:", tokenAddr);
        console.log("    Decimals:", decimals);
        console.log("    Name:", name);
        console.log("    Symbol:", symbol);
        require(tokenAddr == address(adapter), "FAILED: Token address mismatch");
        require(decimals == okbToken.decimals(), "FAILED: Token decimals mismatch");
        require(keccak256(abi.encodePacked(name)) == keccak256(abi.encodePacked(okbToken.name())), "FAILED: Token name mismatch");
        require(keccak256(abi.encodePacked(symbol)) == keccak256(abi.encodePacked(okbToken.symbol())), "FAILED: Token symbol mismatch");

        // Check DepositedOKBAdapter configuration
        require(address(adapter.OKB()) == okbTokenAddress, "FAILED: Adapter OKB mismatch");
        require(address(adapter.PORTAL()) == optimismPortalProxy, "FAILED: Adapter portal mismatch");

        // Check OKBBurner Implementation configuration
        require(address(burnerImplementation.OKB()) == okbTokenAddress, "FAILED: Burner OKB mismatch");

        // Check Adapter burner implementation reference
        require(adapter.BURNER_IMPLEMENTATION() == address(burnerImplementation), "FAILED: Adapter burner implementation mismatch");

        // Check Adapter approval to portal
        uint256 allowance = adapter.allowance(address(adapter), optimismPortalProxy);
        console.log("  [CHECK 7] Adapter approval to Portal:", allowance);
        require(allowance == 0, "FAILED: Adapter should not pre-approve portal");
    }
}
