// SPDX-License-Identifier: MIT
pragma solidity 0.8.15;

// Contracts
import { WETH99 } from "src/L2/xlayer/WETH99.sol";

// Libraries
import { Predeploys } from "src/libraries/Predeploys.sol";

// Interfaces
import { ISemver } from "interfaces/universal/ISemver.sol";
import { IL1Block } from "interfaces/L2/IL1Block.sol";

contract WOKB is WETH99, ISemver {
    /// @custom:semver 1.0.0
    string public constant version = "1.0.0";

    address constant BRIDGE = 0x4200000000000000000000000000000000000010;

    event AutoUnwrap(address indexed user, uint256 amount);

    /// @notice Transfer function that automatically unwraps when called by bridge
    /// @param to Address to transfer to
    /// @param amount Amount to transfer
    /// @return True if the transfer was successful
    function transfer(address to, uint256 amount) external override returns (bool) {
        // If sender is bridge contract, automatically withdraw to OKB and transfer to user
        if (msg.sender == BRIDGE) {
            return _withdrawTo(msg.sender, to, amount);
        }

        // Otherwise execute normal transfer logic
        return transferFrom(msg.sender, to, amount);
    }

    /// @notice Internal method: withdraw WOKB from specified address and send to target address
    /// @param from Address to withdraw WOKB from
    /// @param to Address to receive OKB
    /// @param amount Amount to withdraw
    function _withdrawTo(address from, address to, uint256 amount) internal returns (bool) {
        require(_balanceOf[from] >= amount, "WOKB: insufficient balance");
        _balanceOf[from] -= amount;

        (bool success, ) = payable(to).call{value: amount}("");
        require(success, "WOKB: OKB transfer failed");

        emit Withdrawal(from, amount);
        emit AutoUnwrap(to, amount);

        return true;
    }

    /// @notice Returns the name of the wrapped native asset. Will be "Wrapped Ether"
    ///         if the native asset is Ether.
    function name() external pure override returns (string memory name_) {
        name_ = string.concat("Wrapped ", IL1Block(Predeploys.L1_BLOCK_ATTRIBUTES).gasPayingTokenName());
    }

    /// @notice Returns the symbol of the wrapped native asset. Will be "WETH" if the
    ///         native asset is Ether.
    function symbol() external pure override returns (string memory symbol_) {
        symbol_ = string.concat("W", IL1Block(Predeploys.L1_BLOCK_ATTRIBUTES).gasPayingTokenSymbol());
    }
}
