// SPDX-License-Identifier: MIT
pragma solidity 0.8.15;

// Interfaces
import { IOKB } from "interfaces/L1/IOKB.sol";

/// @title OKBBurner
/// @notice Implementation contract for burning OKB tokens via minimal proxy pattern.
///         This contract is designed to be cloned using CREATE2 for deterministic addresses.
///         Each clone burns exactly the OKB tokens it holds and then self-destructs.
/// @dev This contract should never be used directly - only through minimal proxies.
contract OKBBurner {
    /// @notice Address of the OKB token contract.
    IOKB public immutable OKB;

    /// @notice Emitted when OKB tokens are burned by this burner.
    /// @param amount Amount of OKB tokens burned.
    event OKBBurned(uint256 amount);

    /// @notice Constructor sets the OKB token and adapter addresses.
    /// @param _okb     Address of the OKB token contract.
    constructor(address _okb) {
        require(_okb != address(0), "OKBBurner: OKB address cannot be zero");
        OKB = IOKB(_okb);
    }

    /// @notice Burns all OKB tokens held by this contract and self-destructs.
    ///         Can only be called by the adapter contract.
    /// @dev This function:
    ///      1. Gets the current OKB balance
    ///      2. Calls triggerBridge() to burn all tokens
    ///      3. Self-destructs to clean up and refund gas
    function burnAndDestruct() external {
        // Get balance before burning for event emission
        uint256 balance = OKB.balanceOf(address(this));

        // Burn all OKB tokens held by this contract
        if (balance > 0) {
            OKB.triggerBridge();
            emit OKBBurned(balance);
        }

        // Self-destruct and send any remaining ETH to adapter
        selfdestruct(payable(msg.sender));
    }

    /// @notice Returns the current OKB balance of this burner.
    /// @return balance Current OKB balance.
    function getBalance() external view returns (uint256 balance) {
        return OKB.balanceOf(address(this));
    }
}
