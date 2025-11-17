// SPDX-License-Identifier: MIT
pragma solidity 0.8.15;

import { Script } from "forge-std/Script.sol";
import { SuperchainConfig } from "src/L1/SuperchainConfig.sol";
import { ISuperchainConfig } from "interfaces/L1/ISuperchainConfig.sol";
import { IProxyAdmin } from "interfaces/universal/IProxyAdmin.sol";
import { IProxy } from "interfaces/universal/IProxy.sol";
import { console2 } from "forge-std/console2.sol";
import { DeployUtils } from "scripts/libraries/DeployUtils.sol";
import { Solarray } from "scripts/libraries/Solarray.sol";
import { Transactor } from "src/periphery/Transactor.sol";

/// @notice Script for transferring the guardian of the SuperchainConfig contract.
///         This script deploys a new SuperchainConfig implementation and upgrades the proxy
///         to point to it, reinitializing with a new guardian address.
contract TransferGuardian is Script {
    struct Input {
        /// @notice Address of the existing SuperchainConfig proxy
        address superchainConfigProxy;
        /// @notice Address of the SuperchainProxyAdmin
        address superchainProxyAdmin;
        /// @notice Address of the Transactor (owner of ProxyAdmin)
        address transactor;
        /// @notice New guardian address to set during reinitialization
        address newGuardian;
    }

    struct Output {
        /// @notice Address of the newly deployed SuperchainConfig implementation
        SuperchainConfig newSuperchainConfigImpl;
        /// @notice Address of the SuperchainConfig proxy (unchanged)
        ISuperchainConfig superchainConfigProxy;
        /// @notice Address of the SuperchainProxyAdmin (unchanged)
        IProxyAdmin superchainProxyAdmin;
    }

    bytes32 internal _salt = DeployUtils.DEFAULT_SALT;

    /// @notice The main entry point for the script
    function run() public returns (Output memory output_) {
        Input memory _input = Input({
            superchainConfigProxy: vm.envAddress("SUPERCHAIN_CONFIG_PROXY"),
            superchainProxyAdmin: vm.envAddress("PROXY_ADMIN"),
            transactor: vm.envAddress("TRANSACTOR"),
            newGuardian: vm.envAddress("NEW_GUARDIAN")
        });
        // Validate inputs
        _assertValidInput(_input);

        // Store the existing contracts
        output_.superchainConfigProxy = ISuperchainConfig(_input.superchainConfigProxy);
        output_.superchainProxyAdmin = IProxyAdmin(_input.superchainProxyAdmin);

        // Check existing implementation version using keccak256 hash comparison for string compatibility
        require(
            keccak256(bytes(output_.superchainConfigProxy.version())) == keccak256(bytes("2.4.0")),
            "TransferGuardian: existing implementation version must be 2.4.0"
        );

        vm.startBroadcast();
        // Deploy new implementation
        _deployNewSuperchainConfigImpl(output_);

        // Perform the upgrade and reinitialization
        _upgradeAndReinitialize(_input, output_);
        vm.stopBroadcast();

        // Validate outputs
        _assertValidOutput(_input, output_);

        return output_;
    }

    /// @notice Deploys a new SuperchainConfig implementation contract
    function _deployNewSuperchainConfigImpl(Output memory _output) internal {
        _output.newSuperchainConfigImpl = new SuperchainConfig();
    }

    /// @notice Upgrades the proxy to the new implementation and reinitializes
    function _upgradeAndReinitialize(Input memory _input, Output memory _output) internal {
        IProxyAdmin proxyAdmin = _output.superchainProxyAdmin;
        ISuperchainConfig proxy = _output.superchainConfigProxy;
        SuperchainConfig newImpl = _output.newSuperchainConfigImpl;
        Transactor transactor = Transactor(_input.transactor);

        // Verify ownership chain
        address proxyAdminOwner = proxyAdmin.owner();
        require(proxyAdminOwner == _input.transactor, "TransferGuardian: Transactor must be the owner of ProxyAdmin");

        // Encode the initialize() call data
        bytes memory initCalldata = abi.encodeCall(ISuperchainConfig.initialize, (_input.newGuardian));

        // Encode the ProxyAdmin.upgradeAndCall() call
        bytes memory upgradeAndCallData = abi.encodeWithSelector(
            IProxyAdmin.upgradeAndCall.selector, payable(address(proxy)), address(newImpl), initCalldata
        );

        // Call ProxyAdmin.upgradeAndCall() through Transactor.CALL()
        (bool success,) = transactor.CALL(
            address(proxyAdmin),
            upgradeAndCallData,
            0 // no ETH value
        );
        require(success, "TransferGuardian: Transactor CALL to ProxyAdmin.upgradeAndCall failed");
        console2.log("New SuperchainConfig implementation:", address(newImpl));
        console2.log("New Guardian:", _input.newGuardian);
    }

    /// @notice Validates the input parameters
    function _assertValidInput(Input memory _input) internal view {
        require(_input.superchainConfigProxy != address(0), "TransferGuardian: proxy not set");
        require(_input.superchainProxyAdmin != address(0), "TransferGuardian: proxyAdmin not set");
        require(_input.transactor != address(0), "TransferGuardian: transactor not set");
        require(_input.newGuardian != address(0), "TransferGuardian: newGuardian not set");

        // Verify contracts have code
        require(_input.superchainConfigProxy.code.length > 0, "TransferGuardian: proxy must have code");
        require(_input.superchainProxyAdmin.code.length > 0, "TransferGuardian: proxyAdmin must have code");
        require(_input.transactor.code.length > 0, "TransferGuardian: transactor must have code");
    }

    /// @notice Validates the output after upgrade
    function _assertValidOutput(Input memory _input, Output memory _output) internal {
        // Verify the proxy is now pointing to the new implementation
        vm.startPrank(address(0));
        address actualImpl = IProxy(payable(address(_output.superchainConfigProxy))).implementation();
        vm.stopPrank();
        require(actualImpl == address(_output.newSuperchainConfigImpl), "TransferGuardian: implementation mismatch");

        // Verify the guardian was updated
        ISuperchainConfig proxy = _output.superchainConfigProxy;
        require(proxy.guardian() == _input.newGuardian, "TransferGuardian: guardian not updated");

        // Verify the proxy is initialized
        DeployUtils.assertInitialized({ _contractAddress: address(proxy), _isProxy: true, _slot: 0, _offset: 0 });

        // Verify the implementation contract is not initialized with guardian
        require(
            _output.newSuperchainConfigImpl.guardian() == address(0), "TransferGuardian: impl guardian should be zero"
        );

        // Verify the implementation version
        require(
            keccak256(bytes(_output.newSuperchainConfigImpl.version())) == keccak256(bytes("2.5.0")),
            "TransferGuardian: implementation version must be 2.5.0"
        );
    }
}
