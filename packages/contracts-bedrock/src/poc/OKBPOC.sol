// SPDX-License-Identifier: UNLICENSED
pragma solidity ^0.8.10;

import {ERC20} from "solady/ext/wake/weird/ERC20.sol";
contract OKBPOC is ERC20 {
    constructor() ERC20(1e20) {}
}
