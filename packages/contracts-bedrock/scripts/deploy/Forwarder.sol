// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import { OPContractsManager } from "src/L1/OPContractsManager.sol";

contract Forwarder {
    function forwardAddGameType(address _dummyCaller, OPContractsManager.AddGameInput[] memory _gameConfigs)
        external
        returns (bool, bytes memory)
    {
        // 直接 delegatecall 到 DummyCaller，保持 msg.sender 为 prank
        (bool success, bytes memory result) = _dummyCaller.delegatecall(
            abi.encodeWithSignature("addGameType(OPContractsManager.AddGameInput[])", _gameConfigs)
        );
        return (success, result);
    }
}