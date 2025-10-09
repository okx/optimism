// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import { Script } from "forge-std/Script.sol";
import { console2 as console } from "forge-std/console2.sol";

import { GnosisSafe as Safe } from "safe-contracts/GnosisSafe.sol";
import { GnosisSafeProxyFactory as SafeProxyFactory } from "safe-contracts/proxies/GnosisSafeProxyFactory.sol";
import { OwnerManager } from "safe-contracts/base/OwnerManager.sol";
import { ModuleManager } from "safe-contracts/base/ModuleManager.sol";
import { GuardManager } from "safe-contracts/base/GuardManager.sol";
import { Enum as SafeOps } from "safe-contracts/common/Enum.sol";

import { IProxyAdmin } from "interfaces/universal/IProxyAdmin.sol";
import { LivenessGuard } from "src/safe/LivenessGuard.sol";
import { LivenessModule } from "src/safe/LivenessModule.sol";
import { Transactor } from "src/periphery/Transactor.sol";

/// @notice Configuration for a Safe
struct SafeConfig {
    uint256 threshold;
    address[] owners;
}

/// @notice Configuration for the Liveness Module
struct LivenessModuleConfig {
    uint256 livenessInterval;
    uint256 thresholdPercentage;
    uint256 minOwners;
    address fallbackOwner;
}

/// @notice Configuration for the Security Council Safe.
struct SecurityCouncilConfig {
    SafeConfig safeConfig;
    LivenessModuleConfig livenessModuleConfig;
}

/// @title TransferProxyAdminL1AndL2
/// @notice Comprehensive script to deploy security council governance and transfer L1 ProxyAdmin ownership.
///         This script performs the following:
///         1. Deploys FoundationUpgradeSafe (5/7 multisig)
///         2. Deploys SecurityCouncilSafe (10/13 multisig) with LivenessModule and LivenessGuard
///         3. Deploys a 2/2 multisig (ProxyAdminOwnerSafe) with:
///            - SecurityCouncilSafe
///            - FoundationUpgradeSafe
///         4. Transfers L1 ProxyAdmin ownership to the 2/2 multisig via Transactor
///
/// @dev For L2 ProxyAdmin ownership transfer, use the separate TransferProxyAdminL2.s.sol script
///      which should be executed directly on L2 after this script completes.
contract TransferProxyAdminL1AndL2 is Script {
    // Deployed contract addresses (stored as state variables)
    address public foundationUpgradeSafe;
    address public securityCouncilSafe;
    address public proxyAdminOwnerSafe;
    address public livenessGuard;
    address public livenessModule;
    address public safeProxyFactory;
    address public safeSingleton;

    /// @notice Modifier that wraps a function in broadcasting.
    modifier broadcast() {
        vm.startBroadcast(msg.sender);
        _;
        vm.stopBroadcast();
    }

    /// @notice Main execution function.
    function run() public {
        console.log("====================================================");
        console.log("Starting L1 ProxyAdmin Ownership Transfer");
        console.log("====================================================\n");

        // Step 1: Deploy FoundationUpgradeSafe (needed as fallback owner for LivenessModule)
        console.log("Step 1: Deploying FoundationUpgradeSafe...");
        foundationUpgradeSafe = deployFoundationUpgradeSafe();
        console.log("  FoundationUpgradeSafe deployed at:", foundationUpgradeSafe);
        console.log();

        // Step 2: Deploy SecurityCouncilSafe (with deployer, threshold 1)
        console.log("Step 2: Deploying SecurityCouncilSafe...");
        securityCouncilSafe = deploySecurityCouncilSafe();
        console.log("  SecurityCouncilSafe deployed at:", securityCouncilSafe);
        console.log();

        // Step 3: Configure SecurityCouncilSafe with LivenessModule and LivenessGuard
        console.log("Step 3: Configuring SecurityCouncilSafe with Liveness Module/Guard...");
        configureSecurityCouncilSafe();
        console.log("  SecurityCouncilSafe configured successfully");
        console.log();

        // Step 4: Deploy 2/2 multisig with SecurityCouncilSafe and FoundationUpgradeSafe as owners
        console.log("Step 4: Deploying 2/2 ProxyAdminOwnerSafe...");
        proxyAdminOwnerSafe = deployProxyAdminOwnerSafe();
        console.log("  ProxyAdminOwnerSafe deployed at:", proxyAdminOwnerSafe);
        console.log();

        // Step 5: Transfer L1 ProxyAdmin ownership to the 2/2 multisig
        console.log("Step 5: Transferring L1 ProxyAdmin ownership...");
        transferL1ProxyAdminOwnership(proxyAdminOwnerSafe);
        console.log();

        console.log("====================================================");
        console.log("L1 ProxyAdmin Ownership Transfer Complete");
        console.log("====================================================\n");
    }

    /// @notice Returns a SafeConfig similar to that of the Foundation Safe on Mainnet.
    function _getExampleFoundationConfig() internal returns (SafeConfig memory safeConfig_) {
        address[] memory exampleFoundationOwners = new address[](7);
        for (uint256 i; i < exampleFoundationOwners.length; i++) {
            exampleFoundationOwners[i] = makeAddr(string.concat("fnd-", vm.toString(i)));
        }
        safeConfig_ = SafeConfig({ threshold: 5, owners: exampleFoundationOwners });
    }

    /// @notice Deploys a Safe with a configuration similar to that of the Foundation Safe on Mainnet.
    function deployFoundationUpgradeSafe() public broadcast returns (address addr_) {
        SafeConfig memory exampleFoundationConfig = _getExampleFoundationConfig();
        addr_ = deploySafe({
            _name: "FoundationUpgradeSafe",
            _owners: exampleFoundationConfig.owners,
            _threshold: exampleFoundationConfig.threshold,
            _keepDeployer: false
        });
    }

    /// @notice Returns a SafeConfig similar to that of the Security Council Safe on Mainnet.
    function _getExampleCouncilConfig() internal returns (SecurityCouncilConfig memory councilConfig_) {
        address[] memory exampleCouncilOwners = new address[](13);
        for (uint256 i; i < exampleCouncilOwners.length; i++) {
            exampleCouncilOwners[i] = makeAddr(string.concat("sc-", vm.toString(i)));
        }
        SafeConfig memory safeConfig = SafeConfig({ threshold: 10, owners: exampleCouncilOwners });
        councilConfig_ = SecurityCouncilConfig({
            safeConfig: safeConfig,
            livenessModuleConfig: LivenessModuleConfig({
                livenessInterval: 14 weeks,
                thresholdPercentage: 75,
                minOwners: 8,
                fallbackOwner: foundationUpgradeSafe
            })
        });
    }

    /// @notice Deploy a Security Council Safe with deployer and threshold 1 for initial configuration.
    function deploySecurityCouncilSafe() public broadcast returns (address addr_) {
        // Deploy the safe with the extra deployer key, and keep the threshold at 1 to allow for further setup.
        SecurityCouncilConfig memory exampleCouncilConfig = _getExampleCouncilConfig();
        addr_ = payable(
            deploySafe({
                _name: "SecurityCouncilSafe",
                _owners: exampleCouncilConfig.safeConfig.owners,
                _threshold: 1,
                _keepDeployer: true
            })
        );
    }

    /// @notice Deploy a LivenessGuard for use on the Security Council Safe.
    ///         Note this function does not have the broadcast modifier.
    function deployLivenessGuard() internal returns (address) {
        Safe councilSafe = Safe(payable(securityCouncilSafe));
        livenessGuard = address(new LivenessGuard(councilSafe));
        console.log("  New LivenessGuard deployed at %s", livenessGuard);
        return livenessGuard;
    }

    /// @notice Deploy a LivenessModule for use on the Security Council Safe
    ///         Note this function does not have the broadcast modifier.
    function deployLivenessModule() internal returns (address) {
        Safe councilSafe = Safe(payable(securityCouncilSafe));
        LivenessModuleConfig memory livenessModuleConfig = _getExampleCouncilConfig().livenessModuleConfig;

        livenessModule = address(
            new LivenessModule({
                _safe: councilSafe,
                _livenessGuard: LivenessGuard(livenessGuard),
                _livenessInterval: livenessModuleConfig.livenessInterval,
                _thresholdPercentage: livenessModuleConfig.thresholdPercentage,
                _minOwners: livenessModuleConfig.minOwners,
                _fallbackOwner: livenessModuleConfig.fallbackOwner
            })
        );
        console.log("  New LivenessModule deployed at %s", livenessModule);
        return livenessModule;
    }

    /// @notice Configure the Security Council Safe with the LivenessModule and LivenessGuard.
    function configureSecurityCouncilSafe() internal broadcast {
        SecurityCouncilConfig memory exampleCouncilConfig = _getExampleCouncilConfig();
        Safe safe = Safe(payable(securityCouncilSafe));

        // Deploy and add the Liveness Guard.
        address guard = deployLivenessGuard();
        _callViaSafe({ _safe: safe, _target: address(safe), _data: abi.encodeCall(GuardManager.setGuard, (guard)) });
        console.log("  LivenessGuard setup on SecurityCouncilSafe");

        // Deploy and add the Liveness Module.
        address module = deployLivenessModule();
        _callViaSafe({ _safe: safe, _target: address(safe), _data: abi.encodeCall(ModuleManager.enableModule, (module)) });
        console.log("  LivenessModule enabled on SecurityCouncilSafe");

        // Finalize configuration by removing the additional deployer key.
        removeDeployerFromSafe({ _safe: safe, _newThreshold: exampleCouncilConfig.safeConfig.threshold });

        address[] memory owners = safe.getOwners();
        require(
            safe.getThreshold() == LivenessModule(module).getRequiredThreshold(owners.length),
            "TransferProxyAdminL1AndL2: safe threshold must be equal to the LivenessModule's required threshold"
        );
    }

    /// @notice Make a call from the Safe contract to an arbitrary address with arbitrary data
    function _callViaSafe(Safe _safe, address _target, bytes memory _data) internal {
        // This is the signature format used when the caller is also the signer.
        bytes memory signature = abi.encodePacked(uint256(uint160(msg.sender)), bytes32(0), uint8(1));

        _safe.execTransaction({
            to: _target,
            value: 0,
            data: _data,
            operation: SafeOps.Operation.Call,
            safeTxGas: 0,
            baseGas: 0,
            gasPrice: 0,
            gasToken: address(0),
            refundReceiver: payable(address(0)),
            signatures: signature
        });
    }

    /// @notice If the keepDeployer option was used with deploySafe(), this function can be used to remove the deployer.
    ///         Note this function does not have the broadcast modifier.
    function removeDeployerFromSafe(Safe _safe, uint256 _newThreshold) internal {
        // The sentinel address is used to mark the start and end of the linked list of owners in the Safe.
        address sentinelOwners = address(0x1);

        // Because deploySafe() always adds msg.sender first (if keepDeployer is true), we know that the previousOwner
        // will be sentinelOwners.
        _callViaSafe({
            _safe: _safe,
            _target: address(_safe),
            _data: abi.encodeCall(OwnerManager.removeOwner, (sentinelOwners, msg.sender, _newThreshold))
        });
        console.log("  Removed deployer and set threshold to %d", _newThreshold);
    }

    /// @notice Gets the address of the SafeProxyFactory and Safe singleton for use in deploying a new GnosisSafe.
    function _getSafeFactory() internal returns (SafeProxyFactory safeProxyFactory_, Safe safeSingleton_) {
        // Return cached values if already deployed
        if (safeProxyFactory != address(0)) {
            return (SafeProxyFactory(safeProxyFactory), Safe(payable(safeSingleton)));
        }

        // These are the standard create2 deployed contracts. First we'll check if they are deployed,
        // if not we'll deploy new ones, though not at these addresses.
        address safeProxyFactoryAddr = 0xa6B71E26C5e0845f74c812102Ca7114b6a896AB2;
        address safeSingletonAddr = 0xd9Db270c1B5E3Bd161E8c8503c55cEABeE709552;

        if (safeProxyFactoryAddr.code.length == 0) {
            safeProxyFactory_ = new SafeProxyFactory();
            safeProxyFactory = address(safeProxyFactory_);
        } else {
            safeProxyFactory_ = SafeProxyFactory(safeProxyFactoryAddr);
            safeProxyFactory = safeProxyFactoryAddr;
        }

        if (safeSingletonAddr.code.length == 0) {
            safeSingleton_ = new Safe();
            safeSingleton = address(safeSingleton_);
        } else {
            safeSingleton_ = Safe(payable(safeSingletonAddr));
            safeSingleton = safeSingletonAddr;
        }

        console.log("SafeProxyFactory: %s", safeProxyFactory);
        console.log("SafeSingleton:    %s", safeSingleton);
    }

    /// @notice Deploy a new Safe contract.
    /// @param _name The name of the Safe to deploy (used for salt).
    /// @param _owners The owners of the Safe.
    /// @param _threshold The threshold of the Safe.
    /// @param _keepDeployer Whether or not the deployer address will be added as an owner of the Safe.
    function deploySafe(
        string memory _name,
        address[] memory _owners,
        uint256 _threshold,
        bool _keepDeployer
    )
        internal
        returns (address addr_)
    {
        bytes32 salt = keccak256(abi.encode(_name, block.chainid, msg.sender));
        console.log("Deploying safe: %s", _name);
        (SafeProxyFactory factory, Safe singleton) = _getSafeFactory();

        if (_keepDeployer) {
            address[] memory expandedOwners = new address[](_owners.length + 1);
            // By always adding msg.sender first we know that the previousOwner will be SENTINEL_OWNERS, which makes it
            // easier to call removeOwner later.
            expandedOwners[0] = msg.sender;
            for (uint256 i = 0; i < _owners.length; i++) {
                expandedOwners[i + 1] = _owners[i];
            }
            _owners = expandedOwners;
        }

        bytes memory initData = abi.encodeCall(
            Safe.setup, (_owners, _threshold, address(0), hex"", address(0), address(0), 0, payable(address(0)))
        );
        addr_ = address(factory.createProxyWithNonce(address(singleton), initData, uint256(salt)));

        console.log("  Deployed at: %s", addr_);
    }

    /// @notice Deploy a 2/2 multisig Safe with SecurityCouncilSafe and FoundationUpgradeSafe as owners.
    /// @return addr_ The address of the deployed ProxyAdminOwnerSafe.
    function deployProxyAdminOwnerSafe() internal broadcast returns (address addr_) {
        console.log("  Creating 2/2 multisig with owners:");
        console.log("    - SecurityCouncilSafe:    %s", securityCouncilSafe);
        console.log("    - FoundationUpgradeSafe:  %s", foundationUpgradeSafe);

        // Create the owners array
        address[] memory owners = new address[](2);
        owners[0] = securityCouncilSafe;
        owners[1] = foundationUpgradeSafe;

        // Deploy the 2/2 multisig
        addr_ = deploySafe({ _name: "ProxyAdminOwnerSafe", _owners: owners, _threshold: 2, _keepDeployer: false });

        // Verify the safe configuration
        Safe safe = Safe(payable(addr_));
        require(safe.getThreshold() == 2, "TransferProxyAdminL1AndL2: threshold must be 2");
        require(safe.getOwners().length == 2, "TransferProxyAdminL1AndL2: must have 2 owners");

        console.log("  Safe threshold: 2/2");
    }

    /// @notice Transfer L1 ProxyAdmin ownership to the ProxyAdminOwnerSafe via Transactor.
    /// @param _newOwner The address of the new ProxyAdmin owner (ProxyAdminOwnerSafe).
    function transferL1ProxyAdminOwnership(address _newOwner) internal broadcast {
        // Read addresses from environment variables
        address proxyAdminAddr = vm.envAddress("PROXY_ADMIN");
        address transactorAddr = vm.envAddress("TRANSACTOR");

        console.log("  Reading addresses from environment:");
        console.log("    PROXY_ADMIN: %s", proxyAdminAddr);
        console.log("    TRANSACTOR:  %s", transactorAddr);
        console.log();

        IProxyAdmin proxyAdmin = IProxyAdmin(proxyAdminAddr);
        Transactor transactor = Transactor(transactorAddr);

        // Get current owner
        address currentOwner = proxyAdmin.owner();
        console.log("  Current L1 ProxyAdmin owner: %s", currentOwner);
        console.log("  New L1 ProxyAdmin owner:     %s", _newOwner);

        require(currentOwner != _newOwner, "TransferProxyAdminL1AndL2: new owner is already the owner");

        // Generate calldata for transferOwnership(address)
        bytes memory transferOwnershipCalldata = abi.encodeCall(IProxyAdmin.transferOwnership, (_newOwner));
        console.log("  transferOwnership calldata generated");

        // Call transferOwnership through Transactor.CALL
        // Transactor.CALL(address _target, bytes memory _data, uint256 _value)
        (bool success,) = transactor.CALL(proxyAdminAddr, transferOwnershipCalldata, 0);
        require(success, "TransferProxyAdminL1AndL2: Transactor.CALL failed");

        // Verify the transfer
        address actualOwner = proxyAdmin.owner();
        require(actualOwner == _newOwner, "TransferProxyAdminL1AndL2: L1 ownership transfer failed");

        console.log("  L1 ProxyAdmin ownership successfully transferred");
    }
}
