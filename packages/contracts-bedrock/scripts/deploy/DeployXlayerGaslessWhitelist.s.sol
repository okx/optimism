// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import { Script, console2 } from "forge-std/Script.sol";
import { TransparentUpgradeableProxy } from "@openzeppelin/contracts-v5/proxy/transparent/TransparentUpgradeableProxy.sol";

interface IDeployFactory {
    function deploy(bytes memory initCode, bytes32 salt) external returns (address payable createdContract);

    function getAddress(bytes memory initCode, bytes32 salt) external view returns (address);
}

/// @dev Minimal view of the GaslessWhitelist implementation. The implementation itself is pinned to
///      solc 0.8.15 and lives in its own compilation unit, so it is referenced by interface + by
///      precompiled artifact (vm.getCode) instead of being imported — importing it would force this
///      0.8.28 + OZ-v5 script unit to also satisfy 0.8.15, which is impossible.
interface IGaslessWhitelist {
    function initialize(address owner) external;
    function owner() external view returns (address);
    function gaslessEnabled() external view returns (bool);
}

interface IOwner {
    function owner() external view returns (address);
}

contract DeployGaslessWhitelist is Script {
    IDeployFactory private constant DEPLOY_FACTORY = IDeployFactory(0xFaC897544659Fb136C064d5428947f5BC9cC1Fa2);

    bytes32 private constant ERC1967_ADMIN_SLOT = bytes32(uint256(keccak256("eip1967.proxy.admin")) - 1);

    error UnexpectedImplementationAddress(address expected, address actual);
    error UnexpectedProxyAddress(address expected, address actual);
    error UnexpectedWhitelistOwner(address expected, address actual);
    error UnexpectedProxyAdminOwner(address expected, address actual);

    function run() external returns (address implementation, address proxy) {
        // `owner` becomes BOTH the ProxyAdmin owner (OZ v5 auto-deploys a ProxyAdmin owned by the
        // proxy constructor's `initialOwner`) and the GaslessWhitelist owner. The broadcaster MUST
        // equal `owner`, because initialize() asserts msg.sender == ProxyAdmin owner (see below).
        address owner = vm.envOr("OWNER", msg.sender);
        bytes32 implementationSalt = vm.envOr("IMPLEMENTATION_SALT", keccak256("GaslessWhitelist.implementation"));
        bytes32 proxySalt = vm.envOr("PROXY_SALT", keccak256("GaslessWhitelist.proxy"));

        // Implementation creation code from the precompiled artifact (solc 0.8.15, separate unit).
        bytes memory implementationInitCode = vm.getCode("XlayerGaslessWhitelist.sol:GaslessWhitelist");
        address predictedImplementation = DEPLOY_FACTORY.getAddress(implementationInitCode, implementationSalt);

        // Deploy the proxy WITHOUT init data. GaslessWhitelist.initialize() runs
        // _assertOnlyProxyAdminOrProxyAdminOwner(), but during the proxy constructor the EIP-1967
        // admin slot is not set yet and msg.sender is the deploy factory, so an in-constructor
        // initialize would revert. We deploy bare and initialize() afterwards as `owner`.
        bytes memory proxyInitCode = abi.encodePacked(
            type(TransparentUpgradeableProxy).creationCode, abi.encode(predictedImplementation, owner, bytes(""))
        );
        address predictedProxy = DEPLOY_FACTORY.getAddress(proxyInitCode, proxySalt);

        vm.startBroadcast();

        implementation = DEPLOY_FACTORY.deploy(implementationInitCode, implementationSalt);
        proxy = DEPLOY_FACTORY.deploy(proxyInitCode, proxySalt);
        // Initialize through the proxy. Caller (broadcaster) is `owner`, which equals the auto-
        // deployed ProxyAdmin's owner, so _assertOnlyProxyAdminOrProxyAdminOwner() passes. Because
        // `owner` is not the proxy admin (the ProxyAdmin contract is), the transparent proxy
        // forwards this call to the implementation.
        IGaslessWhitelist(proxy).initialize(owner);

        vm.stopBroadcast();

        if (implementation != predictedImplementation) {
            revert UnexpectedImplementationAddress(predictedImplementation, implementation);
        }
        if (proxy != predictedProxy) {
            revert UnexpectedProxyAddress(predictedProxy, proxy);
        }

        address proxyAdmin = address(uint160(uint256(vm.load(proxy, ERC1967_ADMIN_SLOT))));
        address whitelistOwner = IGaslessWhitelist(proxy).owner();
        address proxyAdminOwner = IOwner(proxyAdmin).owner();

        if (whitelistOwner != owner) {
            revert UnexpectedWhitelistOwner(owner, whitelistOwner);
        }
        if (proxyAdminOwner != owner) {
            revert UnexpectedProxyAdminOwner(owner, proxyAdminOwner);
        }

        console2.log("DeployFactory:", address(DEPLOY_FACTORY));
        console2.log("Implementation salt:");
        console2.logBytes32(implementationSalt);
        console2.log("Proxy salt:");
        console2.logBytes32(proxySalt);
        console2.log("Predicted implementation:", predictedImplementation);
        console2.log("Predicted proxy:", predictedProxy);
        console2.log("GaslessWhitelist implementation:", implementation);
        console2.log("GaslessWhitelist proxy:", proxy);
        console2.log("ProxyAdmin:", proxyAdmin);
        console2.log("Configured owner:", owner);
        console2.log("GaslessWhitelist owner:", whitelistOwner);
        console2.log("ProxyAdmin owner:", proxyAdminOwner);
        console2.log("Gasless enabled:", IGaslessWhitelist(proxy).gaslessEnabled());
    }
}
