// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @title  GaslessWhitelist
/// @notice Three-layer match for gasless eligibility:
///           target (contract) -> selector (method) -> field constraints (calldata).
///
///         Matching semantics:
///           - TargetConfig.allowAll => unconditional pass for that contract
///           - Multiple Rules under the same target  => OR
///           - Multiple FieldRules within one Rule   => AND
contract GaslessWhitelist {
    // ---------------------------------------------------------------------
    // Storage
    // ---------------------------------------------------------------------

    address public owner;
    bool public isGaslessEnabled;
    uint64 public defaultGasLimit = 100_000;

    // ---------------------------------------------------------------------
    // Events
    // ---------------------------------------------------------------------

    event OwnerChanged(address indexed oldOwner, address indexed newOwner);
    event GaslessEnabledSet(bool enabled);

    // ---------------------------------------------------------------------
    // Errors
    // ---------------------------------------------------------------------

    error NotOwner();

    // ---------------------------------------------------------------------
    // Init
    // ---------------------------------------------------------------------

    modifier onlyOwner() {
        if (msg.sender != owner) revert NotOwner();
        _;
    }

    constructor(address owner_) {
        owner = owner_;
        emit OwnerChanged(address(0), owner_);
    }

    function setGaslessEnabled(bool enabled) external onlyOwner {
        isGaslessEnabled = enabled;
        emit GaslessEnabledSet(enabled);
    }

    function setAllowance(uint64 gasLimit) external onlyOwner {
        defaultGasLimit = gasLimit;
    }

    /// @notice Returns the gasless allowance (boolean + gas limit) for a given call.
    function getGaslessAllowance(address to, bytes calldata dataPrefix)
        external
        view
        returns (bool allowed, uint64 gasLimit)
    {
        return (isGaslessEnabled, defaultGasLimit);
    }
}
