// SPDX-License-Identifier: MIT
pragma solidity 0.8.15;

import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { Ownable } from "@openzeppelin/contracts/access/Ownable.sol";

/// @title ERC20Rescuer
/// @notice Contract that allows the owner to transfer ERC20 tokens out of the contract.
///         Uses OpenZeppelin's Ownable for access control and ownership management.
contract ERC20Rescuer is Ownable {
    /// @notice Emitted when ERC20 tokens are rescued from the contract.
    /// @param token The address of the ERC20 token contract.
    /// @param to The address receiving the rescued tokens.
    /// @param amount The amount of tokens rescued.
    event ERC20Rescued(address indexed token, address indexed to, uint256 amount);

    /// @notice Constructs the ERC20Rescuer contract.
    /// @param _owner The initial owner address who can rescue tokens.
    constructor(address _owner) Ownable(_owner) { }

    /// @notice Rescues ERC20 tokens from this contract.
    /// @param _token The address of the ERC20 token to rescue.
    /// @param _to The address to send the rescued tokens to.
    /// @param _amount The amount of tokens to rescue.
    function rescueERC20(address _token, address _to, uint256 _amount) external onlyOwner {
        IERC20(_token).transfer(_to, _amount);
        emit ERC20Rescued(_token, _to, _amount);
    }
}
