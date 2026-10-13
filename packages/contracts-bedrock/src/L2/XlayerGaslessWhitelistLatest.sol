// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import { Initializable } from "@openzeppelin/contracts-upgradeable-v5/proxy/utils/Initializable.sol";
import { OwnableUpgradeable } from "@openzeppelin/contracts-upgradeable-v5/access/OwnableUpgradeable.sol";

/// @custom:proxied true
/// @title Gasless Whitelist
/// @notice Checks whether a target call matches X Layer fixed-contract gasless rules.
contract GaslessWhitelist is Initializable, OwnableUpgradeable {
    bytes4 public constant TRANSFER_SELECTOR = 0xa9059cbb;
    bytes4 public constant TRANSFER_FROM_SELECTOR = 0x23b872dd;
    bytes4 public constant APPROVE_SELECTOR = 0x095ea7b3;

    uint256 private constant SELECTOR_LENGTH = 4;
    uint256 private constant DATA_PREFIX_LENGTH = 36;

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

    mapping(address => FullyGaslessTarget) public fullyGaslessTargets;
    mapping(address => GaslessSelectorRule) public gaslessTransferTokens;
    mapping(address => GaslessSelectorRule) public gaslessTransferFromTokens;
    mapping(address => mapping(address => ApproveSpenderRule)) public approveSpendersByToken;
    uint64 public maxGasLimit;
    bool public gaslessEnabled;

    struct ApproveSpenderConfig {
        address token;
        address spender;
        bool allowed;
        uint64 gasLimit;
    }

    event FullyGaslessTargetSet(address indexed target, bool allowed, uint64 gasLimit);
    event GaslessTransferTokenSet(address indexed token, bool allowed, uint64 gasLimit);
    event GaslessTransferFromTokenSet(address indexed token, bool allowed, uint64 gasLimit);
    event ApproveSpenderSet(address indexed token, address indexed spender, bool allowed, uint64 gasLimit);
    event GaslessEnabledSet(bool enabled);
    event MaxGasLimitSet(uint64 maxGasLimit);

    error ZeroAddress();
    error InvalidGasLimit();
    error InvalidMaxGasLimit();
    error GasLimitExceedsMax();

    /// @notice Disables initialization on the implementation contract.
    /// @dev The proxy must call {initialize} during deployment.
    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor() {
        _disableInitializers();
    }

    /// @notice Initializes contract ownership.
    /// @dev `gaslessEnabled` intentionally remains false until the owner enables it.
    /// @param initialOwner Account allowed to manage whitelist configuration.
    function initialize(address initialOwner) external initializer {
        __Ownable_init(initialOwner);
        maxGasLimit = 16_777_216;
    }

    /// @notice Returns whether a call is eligible for gasless execution and its gas limit.
    /// @dev `dataPrefix` must contain at least the 4-byte selector. Approve checks require
    ///      the selector plus the first ABI argument, for a total of 36 bytes.
    /// @param to Transaction target address.
    /// @param dataPrefix Calldata prefix containing selector and optionally the first parameter.
    function getGaslessAllowance(
        address to,
        bytes calldata dataPrefix
    )
        external
        view
        returns (bool allowed, uint64 gasLimit)
    {
        if (!gaslessEnabled || maxGasLimit == 0 || to == address(0) || dataPrefix.length < SELECTOR_LENGTH) {
            return (false, 0);
        }

        bytes4 selector;
        bytes32 firstParam;

        assembly ("memory-safe") {
            selector := calldataload(dataPrefix.offset)
        }

        if (dataPrefix.length >= DATA_PREFIX_LENGTH) {
            assembly ("memory-safe") {
                firstParam := calldataload(add(dataPrefix.offset, SELECTOR_LENGTH))
            }
        }

        return _getGaslessAllowance(to, selector, firstParam, dataPrefix.length >= DATA_PREFIX_LENGTH);
    }

    /// @notice Adds or removes a target whose every selector is gasless eligible.
    /// @param target Target contract address.
    /// @param allowed True to allow, false to remove.
    /// @param gasLimit Gas limit returned when the rule matches.
    function setFullyGaslessTarget(address target, bool allowed, uint64 gasLimit) external onlyOwner {
        _setFullyGaslessTarget(target, allowed, gasLimit);
    }

    /// @notice Adds or removes a token whose `transfer(address,uint256)` calls are eligible.
    /// @param token Token contract address.
    /// @param allowed True to allow, false to remove.
    /// @param gasLimit Gas limit returned when the rule matches.
    function setGaslessTransferToken(address token, bool allowed, uint64 gasLimit) external onlyOwner {
        _setGaslessTransferToken(token, allowed, gasLimit);
    }

    /// @notice Adds or removes a token whose `transferFrom(address,address,uint256)` calls are
    ///         eligible.
    /// @dev Independent from the `transfer` rule and gas limit. Set `gasLimit` to account for the
    ///      extra allowance read/write that `transferFrom` performs over `transfer`.
    /// @param token Token contract address.
    /// @param allowed True to allow, false to remove.
    /// @param gasLimit Gas limit returned when the rule matches.
    function setGaslessTransferFromToken(address token, bool allowed, uint64 gasLimit) external onlyOwner {
        _setGaslessTransferFromToken(token, allowed, gasLimit);
    }

    /// @notice Adds or removes an allowed approve spender for a token.
    /// @dev Covers both approve-to-amount and approve-to-zero because only spender is parsed.
    /// @param token Token contract address.
    /// @param spender Spender allowed in `approve(address,uint256)`.
    /// @param allowed True to allow, false to remove.
    /// @param gasLimit Gas limit returned when the rule matches.
    function setApproveSpender(address token, address spender, bool allowed, uint64 gasLimit) external onlyOwner {
        _setApproveSpender(token, spender, allowed, gasLimit);
    }

    /// @notice Enables or disables all gasless whitelist rules.
    /// @param enabled True to enable rule matching, false to block every rule.
    function setGaslessEnabled(bool enabled) external onlyOwner {
        gaslessEnabled = enabled;
        emit GaslessEnabledSet(enabled);
    }

    /// @notice Sets the global gas limit cap returned by whitelist rules.
    /// @dev New rule configurations above the cap are rejected; existing rule returns are capped.
    /// @param newMaxGasLimit New non-zero maximum gas limit.
    function setMaxGasLimit(uint64 newMaxGasLimit) external onlyOwner {
        if (newMaxGasLimit == 0) {
            revert InvalidMaxGasLimit();
        }
        maxGasLimit = newMaxGasLimit;
        emit MaxGasLimitSet(newMaxGasLimit);
    }

    /// @notice Batch adds or removes full-target whitelist entries.
    /// @param configs Target, allow flag, and gas limit for each full-target rule.
    function batchSetFullyGaslessTargets(FullyGaslessTargetConfig[] calldata configs) external onlyOwner {
        uint256 length = configs.length;
        for (uint256 i; i < length; ++i) {
            FullyGaslessTargetConfig calldata config = configs[i];
            _setFullyGaslessTarget(config.target, config.allowed, config.gasLimit);
        }
    }

    /// @notice Batch adds or removes transfer-token whitelist entries.
    /// @param configs Token, allow flag, and gas limit for each transfer rule.
    function batchSetGaslessTransferTokens(GaslessTransferTokenConfig[] calldata configs) external onlyOwner {
        uint256 length = configs.length;
        for (uint256 i; i < length; ++i) {
            GaslessTransferTokenConfig calldata config = configs[i];
            _setGaslessTransferToken(config.token, config.allowed, config.gasLimit);
        }
    }

    /// @notice Batch adds or removes transferFrom-token whitelist entries.
    /// @param configs Token, allow flag, and gas limit for each transferFrom rule.
    function batchSetGaslessTransferFromTokens(GaslessTransferTokenConfig[] calldata configs) external onlyOwner {
        uint256 length = configs.length;
        for (uint256 i; i < length; ++i) {
            GaslessTransferTokenConfig calldata config = configs[i];
            _setGaslessTransferFromToken(config.token, config.allowed, config.gasLimit);
        }
    }

    /// @notice Batch configures approve spender whitelist entries.
    /// @param configs Token, spender, allow flag, and gas limit for each approve rule.
    function batchSetApproveSpenders(ApproveSpenderConfig[] calldata configs) external onlyOwner {
        uint256 length = configs.length;
        for (uint256 i; i < length; ++i) {
            ApproveSpenderConfig calldata config = configs[i];
            _setApproveSpender(config.token, config.spender, config.allowed, config.gasLimit);
        }
    }

    /// @dev Applies rule priority after selector and first parameter have been parsed.
    function _getGaslessAllowance(
        address to,
        bytes4 selector,
        bytes32 firstParam,
        bool hasFirstParam
    )
        internal
        view
        returns (bool allowed, uint64 gasLimit)
    {
        FullyGaslessTarget memory fullTargetConfig = fullyGaslessTargets[to];
        if (fullTargetConfig.allowed) {
            return (true, _capGasLimit(fullTargetConfig.gasLimit));
        }

        if (selector == TRANSFER_SELECTOR) {
            GaslessSelectorRule memory transferConfig = gaslessTransferTokens[to];
            if (!transferConfig.allowed) {
                return (false, 0);
            }
            return (true, _capGasLimit(transferConfig.gasLimit));
        }

        if (selector == TRANSFER_FROM_SELECTOR) {
            GaslessSelectorRule memory transferFromConfig = gaslessTransferFromTokens[to];
            if (!transferFromConfig.allowed) {
                return (false, 0);
            }
            return (true, _capGasLimit(transferFromConfig.gasLimit));
        }

        if (selector == APPROVE_SELECTOR && hasFirstParam) {
            address spender = address(uint160(uint256(firstParam)));
            ApproveSpenderRule memory approveConfig = approveSpendersByToken[to][spender];
            if (!approveConfig.allowed) {
                return (false, 0);
            }
            return (true, _capGasLimit(approveConfig.gasLimit));
        }

        return (false, 0);
    }

    /// @dev Applies the current global cap to matched rules.
    function _capGasLimit(uint64 gasLimit) internal view returns (uint64 cappedGasLimit) {
        uint64 currentMaxGasLimit = maxGasLimit;
        return gasLimit > currentMaxGasLimit ? currentMaxGasLimit : gasLimit;
    }

    /// @dev Stores one full-target whitelist entry and emits its indexing event.
    function _setFullyGaslessTarget(address target, bool allowed, uint64 gasLimit) internal {
        _revertIfZero(target);
        _validateGasLimit(allowed, gasLimit);
        fullyGaslessTargets[target] = FullyGaslessTarget({ allowed: allowed, gasLimit: gasLimit });
        emit FullyGaslessTargetSet(target, allowed, gasLimit);
    }

    /// @dev Stores one transfer-token whitelist entry and emits its indexing event.
    function _setGaslessTransferToken(address token, bool allowed, uint64 gasLimit) internal {
        _revertIfZero(token);
        _validateGasLimit(allowed, gasLimit);
        gaslessTransferTokens[token] = GaslessSelectorRule({ allowed: allowed, gasLimit: gasLimit });
        emit GaslessTransferTokenSet(token, allowed, gasLimit);
    }

    /// @dev Stores one transferFrom-token whitelist entry and emits its indexing event.
    function _setGaslessTransferFromToken(address token, bool allowed, uint64 gasLimit) internal {
        _revertIfZero(token);
        _validateGasLimit(allowed, gasLimit);
        gaslessTransferFromTokens[token] = GaslessSelectorRule({ allowed: allowed, gasLimit: gasLimit });
        emit GaslessTransferFromTokenSet(token, allowed, gasLimit);
    }

    /// @dev Stores one token-spender approve whitelist entry and emits its indexing event.
    function _setApproveSpender(address token, address spender, bool allowed, uint64 gasLimit) internal {
        _revertIfZero(token);
        _revertIfZero(spender);
        _validateGasLimit(allowed, gasLimit);
        approveSpendersByToken[token][spender] = ApproveSpenderRule({ allowed: allowed, gasLimit: gasLimit });
        emit ApproveSpenderSet(token, spender, allowed, gasLimit);
    }

    /// @dev Reverts if an address configuration value is zero.
    function _revertIfZero(address account) internal pure {
        if (account == address(0)) {
            revert ZeroAddress();
        }
    }

    /// @dev Reverts if a rule is enabled with a zero gas limit, which would return (true, 0).
    ///      Removals pass `allowed == false` and are exempt.
    function _validateGasLimit(bool allowed, uint64 gasLimit) internal view {
        if (allowed && gasLimit == 0) {
            revert InvalidGasLimit();
        }
        if (allowed && gasLimit > maxGasLimit) {
            revert GasLimitExceedsMax();
        }
    }
}
