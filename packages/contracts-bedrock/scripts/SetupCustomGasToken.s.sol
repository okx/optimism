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
import {IOptimismPortal2} from "interfaces/L1/IOptimismPortal2.sol";
import {IL1Block} from "interfaces/L2/IL1Block.sol";

// Libraries
import {Features} from "src/libraries/Features.sol";
import {GasPayingToken} from "src/libraries/GasPayingToken.sol";
import {LibString} from "@solady/utils/LibString.sol";
import {Predeploys} from "src/libraries/Predeploys.sol";

import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {ERC20Burnable} from "@openzeppelin/contracts/token/ERC20/extensions/ERC20Burnable.sol";

/// @title MockOKB
/// @notice Mock OKB token for testing custom gas token setup
contract MockOKB is ERC20, ERC20Burnable {
    constructor() ERC20("Mock OKB", "OKB") {
        _mint(msg.sender, 21000000 * 10 ** decimals());
    }

    function decimals() public pure override returns (uint8) {
        return 18;
    }

    /// @notice Burn all tokens of msg.sender
    function triggerBridge() external {
        _burn(msg.sender, balanceOf(msg.sender));
    }
}

/// @title SetupCustomGasToken
/// @notice Foundry script to set up and verify custom gas token configuration
/// @dev This script:
///      1. Deploys mock OKB token with triggerBridge functionality
///      2. Deploys OKBBurner implementation contract for minimal proxy pattern
///      3. Deploys DepositedOKBAdapter with burner implementation reference
///      4. Sets gas paying token in SystemConfig storage
///      5. Verifies all configurations on L1
///      6. Provides test function for deposit functionality
contract SetupCustomGasToken is Script {
    using stdJson for string;

    // Addresses to be loaded from deployment artifacts
    address systemConfigProxy;
    address optimismPortalProxy;
    address l1BlockAddress;
    address deployerAddress;

    // Deployed contracts
    MockOKB okbToken;
    OKBBurner burnerImplementation;
    DepositedOKBAdapter adapter;

    // Configuration
    bytes32 constant TOKEN_NAME = bytes32("Mock OKB");
    bytes32 constant TOKEN_SYMBOL = bytes32("OKB");
    uint8 constant TOKEN_DECIMALS = 18;

    function setUp() public {
        // Get deployer address from msg.sender (set by forge script --private-key)
        deployerAddress = msg.sender;
        console.log("Deployer address:", deployerAddress);

        // Parse addresses from environment variables
        systemConfigProxy = vm.envAddress("SYSTEM_CONFIG_PROXY_ADDRESS");
        optimismPortalProxy = vm.envAddress("OPTIMISM_PORTAL_PROXY_ADDRESS");

        console.log("SystemConfig Proxy:", systemConfigProxy);
        console.log("OptimismPortal Proxy:", optimismPortalProxy);

        // L1Block is a predeploy on L2 at a fixed address
        l1BlockAddress = Predeploys.L1_BLOCK_ATTRIBUTES;
        console.log("L1Block Address:", l1BlockAddress);
    }

    function run() public {
        console.log("\n=== Starting Custom Gas Token Setup ===\n");

        vm.startBroadcast(msg.sender);

        // Step 1: Deploy Mock OKB Token
        console.log("Step 1: Deploying Mock OKB Token...");
        deployMockOKB();

        // Step 2: Deploy OKBBurner Implementation
        console.log("\nStep 2: Deploying OKBBurner Implementation...");
        deployBurnerImplementation();

        // Step 3: Deploy DepositedOKBAdapter
        console.log("\nStep 3: Deploying DepositedOKBAdapter...");
        deployAdapter();

        // Step 4: Set gas paying token in SystemConfig storage
        console.log("\nStep 4: Setting gas paying token in SystemConfig storage...");
        setGasPayingToken();

        vm.stopBroadcast();

        // Step 5: Verify all configurations
        console.log("\n=== Verification Phase ===\n");
        verifyL1Configuration();
    }

    /// @notice Deploy mock OKB token with 21M supply
    function deployMockOKB() internal {
        okbToken = new MockOKB();
        console.log("  MockOKB deployed at:", address(okbToken));
        console.log("  Token name:", okbToken.name());
        console.log("  Token symbol:", okbToken.symbol());
        console.log("  Token decimals:", okbToken.decimals());
        console.log("  Total supply:", okbToken.totalSupply() / 1e18, "OKB");
        console.log("  Deployer balance:", okbToken.balanceOf(deployerAddress) / 1e18, "OKB");
    }

    /// @notice Deploy OKBBurner implementation contract
    function deployBurnerImplementation() internal {
        burnerImplementation = new OKBBurner(address(okbToken), address(0)); // adapter address will be set later
        console.log("  OKBBurner Implementation deployed at:", address(burnerImplementation));
        console.log("  Burner OKB token:", address(burnerImplementation.OKB()));
        console.log("  Burner adapter (placeholder):", address(burnerImplementation.ADAPTER()));
    }

    /// @notice Deploy DepositedOKBAdapter
    function deployAdapter() internal {
        adapter = new DepositedOKBAdapter(
            address(okbToken),
            payable(optimismPortalProxy),
            address(burnerImplementation)
        );
        console.log("  DepositedOKBAdapter deployed at:", address(adapter));
        console.log("  Adapter name:", adapter.name());
        console.log("  Adapter symbol:", adapter.symbol());
        console.log("  OKB token:", address(adapter.OKB()));
        console.log("  Portal:", address(adapter.PORTAL()));
        console.log("  Burner implementation:", adapter.BURNER_IMPLEMENTATION());
    }

    /// @notice Set gas paying token in SystemConfig storage
    /// @dev This writes to the GasPayingToken storage slots directly
    function setGasPayingToken() internal {
        ISystemConfig systemConfig = ISystemConfig(systemConfigProxy);
        // adapter is the gas paying token
        systemConfig.setGasPayingToken(address(adapter), TOKEN_DECIMALS, TOKEN_NAME, TOKEN_SYMBOL);
    }

    /// @notice Verify L1 configuration
    function verifyL1Configuration() internal view {
        console.log("Step 5: Verifying L1 Configuration...\n");

        ISystemConfig systemConfig = ISystemConfig(systemConfigProxy);
        IOptimismPortal2 portal = IOptimismPortal2(payable(optimismPortalProxy));

        // Check 1: SystemConfig isCustomGasToken
        bool isCustomGasToken = systemConfig.isCustomGasToken();
        console.log("  [CHECK 1] SystemConfig.isCustomGasToken():", isCustomGasToken);
        require(isCustomGasToken, "FAILED: SystemConfig custom gas token not enabled");

        // Check 2: SystemConfig gasPayingToken
        (address tokenAddr, uint8 decimals) = systemConfig.gasPayingToken();
        console.log("  [CHECK 2] SystemConfig.gasPayingToken():");
        console.log("    Address:", tokenAddr);
        console.log("    Decimals:", decimals);
        require(tokenAddr == address(adapter), "FAILED: Token address mismatch");
        require(decimals == 18, "FAILED: Token decimals must be 18");

        // Check 3: OptimismPortal isCustomGasToken
        bool portalCGT = portal.isCustomGasToken();
        console.log("  [CHECK 3] OptimismPortal.isCustomGasToken():", portalCGT);
        require(portalCGT, "FAILED: Portal custom gas token not enabled");

        // Check 4: DepositedOKBAdapter configuration
        console.log("  [CHECK 4] DepositedOKBAdapter configuration:");
        console.log("    OKB Token:", address(adapter.OKB()));
        console.log("    Portal:", address(adapter.PORTAL()));
        require(address(adapter.OKB()) == address(okbToken), "FAILED: Adapter OKB mismatch");
        require(address(adapter.PORTAL()) == optimismPortalProxy, "FAILED: Adapter portal mismatch");

        // Check 5: OKBBurner Implementation configuration
        console.log("  [CHECK 5] OKBBurner Implementation configuration:");
        console.log("    OKB Token:", address(burnerImplementation.OKB()));
        console.log("    Adapter Address:", address(burnerImplementation.ADAPTER()));
        require(address(burnerImplementation.OKB()) == address(okbToken), "FAILED: Burner OKB mismatch");

        // Check 6: Adapter burner implementation reference
        console.log("  [CHECK 6] Adapter burner implementation:");
        console.log("    Burner Implementation:", adapter.BURNER_IMPLEMENTATION());
        require(adapter.BURNER_IMPLEMENTATION() == address(burnerImplementation), "FAILED: Adapter burner implementation mismatch");

        // Check 7: Adapter approval to portal
        uint256 allowance = adapter.allowance(address(adapter), optimismPortalProxy);
        console.log("  [CHECK 7] Adapter approval to Portal:", allowance);
        require(allowance == type(uint256).max, "FAILED: Adapter should pre-approve portal");

        console.log("\n  ✅ All L1 configuration checks passed!");
    }
}
