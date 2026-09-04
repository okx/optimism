// SPDX-License-Identifier: MIT
//
// @dev TEST-ONLY: Genesis-placed system contract for the X Layer devnet Force Tx blacklist.
//      Bound to the fixed devnet address 0xb1ac000000000000000000000000000000000001. Installed as
//      runtime bytecode in the L2 genesis alloc, so no constructor runs and storage starts zeroed.
//      Do not add a constructor, initializer, owner, or proxy.
pragma solidity 0.8.28;

/// @title TxBlacklistTestable
/// @notice Test-only Force Tx (deposit) blacklist for the X Layer devnet. The execution layer performs
///         a system call `isBlacklisted(bytes32)` (selector 0x6b623bbe) against the fixed devnet address
///         to decide whether to intercept an interceptable L1 user deposit by its source hash. Operator
///         management is intentionally permissionless (test-only); blacklist mutation is operator-gated.
contract TxBlacklistTestable {
    /// @notice Maximum number of entries accepted by a single batch call.
    uint256 public constant MAX_BATCH_SIZE = 500;

    /// @notice Accounts authorized to mutate the blacklist.
    mapping(address => bool) public operators;
    /// @notice Blacklisted deposit source hashes.
    mapping(bytes32 => bool) public blacklisted;
    /// @notice Number of currently-blacklisted hashes.
    uint256 public blacklistCount;

    event OperatorSet(address indexed operator, bool allowed);
    event Blacklisted(bytes32 indexed hash);
    event RemovedFromBlacklist(bytes32 indexed hash);

    error NotOperator(address caller);
    error ZeroHash();
    error BatchTooLarge(uint256 size);

    modifier onlyOperator() {
        if (!operators[msg.sender]) revert NotOperator(msg.sender);
        _;
    }

    /// @notice Permissionless (test-only) operator management.
    function setOperator(address operator, bool allowed) external {
        operators[operator] = allowed;
        emit OperatorSet(operator, allowed);
    }

    /// @notice Permissionless (test-only) batch operator management.
    function setOperators(address[] calldata ops, bool allowed) external {
        if (ops.length > MAX_BATCH_SIZE) revert BatchTooLarge(ops.length);
        for (uint256 i = 0; i < ops.length; i++) {
            operators[ops[i]] = allowed;
            emit OperatorSet(ops[i], allowed);
        }
    }

    /// @notice Add a single source hash to the blacklist.
    function addToBlacklist(bytes32 hash) external onlyOperator {
        _add(hash);
    }

    /// @notice Add a batch of source hashes to the blacklist.
    function batchAddToBlacklist(bytes32[] calldata hashes) external onlyOperator {
        if (hashes.length > MAX_BATCH_SIZE) revert BatchTooLarge(hashes.length);
        for (uint256 i = 0; i < hashes.length; i++) {
            _add(hashes[i]);
        }
    }

    /// @notice Remove a single source hash from the blacklist.
    function removeFromBlacklist(bytes32 hash) external onlyOperator {
        _remove(hash);
    }

    /// @notice Remove a batch of source hashes from the blacklist.
    function batchRemoveFromBlacklist(bytes32[] calldata hashes) external onlyOperator {
        if (hashes.length > MAX_BATCH_SIZE) revert BatchTooLarge(hashes.length);
        for (uint256 i = 0; i < hashes.length; i++) {
            _remove(hashes[i]);
        }
    }

    /// @notice Execution-layer hot-path surface. Selector MUST remain 0x6b623bbe.
    function isBlacklisted(bytes32 hash) external view returns (bool) {
        return blacklisted[hash];
    }

    /// @notice Batch view helper (not on the execution-layer hot path).
    function areBlacklisted(bytes32[] calldata hashes) external view returns (bool[] memory result) {
        result = new bool[](hashes.length);
        for (uint256 i = 0; i < hashes.length; i++) {
            result[i] = blacklisted[hashes[i]];
        }
    }

    function _add(bytes32 hash) internal {
        if (hash == bytes32(0)) revert ZeroHash();
        if (!blacklisted[hash]) {
            blacklisted[hash] = true;
            blacklistCount++;
            emit Blacklisted(hash);
        }
    }

    function _remove(bytes32 hash) internal {
        if (blacklisted[hash]) {
            blacklisted[hash] = false;
            blacklistCount--;
            emit RemovedFromBlacklist(hash);
        }
    }
}
