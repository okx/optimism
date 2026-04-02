// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import { Script } from "forge-std/Script.sol";
import { console } from "forge-std/console.sol";
import { DisputeGameFactory } from "src/dispute/DisputeGameFactory.sol";
import { AnchorStateRegistry } from "src/dispute/AnchorStateRegistry.sol";
import { IDisputeGameFactory } from "interfaces/dispute/IDisputeGameFactory.sol";
import { IProxyAdmin } from "interfaces/universal/IProxyAdmin.sol";
import { ISystemConfig } from "interfaces/L1/ISystemConfig.sol";
import { IOptimismPortal2 } from "interfaces/L1/IOptimismPortal2.sol";
import { GameTypes } from "src/dispute/lib/Types.sol";

interface ITransactor {
    function CALL(address, bytes memory, uint256) external payable returns (bool, bytes memory);
}

contract UpgradeToV6 is Script {
    function run() external {
        address sysConfig = vm.envAddress("SYSTEM_CONFIG_PROXY_ADDRESS");
        address transactor = vm.envAddress("TRANSACTOR");
        uint256 finalityDelay = vm.envUint("DISPUTE_GAME_FINALITY_DELAY_SECONDS");

        address dgfProxy = ISystemConfig(sysConfig).disputeGameFactory();
        address portal = ISystemConfig(sysConfig).optimismPortal();
        address asrProxy = address(IOptimismPortal2(payable(portal)).anchorStateRegistry());
        address proxyAdmin = address(uint160(uint256(vm.load(dgfProxy,
            bytes32(0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103)))));

        console.log("DGF proxy:", dgfProxy, "version:", DisputeGameFactory(dgfProxy).version());
        console.log("ASR proxy:", asrProxy, "version:", AnchorStateRegistry(asrProxy).version());

        vm.startBroadcast();

        // Deploy new implementations
        address newDGF = address(new DisputeGameFactory());
        address newASR = address(new AnchorStateRegistry(finalityDelay));
        console.log("New DGF impl:", newDGF);
        console.log("New ASR impl:", newASR);

        // Upgrade via Transactor -> ProxyAdmin
        _upgradeProxy(transactor, proxyAdmin, dgfProxy, newDGF);
        console.log("Upgraded DGF");
        _upgradeProxy(transactor, proxyAdmin, asrProxy, newASR);
        console.log("Upgraded ASR");

        vm.stopBroadcast();

        // Verify
        console.log("DGF version:", DisputeGameFactory(dgfProxy).version());
        console.log("ASR version:", AnchorStateRegistry(asrProxy).version());
        console.log("DGF owner:", DisputeGameFactory(dgfProxy).owner());
        console.log("PDG (type 1):", address(IDisputeGameFactory(dgfProxy).gameImpls(GameTypes.PERMISSIONED_CANNON)));
    }

    function _upgradeProxy(address transactor, address proxyAdmin, address proxy, address impl) internal {
        bytes memory data = abi.encodeCall(IProxyAdmin.upgrade, (payable(proxy), impl));
        (bool ok,) = ITransactor(transactor).CALL(proxyAdmin, data, 0);
        require(ok, "upgrade failed");
    }
}
