// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import { console2 as console } from "forge-std/console2.sol";
import { Script } from "forge-std/Script.sol";
import { GnosisSafe as Safe } from "safe-contracts/GnosisSafe.sol";
import { GnosisSafeProxyFactory as SafeProxyFactory } from "safe-contracts/proxies/GnosisSafeProxyFactory.sol";

/// @title DeploySimpleSafe
/// @notice Simplified Safe deployment script to replace Transactor as ProxyAdminOwner on both L1 and L2
contract DeploySimpleSafe is Script {
    /// @notice Deploy a 2/3 Safe as ProxyAdminOwner on both L1 and L2
    function run() public {
        vm.startBroadcast();

        // Configure Safe parameters
        address[] memory owners = new address[](3);
        owners[0] = 0x6eE7BDa7AF04F61ccf93aB4b8DB2289aBe76C6aA;
        owners[1] = 0xD3C6821DE67A5c0345EC5A41e4C83739f7043972;
        owners[2] = 0x11CAA37c9e9Da2621bB45Af77cB7debEE3881d2E;

        uint256 threshold = 2;

        // Deploy Safe
        address safeAddress = deploySafe("ProxyAdminSafe", owners, threshold);

        console.log("ProxyAdminSafe deployed at:", safeAddress);
        console.log("   Owners:");
        for (uint256 i = 0; i < owners.length; i++) {
            console.log("      [%d] %s", i, owners[i]);
        }
        console.log("   Threshold:", threshold);

        vm.stopBroadcast();
    }

    /// @notice Deploy Safe contract using CREATE2 for deterministic deployment
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

        // Generate salt from owners and name for deterministic deployment
        // Note: Same owners will produce same address across deployments
        bytes32 salt = keccak256(abi.encode(_name, _owners));
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

    /// @notice Get Safe factory contracts (v1.3.0)
    /// @dev Uses canonical Safe v1.3.0 deployment addresses.
    ///      These addresses are available on most EVM chains.
    ///      If not found, new instances will be deployed.
    ///      Ref: https://github.com/safe-global/safe-deployments
    /// @return safeProxyFactory_ SafeProxyFactory contract instance
    /// @return safeSingleton_ Safe Singleton contract instance
    function _getSafeFactory() internal returns (SafeProxyFactory safeProxyFactory_, Safe safeSingleton_) {
        // Canonical Safe v1.3.0 deployment addresses
        // These addresses are deterministic across most EVM-compatible chains
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
