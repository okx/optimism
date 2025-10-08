// SPDX-License-Identifier: MIT
pragma solidity 0.8.15;

// Contracts
import { ERC20 } from "@openzeppelin/contracts/token/ERC20/ERC20.sol";

// Interfaces
import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";

import { IOptimismPortal2 } from "interfaces/L1/IOptimismPortal2.sol";

/// @title DepositedOKBAdapter
/// @notice This contract is an ERC20 adapter that allows for burning OKB tokens on L1
///         and depositing them into L2. It enforces the strict 21 million supply cap
///         by burning OKB tokens and minting equivalent deposit tokens that can only
///         be used with the OptimismPortal for deposits to L2.
///
///         Key features:
///         - Burns OKB from user's wallet upon deposit
///         - Mints deposit tokens that are locked to this contract
///         - Only the OptimismPortal can transfer these tokens
///         - Automatically initiates L2 deposit transaction
///
///         This design enables:
///         - Generic lock-and-mint pattern for custom gas tokens
///         - OKB burning to maintain strict supply constraints
///         - Seamless integration with existing OP Stack infrastructure
/// @dev This token is set as the gasPayingToken on SystemConfig.
contract DepositedOKBAdapter is ERC20 {
    /// @notice Address of the OptimismPortal2 contract that this adapter works with.
    IOptimismPortal2 public immutable PORTAL;

    /// @notice Address of the OKB token contract.
    IERC20 public immutable OKB;

    /// @notice Address where burned OKB tokens are sent (address(0)).
    address public constant BURN_ADDRESS = address(0x1111111111111111111111111111111111111111);

    /// @notice Default gas limit for L2 transactions.
    uint64 public constant DEFAULT_GAS_LIMIT = 100_000;

    /// @notice Emitted when a user deposits OKB and initiates an L2 transaction.
    /// @param from   Address that deposited the OKB.
    /// @param to     Target address on L2.
    /// @param amount Amount of OKB burned and deposited.
    event Deposited(address indexed from, address indexed to, uint256 amount);

    /// @notice Thrown when transfer is attempted outside of portal operations.
    error DepositedOKBAdapter_TransferNotAllowed();

    /// @notice Thrown when trying to transfer to an address other than the portal.
    error DepositedOKBAdapter_OnlyPortalTransfer();

    /// @notice Constructor sets up the adapter with references to OKB and OptimismPortal.
    /// @param _okb    Address of the OKB token contract.
    /// @param _portal Address of the OptimismPortal2 contract.
    constructor(address _okb, address payable _portal) ERC20("Deposited OKB", "dOKB") {
        require(_okb != address(0), "DepositedOKBAdapter: OKB address cannot be zero");
        require(_portal != address(0), "DepositedOKBAdapter: Portal address cannot be zero");

        OKB = IERC20(_okb);
        PORTAL = IOptimismPortal2(_portal);

        // Approve the portal to pull deposit tokens
        _approve(address(this), _portal, type(uint256).max);
    }

    /// @notice Allows users to burn OKB and deposit into L2.
    ///         This function:
    ///         1. Transfers OKB from the user to this contract
    ///         2. Burns the OKB by sending it to address(0)
    ///         3. Mints deposit tokens to this contract
    ///         4. Initiates an L2 deposit transaction via the portal
    /// @param _to         Target address on L2 to receive the tokens.
    /// @param _amount     Amount of OKB to burn and deposit.
    /// @param _gasLimit   Gas limit for the L2 transaction.
    /// @param _isCreation Whether this is a contract creation transaction.
    /// @param _data       Additional data to pass to the L2 transaction.
    function deposit(address _to, uint256 _amount, uint64 _gasLimit, bool _isCreation, bytes memory _data) external {
        require(_amount > 0, "DepositedOKBAdapter: amount must be greater than zero");

        // Transfer OKB from user to this contract
        OKB.transferFrom(msg.sender, address(this), _amount);

        // Burn the OKB
        OKB.transfer(BURN_ADDRESS, _amount);

        // Mint deposit tokens to this contract
        _mint(address(this), _amount);

        // Initiate the deposit transaction on L2
        PORTAL.depositERC20Transaction(_to, _amount, _amount, _gasLimit, _isCreation, _data);

        emit Deposited(msg.sender, _to, _amount);
    }

    /// @notice Convenience function for simple deposits with default parameters.
    /// @param _to     Target address on L2 to receive the tokens.
    /// @param _amount Amount of OKB to burn and deposit.
    function deposit(address _to, uint256 _amount) external {
        require(_amount > 0, "DepositedOKBAdapter: amount must be greater than zero");

        // Transfer OKB from user to this contract
        OKB.transferFrom(msg.sender, address(this), _amount);

        // Burn the OKB by sending to address(0)
        OKB.transfer(BURN_ADDRESS, _amount);

        // Mint deposit tokens to this contract
        _mint(address(this), _amount);

        // Initiate the deposit transaction on L2 with default gas limit
        PORTAL.depositERC20Transaction(_to, _amount, _amount, DEFAULT_GAS_LIMIT, false, bytes(""));

        emit Deposited(msg.sender, _to, _amount);
    }

    /// @notice Override transfer to restrict transfers to only portal operations.
    ///         This ensures that deposit tokens can only be used by the portal
    ///         and cannot be transferred or traded elsewhere.
    /// @param to     Recipient address.
    /// @param amount Amount to transfer.
    /// @return bool  True if transfer succeeds.
    function transfer(address to, uint256 amount) public virtual override returns (bool) {
        if (msg.sender == address(PORTAL) && to != address(PORTAL)) {
            revert DepositedOKBAdapter_TransferNotAllowed();
        }

        return super.transfer(to, amount);
    }

    /// @notice Override transferFrom to restrict transfers to only portal operations.
    /// @param from   Sender address.
    /// @param to     Recipient address.
    /// @param amount Amount to transfer.
    /// @return bool  True if transfer succeeds.
    function transferFrom(address from, address to, uint256 amount) public virtual override returns (bool) {
        // Only allow portal to pull from this contract
        if (msg.sender == address(PORTAL) && from == address(this)) {
            return super.transferFrom(from, to, amount);
        }

        revert DepositedOKBAdapter_TransferNotAllowed();
    }
}
