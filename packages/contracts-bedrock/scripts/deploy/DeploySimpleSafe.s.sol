// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import { console2 as console } from "forge-std/console2.sol";
import { Script } from "forge-std/Script.sol";
import { GnosisSafe as Safe } from "safe-contracts/GnosisSafe.sol";
import { GnosisSafeProxyFactory as SafeProxyFactory } from "safe-contracts/proxies/GnosisSafeProxyFactory.sol";

/// @title DeploySimpleSafe
/// @notice 简化的 Safe 部署脚本，用于替代 Transactor 作为 l1ProxyAdminOwner
contract DeploySimpleSafe is Script {
    /// @notice 部署一个简单的 Safe 作为 l1ProxyAdminOwner
    function run() public {
        vm.startBroadcast();
        // 从环境变量获取部署者地址作为唯一所有者
        address deployer = vm.addr(vm.envUint("DEPLOYER_PRIVATE_KEY"));

        // 配置 Safe 参数
        address[] memory owners = new address[](1);
        owners[0] = deployer;

        uint256 threshold = 1; // 阈值设为 1，简化部署

        // 部署 Safe
        address safeAddress = deploySafe("L1ProxyAdminSafe", owners, threshold);

        console.log("L1ProxyAdminSafe deployed at:", safeAddress);
        console.log("   Owner:", deployer);
        console.log("   Threshold:", threshold);

        vm.stopBroadcast();
    }

    /// @notice 部署 Safe 合约
    /// @param _name Safe 合约名称
    /// @param _owners 所有者地址数组
    /// @param _threshold 签名阈值
    /// @return addr_ 部署的 Safe 合约地址
    function deploySafe(
        string memory _name,
        address[] memory _owners,
        uint256 _threshold
    ) internal returns (address addr_) {
        // 获取或部署 SafeProxyFactory 和 Safe Singleton
        (SafeProxyFactory safeProxyFactory, Safe safeSingleton) = _getSafeFactory();

        // 生成 salt（使用名称确保确定性部署）
        bytes32 salt = keccak256(abi.encode(_name, "DeploySimpleSafe"));
        console.log("Deploying safe: %s with salt %s", _name, vm.toString(salt));

        // 准备初始化数据
        bytes memory initData = abi.encodeCall(
            Safe.setup,
            (_owners, _threshold, address(0), hex"", address(0), address(0), 0, payable(address(0)))
        );

        // 创建 Safe 代理（使用 createProxyWithNonce 以支持 salt）
        addr_ = address(safeProxyFactory.createProxyWithNonce(address(safeSingleton), initData, uint256(salt)));

        console.log("New Safe %s deployed at: %s", _name, addr_);
    }

    /// @notice 获取 Safe 工厂合约
    /// @return safeProxyFactory_ SafeProxyFactory 合约实例
    /// @return safeSingleton_ Safe Singleton 合约实例
    function _getSafeFactory() internal returns (SafeProxyFactory safeProxyFactory_, Safe safeSingleton_) {
        // 使用标准部署地址
        address safeProxyFactory = 0xa6B71E26C5e0845f74c812102Ca7114b6a896AB2;
        address safeSingleton = 0xd9Db270c1B5E3Bd161E8c8503c55cEABeE709552;

        // 检查是否已部署，如果没有则部署新的
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
