// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

// Interfaces
import { ISemver } from "interfaces/universal/ISemver.sol";
import { IProxyAdminOwnedBase } from "interfaces/universal/IProxyAdminOwnedBase.sol";

/// @title IGaslessWhitelist
/// @notice Interface for the GaslessWhitelist predeploy.
interface IGaslessWhitelist is ISemver, IProxyAdminOwnedBase {
    struct FullyGaslessTarget {
        bool allowed;
        uint64 gasLimit;
    }

    struct GaslessSelectorRule {
        bool allowed;
        uint64 gasLimit;
    }

    struct ApproveSpenderRule {
        bool allowed;
        uint64 gasLimit;
    }

    struct FullyGaslessTargetConfig {
        address target;
        bool allowed;
        uint64 gasLimit;
    }

    struct GaslessTransferTokenConfig {
        address token;
        bool allowed;
        uint64 gasLimit;
    }

    struct ApproveSpenderConfig {
        address token;
        address spender;
        bool allowed;
        uint64 gasLimit;
    }

    event Initialized(uint8 version);
    event OwnershipTransferred(address indexed previousOwner, address indexed newOwner);
    event FullyGaslessTargetSet(address indexed target, bool allowed, uint64 gasLimit);
    event GaslessTransferTokenSet(address indexed token, bool allowed, uint64 gasLimit);
    event GaslessTransferFromTokenSet(address indexed token, bool allowed, uint64 gasLimit);
    event ApproveSpenderSet(address indexed token, address indexed spender, bool allowed, uint64 gasLimit);
    event GaslessEnabledSet(bool enabled);

    error ZeroAddress();
    error InvalidGasLimit();

    function TRANSFER_SELECTOR() external view returns (bytes4);
    function TRANSFER_FROM_SELECTOR() external view returns (bytes4);
    function APPROVE_SELECTOR() external view returns (bytes4);
    function initialize(address _owner) external;
    function getGaslessAllowance(
        address to,
        bytes calldata dataPrefix
    )
        external
        view
        returns (bool allowed, uint64 gasLimit);
    function setFullyGaslessTarget(address target, bool allowed, uint64 gasLimit) external;
    function setGaslessTransferToken(address token, bool allowed, uint64 gasLimit) external;
    function setGaslessTransferFromToken(address token, bool allowed, uint64 gasLimit) external;
    function setApproveSpender(address token, address spender, bool allowed, uint64 gasLimit) external;
    function setGaslessEnabled(bool enabled) external;
    function batchSetFullyGaslessTargets(FullyGaslessTargetConfig[] calldata configs) external;
    function batchSetGaslessTransferTokens(GaslessTransferTokenConfig[] calldata configs) external;
    function batchSetGaslessTransferFromTokens(GaslessTransferTokenConfig[] calldata configs) external;
    function batchSetApproveSpenders(ApproveSpenderConfig[] calldata configs) external;
    function fullyGaslessTargets(address target) external view returns (bool allowed, uint64 gasLimit);
    function gaslessTransferTokens(address token) external view returns (bool allowed, uint64 gasLimit);
    function gaslessTransferFromTokens(address token) external view returns (bool allowed, uint64 gasLimit);
    function approveSpendersByToken(
        address token,
        address spender
    )
        external
        view
        returns (bool allowed, uint64 gasLimit);
    function gaslessEnabled() external view returns (bool);
    function owner() external view returns (address);
    function transferOwnership(address newOwner) external;
    function renounceOwnership() external;
    function __constructor__() external;
}
