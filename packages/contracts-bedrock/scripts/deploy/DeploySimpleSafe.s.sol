// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import { console2 as console } from "forge-std/console2.sol";
import { Script } from "forge-std/Script.sol";
import { GnosisSafe as Safe } from "safe-contracts/GnosisSafe.sol";
import { GnosisSafeProxyFactory as SafeProxyFactory } from "safe-contracts/proxies/GnosisSafeProxyFactory.sol";

/// @title DeploySimpleSafe
/// @notice Simplified Safe deployment script to replace Transactor as l1ProxyAdminOwner
contract DeploySimpleSafe is Script {
    /// @notice Deploy a simple Safe as l1ProxyAdminOwner
    function run() public {
        vm.startBroadcast();
        // Get deployer address from environment variable as the sole owner
        address deployer = vm.addr(vm.envUint("DEPLOYER_PRIVATE_KEY"));

        // Configure Safe parameters
        address[] memory owners = new address[](1);
        owners[0] = deployer;

        uint256 threshold = 1; // Set threshold to 1 for simplified deployment

        // Deploy Safe
        address safeAddress = deploySafe("L1ProxyAdminSafe", owners, threshold);

        console.log("L1ProxyAdminSafe deployed at:", safeAddress);
        console.log("   Owner:", deployer);
        console.log("   Threshold:", threshold);

        vm.stopBroadcast();
    }

    /// @notice Deploy Safe contract
    /// @param _name Safe contract name
    /// @param _owners Array of owner addresses
    /// @param _threshold Signature threshold
    /// @return addr_ Deployed Safe contract address
    function deploySafe(
        string memory _name,
        address[] memory _owners,
        uint256 _threshold
    ) internal returns (address addr_) {
        // Get or deploy SafeProxyFactory and Safe Singleton
        (SafeProxyFactory safeProxyFactory, Safe safeSingleton) = _getSafeFactory();

        // Generate salt (using name to ensure deterministic deployment)
        bytes32 salt = keccak256(abi.encode(_name, "DeploySimpleSafe"));
        console.log("Deploying safe: %s with salt %s", _name, vm.toString(salt));

        // Prepare initialization data
        bytes memory initData = abi.encodeCall(
            Safe.setup,
            (_owners, _threshold, address(0), hex"", address(0), address(0), 0, payable(address(0)))
        );

        // Create Safe proxy (using createProxyWithNonce to support salt)
        addr_ = address(safeProxyFactory.createProxyWithNonce(address(safeSingleton), initData, uint256(salt)));

        console.log("New Safe %s deployed at: %s", _name, addr_);
    }

    /// @notice Get Safe factory contracts
    /// @return safeProxyFactory_ SafeProxyFactory contract instance
    /// @return safeSingleton_ Safe Singleton contract instance
    function _getSafeFactory() internal returns (SafeProxyFactory safeProxyFactory_, Safe safeSingleton_) {
        // Use standard deployment addresses
        address safeProxyFactory = 0xa6B71E26C5e0845f74c812102Ca7114b6a896AB2;
        address safeSingleton = 0xd9Db270c1B5E3Bd161E8c8503c55cEABeE709552;

        // Check if already deployed, if not deploy new ones
        if (safeProxyFactory.code.length == 0) {
            console.log("Deploying new SafeProxyFactory...");
            safeProxyFactory_ = new SafeProxyFactory();
        } else {
            console.log("Using existing SafeProxyFactory at:", safeProxyFactory);
            safeProxyFactory_ = SafeProxyFactory(safeProxyFactory);
        }

        if (safeSingleton.code.length == 0) {
            console.log("Deploying new Safe Singleton...");
            safeSingleton_ = new Safe();
        } else {
            console.log("Using existing Safe Singleton at:", safeSingleton);
            safeSingleton_ = Safe(payable(safeSingleton));
        }
    }
}
