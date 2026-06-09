// SPDX-License-Identifier: MIT
pragma solidity 0.8.15;

// Contracts
import { Proxy } from "src/universal/Proxy.sol";

/// @custom:predeploy 0x4200000000000000000000000000000000000700
/// @title GaslessWhitelistProxy
/// @notice Transparent (EIP-1967) proxy that fronts the GaslessWhitelist implementation
///         (src/L2/XlayerGaslessWhitelist.sol). Deploying the predeploy as this proxy instead of
///         the bare implementation lets the GaslessWhitelist logic be upgraded in place via
///         `upgradeTo` / `upgradeToAndCall`, which is what makes upgrade rehearsals possible on the
///         devnet.
/// @dev This is a pure pass-through of the OP Stack {Proxy}; it adds no storage and no functions so
///      that the EIP-1967 admin/implementation slots and the implementation's storage layout are
///      identical to a stock OP Stack proxied predeploy.
///
///      Storage model (all logic state lives in THIS proxy's storage via delegatecall):
///        - EIP-1967 implementation slot
///            0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc -> impl address
///        - EIP-1967 admin slot
///            0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103 -> proxy admin
///        - slot 51 (0x33) -> GaslessWhitelist._owner   (OwnableUpgradeable)
///        - slot 105 (0x69) -> GaslessWhitelist.gaslessEnabled
///
///      Transparent-proxy rule: the proxy admin and the implementation owner MUST be different
///      accounts. The admin's calls are intercepted by the proxy (upgradeTo/admin/implementation/
///      changeAdmin) and never reach the implementation, so the admin could not also act as owner.
contract GaslessWhitelistProxy is Proxy {
    /// @param _admin Address allowed to use the transparent proxy admin interface (upgrades).
    constructor(address _admin) Proxy(_admin) { }
}
