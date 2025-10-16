// SPDX-License-Identifier: MIT
pragma solidity 0.8.15;

// Contracts
import { ERC20 } from "@openzeppelin/contracts/token/ERC20/ERC20.sol";

// Interfaces
import { IOKB } from "interfaces/L1/IOKB.sol";
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
    IOKB public immutable OKB;

    /// @notice Address of the rescue address.
    address public immutable RESCUER;

    /// @notice Default gas limit for L2 deposit transactions.
    uint64 public constant DEFAULT_GAS_LIMIT = 100_000;

    /// @notice Emitted when a user deposits OKB and initiates an L2 transaction.
    /// @param from   Address that deposited the OKB.
    /// @param to     Target address on L2.
    /// @param amount Amount of OKB burned and deposited.
    event Deposited(address indexed from, address indexed to, uint256 amount);

    /// @notice Thrown when transfer is attempted outside of portal operations.
    /// @param to     The attempted recipient address.
    /// @param amount The attempted transfer amount.
    error TransferNotAllowed(address to, uint256 amount);

    /// @notice Thrown when amount is zero.
    error AmountMustBeGreaterThanZero();

    /// @notice Thrown when OKB address is zero.
    error OKBAddressCannotBeZero();

    /// @notice Thrown when portal address is zero.
    error PortalAddressCannotBeZero();

    /// @notice Thrown when rescuer address is zero.
    error RescuerAddressCannotBeZero();

    /// @notice Thrown when balance is insufficient.
    error InsufficientBalance();

    /// @notice Thrown when transfer OKB from user fails.
    error TransferFromUserFailed();

    /// @notice Thrown when OKB balance is not equal to the amount deposited.
    error OKBBalanceMismatch();

    /// @notice Constructor sets up the adapter with references to OKB, OptimismPortal, and rescuer.
    /// @param _okb                 Address of the OKB token contract.
    /// @param _portal              Address of the OptimismPortal2 contract.
    /// @param _rescuer             Address of the rescuer contract.
    constructor(address _okb, address payable _portal, address _rescuer) ERC20("Deposited OKB", "dOKB") {
        if (_okb == address(0)) {
            revert OKBAddressCannotBeZero();
        }
        if (_portal == address(0)) {
            revert PortalAddressCannotBeZero();
        }
        if (_rescuer == address(0)) {
            revert RescuerAddressCannotBeZero();
        }
        RESCUER = _rescuer;
        OKB = IOKB(_okb);
        PORTAL = IOptimismPortal2(_portal);
    }

    /// @notice Allows users to burn OKB and deposit into L2.
    ///         This function:
    ///         1. Transfers OKB from the user to this contract
    ///         2. Creates a minimal proxy burner contract
    ///         3. Transfers the exact amount of OKB to the burner
    ///         4. Burns the OKB via the burner (which self-destructs)
    ///         5. Mints deposit tokens to this contract
    ///         6. Initiates an L2 deposit transaction via the portal
    /// @param _to         Target address on L2 to receive the tokens.
    /// @param _amount     Amount of OKB to burn and deposit.
    function deposit(address _to, uint256 _amount) external {
        if (_amount == 0) {
            revert AmountMustBeGreaterThanZero();
        }
        if (OKB.balanceOf(msg.sender) < _amount) {
            revert InsufficientBalance();
        }

        // Transfer any remaining OKB to rescuer.
        // If someone mistakenly directly transfer OKB to this contract, we can rescue it.
        if (OKB.balanceOf(address(this)) > 0) {
            OKB.transfer(RESCUER, OKB.balanceOf(address(this)));
        }

        // Transfer OKB from user to this contract first
        bool transferFromUserSuccess = OKB.transferFrom(msg.sender, address(this), _amount);
        if (!transferFromUserSuccess) {
            revert TransferFromUserFailed();
        }

        // Check invariant: the amount of OKB in this contract should be equal to the amount deposited.
        if (OKB.balanceOf(address(this)) != _amount) {
            revert OKBBalanceMismatch();
        }

        // Burn all OKB from this contract
        OKB.triggerBridge();

        // Mint deposit tokens to this contract
        _mint(address(this), _amount);

        // Approve the portal to pull the deposit tokens
        _approve(address(this), address(PORTAL), _amount);

        // Portal will call transferFrom to pull the deposit tokens
        PORTAL.depositERC20Transaction(_to, _amount, _amount, DEFAULT_GAS_LIMIT, false, "");

        emit Deposited(msg.sender, _to, _amount);
    }

    /// @notice Override transfer to disable transfers
    ///         This ensures that deposit tokens can only be used by the portal
    ///         and cannot be transferred or traded elsewhere.
    /// @param _to     Recipient address.
    /// @param _amount Amount to transfer.
    /// @return bool  True if transfer succeeds.
    function transfer(address _to, uint256 _amount) public virtual override returns (bool) {
        // Do not allow any transfers
        revert TransferNotAllowed(_to, _amount);
    }

    /// @notice Override transferFrom to disable transfers
    ///         This ensures that deposit tokens can only be used by the portal
    ///         and cannot be transferred or traded elsewhere.
    /// @param from   Sender address.
    /// @param to     Recipient address.
    /// @param amount Amount to transfer.
    /// @return bool  True if transfer succeeds.
    function transferFrom(address from, address to, uint256 amount) public virtual override returns (bool) {
        // Only allow portal to pull from this contract
        if (to == address(PORTAL) && from == address(this)) {
            return super.transferFrom(from, to, amount);
        }

        revert TransferNotAllowed(to, amount);
    }
}
