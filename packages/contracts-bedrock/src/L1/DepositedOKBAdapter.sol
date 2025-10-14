// SPDX-License-Identifier: MIT
pragma solidity 0.8.15;

// Contracts
import { ERC20 } from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import { Clones } from "@openzeppelin/contracts/proxy/Clones.sol";
import { OKBBurner } from "src/L1/OKBBurner.sol";

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

    /// @notice Address of the OKBBurner implementation contract.
    address public immutable BURNER_IMPLEMENTATION;

    /// @notice Counter for creating unique burner addresses.
    uint256 private _burnerNonce;

    /// @notice Default gas limit for L2 transactions.
    uint64 public constant DEFAULT_GAS_LIMIT = 100_000;

    /// @notice Emitted when a user deposits OKB and initiates an L2 transaction.
    /// @param from   Address that deposited the OKB.
    /// @param to     Target address on L2.
    /// @param amount Amount of OKB burned and deposited.
    event Deposited(address indexed from, address indexed to, uint256 amount);

    /// @notice Thrown when transfer is attempted outside of portal operations.
    /// @param to     The attempted recipient address.
    /// @param amount The attempted transfer amount.
    error DepositedOKBAdapter_TransferNotAllowed(address to, uint256 amount);

    /// @notice Thrown when trying to transfer to an address other than the portal.
    error DepositedOKBAdapter_OnlyPortalTransfer();

    /// @notice Thrown when burner creation fails.
    error DepositedOKBAdapter_BurnerCreationFailed();

    /// @notice Thrown when OKB transfer to burner fails.
    error DepositedOKBAdapter_TransferToBurnerFailed();

    /// @notice Constructor sets up the adapter with references to OKB, OptimismPortal, and OKBBurner.
    /// @param _okb                 Address of the OKB token contract.
    /// @param _portal              Address of the OptimismPortal2 contract.
    /// @param _burnerImplementation Address of the OKBBurner implementation contract.
    constructor(address _okb, address payable _portal, address _burnerImplementation) ERC20("Deposited OKB", "dOKB") {
        require(_okb != address(0), "DepositedOKBAdapter: OKB address cannot be zero");
        require(_portal != address(0), "DepositedOKBAdapter: Portal address cannot be zero");
        require(_burnerImplementation != address(0), "DepositedOKBAdapter: Burner implementation cannot be zero");

        OKB = IOKB(_okb);
        PORTAL = IOptimismPortal2(_portal);
        BURNER_IMPLEMENTATION = _burnerImplementation;

        // Approve the portal to pull deposit tokens
        _approve(address(this), _portal, type(uint256).max);
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
    /// @param _gasLimit   Gas limit for the L2 transaction.
    /// @param _isCreation Whether this is a contract creation transaction.
    /// @param _data       Additional data to pass to the L2 transaction.
    function deposit(address _to, uint256 _amount, uint64 _gasLimit, bool _isCreation, bytes memory _data) external {
        require(_amount > 0, "DepositedOKBAdapter: amount must be greater than zero");

        // Create a unique salt for deterministic burner address
        bytes32 salt = keccak256(abi.encode(msg.sender, _amount, block.timestamp, _burnerNonce++));

        // Create minimal proxy burner contract
        address burner = Clones.cloneDeterministic(BURNER_IMPLEMENTATION, salt);
        if (burner == address(0)) {
            revert DepositedOKBAdapter_BurnerCreationFailed();
        }

        // Transfer OKB from user to this contract first
        bool transferFromUserSuccess = OKB.transferFrom(msg.sender, address(this), _amount);
        if (!transferFromUserSuccess) {
            revert DepositedOKBAdapter_TransferToBurnerFailed();
        }

        // Transfer the exact amount of OKB from this contract to the burner
        bool transferToBurnerSuccess = OKB.transfer(burner, _amount);
        if (!transferToBurnerSuccess) {
            revert DepositedOKBAdapter_TransferToBurnerFailed();
        }

        // Burn the OKB via the burner (burner will self-destruct)
        OKBBurner(burner).burnAndDestruct();

        // Mint deposit tokens to this contract
        _mint(address(this), _amount);

        // Portal will call transferFrom to pull the deposit tokens
        PORTAL.depositERC20Transaction(_to, _amount, _amount, _gasLimit, _isCreation, _data);

        emit Deposited(msg.sender, _to, _amount);
    }

    /// @notice Returns the current burner nonce for creating unique addresses.
    /// @return nonce Current nonce value.
    function getBurnerNonce() external view returns (uint256 nonce) {
        return _burnerNonce;
    }

    /// @notice Predicts the address of the next burner contract.
    /// @param _user   User address.
    /// @param _amount Amount to be burned.
    /// @return burner Predicted burner address.
    function predictBurnerAddress(address _user, uint256 _amount) external view returns (address burner) {
        bytes32 salt = keccak256(abi.encode(_user, _amount, block.timestamp, _burnerNonce));
        return Clones.predictDeterministicAddress(BURNER_IMPLEMENTATION, salt, address(this));
    }

    /// @notice Override transfer to disable transfers
    ///         This ensures that deposit tokens can only be used by the portal
    ///         and cannot be transferred or traded elsewhere.
    /// @param _to     Recipient address.
    /// @param _amount Amount to transfer.
    /// @return bool  True if transfer succeeds.
    function transfer(address _to, uint256 _amount) public virtual override returns (bool) {
        // Do not allow any transfers
        revert DepositedOKBAdapter_TransferNotAllowed(_to, _amount);
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
        if (msg.sender == address(PORTAL) && from == address(this)) {
            return super.transferFrom(from, to, amount);
        }

        revert DepositedOKBAdapter_TransferNotAllowed(to, amount);
    }
}
