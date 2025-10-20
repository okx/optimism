// SPDX-License-Identifier: MIT
pragma solidity 0.8.15;

// Contracts
import { ERC20 } from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import { Ownable } from "@openzeppelin/contracts/access/Ownable.sol";
import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
// Interfaces
import { IOKB } from "interfaces/L1/IOKB.sol";
import { IOptimismPortal2 } from "interfaces/L1/IOptimismPortal2.sol";

/// @title DepositedOKBAdapter
/// @notice This contract is an ERC20 adapter that allows for burning OKB tokens on L1
///         and depositing them into L2. It enforces the strict 21 million supply cap
///         by burning OKB tokens and minting equivalent deposit tokens (dOKB) that can only
///         be used with the OptimismPortal2 for deposits to L2.
///
///         Key features:
///         - Burns OKB from user's wallet upon deposit
///         - Mints deposit tokens (dOKB) that are locked to this contract
///         - Only this contract and the OptimismPortal2 have balances of dOKB.
///         - Automatically initiates L2 deposit transaction
///
/// @dev This token is set as the gasPayingToken on SystemConfig.
contract DepositedOKBAdapter is ERC20, Ownable {
    using SafeERC20 for IERC20;

    /// @notice Address of the OptimismPortal2 contract that this adapter works with.
    IOptimismPortal2 public immutable PORTAL;

    /// @notice Address of the OKB token contract.
    IOKB public immutable OKB;

    /// @notice Default gas limit for L2 deposit transactions.
    uint64 public constant DEFAULT_GAS_LIMIT = 100_000;

    /// @notice Mapping of whitelisted addresses allowed to deposit.
    mapping(address => bool) public whitelist;

    /// @notice Emitted when a user deposits OKB and initiates an L2 transaction.
    /// @param from   Address that deposited the OKB.
    /// @param to     Target address on L2.
    /// @param amount Amount of OKB burned and deposited.
    event Deposited(address indexed from, address indexed to, uint256 amount);

    /// @notice Emitted when an address is added to the whitelist.
    /// @param account Address that was added to the whitelist.
    event WhitelistAdded(address indexed account);

    /// @notice Emitted when an address is removed from the whitelist.
    /// @param account Address that was removed from the whitelist.
    event WhitelistRemoved(address indexed account);

    /// @notice Thrown when transfer is attempted outside of portal operations.
    error TransferNotAllowed();

    /// @notice Thrown when amount is zero.
    error AmountMustBeGreaterThanZero();

    /// @notice Thrown when balance is insufficient.
    error InsufficientBalance();

    /// @notice Thrown when transfer fails.
    error TransferFailed();

    /// @notice Thrown when transfer OKB from user fails.
    error TransferFromUserFailed();

    /// @notice Thrown when OKB balance is not equal to the amount deposited.
    error OKBBalanceMismatch();

    /// @notice Thrown when caller is not whitelisted.
    error NotWhitelisted();

    /// @notice Thrown when address is zero.
    error AddressCannotBeZero();

    /// @notice Thrown when OKB balance is not zero.
    error OKBBalanceNotZeroAfterBurn();

    /// @notice Constructor sets up the adapter with references to OKB, OptimismPortal, rescuer,
    ///         and premints the maximum supply to the owner.
    /// @param _okb                 Address of the OKB token contract.
    /// @param _portal              Address of the OptimismPortal2 contract.
    /// @param _owner               Address of the contract owner.
    constructor(address _okb, address payable _portal, address _owner) ERC20("Deposited OKB", "dOKB") {
        if (_okb == address(0)) {
            revert AddressCannotBeZero();
        }
        if (_portal == address(0)) {
            revert AddressCannotBeZero();
        }
        OKB = IOKB(_okb);
        PORTAL = IOptimismPortal2(_portal);

        // Premint total supply of OKB to this contract to enforce hard cap
        _mint(address(this), OKB.totalSupply());

        // Transfer ownership to the owner
        transferOwnership(_owner);
    }

    /// @notice Adds multiple addresses to the whitelist in a single transaction.
    /// @param _accounts Array of addresses to add to the whitelist.
    function addToWhitelistBatch(address[] calldata _accounts) external onlyOwner {
        for (uint256 i = 0; i < _accounts.length; i++) {
            if (_accounts[i] == address(0)) {
                revert AddressCannotBeZero();
            }
            whitelist[_accounts[i]] = true;
            emit WhitelistAdded(_accounts[i]);
        }
    }

    /// @notice Removes multiple addresses from the whitelist in a single transaction.
    /// @param _accounts Array of addresses to remove from the whitelist.
    function removeFromWhitelistBatch(address[] calldata _accounts) external onlyOwner {
        for (uint256 i = 0; i < _accounts.length; i++) {
            if (_accounts[i] == address(0)) {
                revert AddressCannotBeZero();
            }
            whitelist[_accounts[i]] = false;
            emit WhitelistRemoved(_accounts[i]);
        }
    }

    /// @notice Allows whitelisted users to burn OKB and deposit into L2.
    ///         This function:
    ///         1. Checks if caller is whitelisted
    ///         2. Transfers OKB from the user to this contract
    ///         3. Creates a minimal proxy burner contract
    ///         4. Transfers the exact amount of OKB to the burner
    ///         5. Burns the OKB via the burner (which self-destructs)
    ///         6. Mints deposit tokens to this contract
    ///         7. Initiates an L2 deposit transaction via the portal
    /// @param _to         Target address on L2 to receive the tokens.
    /// @param _amount     Amount of OKB to burn and deposit.
    function deposit(address _to, uint256 _amount) external {
        if (!whitelist[msg.sender]) {
            revert NotWhitelisted();
        }
        if (_amount == 0) {
            revert AmountMustBeGreaterThanZero();
        }
        if (OKB.balanceOf(msg.sender) < _amount) {
            revert InsufficientBalance();
        }

        // Transfer any remaining OKB to rescuer.
        // If someone mistakenly directly transfer OKB to this contract, transfer it to the owner.
        if (OKB.balanceOf(address(this)) > 0) {
            bool transferSuccess = OKB.transfer(owner(), OKB.balanceOf(address(this)));
            if (!transferSuccess) {
                revert TransferFailed();
            }
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

        // Check invariant: the amount of OKB in this contract should be zero after burning.
        if (OKB.balanceOf(address(this)) > 0) {
            revert OKBBalanceNotZeroAfterBurn();
        }

        // Approve the portal to pull the deposit tokens
        _approve(address(this), address(PORTAL), _amount);

        // Portal will call transferFrom to pull the deposit tokens
        PORTAL.depositERC20Transaction(_to, _amount, _amount, DEFAULT_GAS_LIMIT, false, "");

        emit Deposited(msg.sender, _to, _amount);
    }

    /// @notice Override transfer to disable transfers
    ///         This ensures that deposit tokens can only be used by the portal
    ///         and cannot be transferred or traded elsewhere.
    /// @return bool  Always reverts.
    function transfer(address, /* _to */ uint256 /* _amount */ ) public virtual override returns (bool) {
        // Do not allow any transfers
        revert TransferNotAllowed();
    }

    /// @notice Override transferFrom to disable transfers
    ///         This ensures that deposit tokens can only be used by the portal
    ///         and cannot be transferred or traded elsewhere.
    /// @param from   Sender address.
    /// @param to     Recipient address.
    /// @param amount Amount to transfer.
    /// @return bool  True if transfer succeeds.
    function transferFrom(address from, address to, uint256 amount) public virtual override returns (bool) {
        // Ensure only the portal can pull from this contract, and only to the portal.
        if (msg.sender == address(PORTAL) && to == address(PORTAL) && from == address(this)) {
            return super.transferFrom(from, to, amount);
        }

        revert TransferNotAllowed();
    }

    /// @notice Allows owner to rescue ERC20 tokens sent to this contract.
    /// @param _token   Address of the ERC20 token to rescue.
    /// @param _to      Address to send the tokens to.
    /// @param _amount  Amount of tokens to rescue.
    function rescueERC20(address _token, address _to, uint256 _amount) external onlyOwner {
        if (_token == address(0)) {
            revert AddressCannotBeZero();
        }
        if (_to == address(0)) {
            revert AddressCannotBeZero();
        }
        if (_amount == 0) {
            revert AmountMustBeGreaterThanZero();
        }

        IERC20(_token).safeTransfer(_to, _amount);
    }
}
