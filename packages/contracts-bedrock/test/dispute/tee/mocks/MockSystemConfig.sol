// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import {ISuperchainConfig} from "interfaces/L1/ISuperchainConfig.sol";

contract MockSystemConfig {
    bool public paused;
    address public guardian;
    ISuperchainConfig public superchainConfig;

    constructor(address guardian_) {
        guardian = guardian_;
    }

    function setPaused(bool value) external {
        paused = value;
    }

    function setGuardian(address value) external {
        guardian = value;
    }

    function setSuperchainConfig(ISuperchainConfig value) external {
        superchainConfig = value;
    }
}
