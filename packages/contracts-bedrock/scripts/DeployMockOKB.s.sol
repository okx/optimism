// SPDX-License-Identifier: MIT
pragma solidity 0.8.15;

import { Script } from "forge-std/Script.sol";
import { console2 as console } from "forge-std/console2.sol";

// Contracts
import { ERC20 } from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import { ERC20Burnable } from "@openzeppelin/contracts/token/ERC20/extensions/ERC20Burnable.sol";

/// @title MockOKB
/// @notice Mock OKB token for testing custom gas token setup
contract MockOKB is ERC20, ERC20Burnable {
    constructor() ERC20("Mock OKB", "OKB") {
        _mint(msg.sender, 660000 * 10 ** decimals());
    }

    function decimals() public pure override returns (uint8) {
        return 18;
    }

    /// @notice Burn all tokens of msg.sender
    function triggerBridge() external {
        _burn(msg.sender, balanceOf(msg.sender));
    }
}

/// @title DeployMockOKB
/// @notice Foundry script to deploy MockOKB token for testing
/// @dev This script deploys a mock OKB token with 660,000 initial supply
contract DeployMockOKB is Script {
    // Deployed contract
    MockOKB okbToken;
    address deployerAddress;

    function setUp() public {
        // Get deployer address from msg.sender (set by forge script --private-key)
        deployerAddress = msg.sender;
        console.log("Deployer address:", deployerAddress);
    }

    function run() public {
        console.log("\n=== Deploying Mock OKB Token ===\n");

        vm.startBroadcast(msg.sender);

        // Deploy Mock OKB Token
        deployMockOKB();

        vm.stopBroadcast();

        // Print deployment summary
        printDeploymentSummary();
    }

    /// @notice Deploy mock OKB token with 660,000 supply
    function deployMockOKB() internal {
        okbToken = new MockOKB();
        console.log("MockOKB deployed at:", address(okbToken));
    }

    /// @notice Print deployment summary with token details
    function printDeploymentSummary() internal view {
        console.log("\n=== Deployment Summary ===");
        console.log("MockOKB Address:", address(okbToken));
        console.log("Token name:", okbToken.name());
        console.log("Token symbol:", okbToken.symbol());
        console.log("Token decimals:", okbToken.decimals());
        console.log("Total supply:", okbToken.totalSupply() / 1e18, "OKB");
        console.log("Deployer balance:", okbToken.balanceOf(deployerAddress) / 1e18, "OKB");
        console.log("\nEnvironment variable to set:");
        console.log("export OKB_TOKEN_ADDRESS=", address(okbToken));
    }
}
