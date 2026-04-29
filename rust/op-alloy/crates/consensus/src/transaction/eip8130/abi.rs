//! `sol!` ABI definitions for EIP-8130 system contracts and verifiers.
//!
//! These interfaces define the on-chain components that the node interacts with
//! during AA transaction validation and execution.

use alloy_sol_types::sol;

sol! {
    /// Verifier interface. The protocol delegates signature verification to
    /// contracts implementing this interface via STATICCALL.
    interface IVerifier {
        /// Verifies a signature against a hash and returns the authenticated owner ID.
        /// Reverts on invalid signatures.
        function verify(bytes32 hash, bytes calldata data) external view returns (bytes32 ownerId);
    }
}

sol! {
    /// Owner tuple used in account creation.
    struct OwnerTuple {
        address verifier;
        bytes32 ownerId;
        uint8 scope;
    }

    /// Config operation tuple used in config changes.
    struct ConfigOpTuple {
        uint8 changeType;
        address verifier;
        bytes32 ownerId;
        uint8 scope;
    }

    /// Account configuration system contract. Manages owner registrations,
    /// account creation (CREATE2), config changes, and account locking.
    interface IAccountConfig {
        /// Returns the owner configuration for a given account and owner ID.
        function getOwner(address account, bytes32 ownerId) external view returns (address verifier, uint8 scope);

        /// Deploys a new account via CREATE2.
        function createAccount(bytes32 userSalt, bytes calldata bytecode, OwnerTuple[] calldata initialOwners) external returns (address);

        /// Computes the address that would result from `createAccount` without deploying.
        function getAddress(bytes32 userSalt, bytes calldata bytecode, OwnerTuple[] calldata initialOwners) external view returns (address);

        /// Applies a portable config change batch to an account.
        function applyConfigChange(address account, uint64 chainId, uint64 sequence, ConfigOpTuple[] calldata ownerChanges, bytes calldata authorizerAuth) external;

        /// Returns the current change sequence for an account on a given chain.
        function getChangeSequence(address account, uint64 chainId) external view returns (uint64);

        /// Locks an account's owner configuration for a minimum delay.
        function lock(address account, uint32 unlockDelay, bytes calldata signature) external;

        /// Requests an unlock, starting the timelock.
        function requestUnlock(address account, bytes calldata signature) external;

        /// Completes the unlock after the delay has elapsed.
        function unlock(address account, bytes calldata signature) external;

        /// Returns the lock state for an account.
        function getLockState(address account) external view returns (bool locked, uint32 unlockDelay, uint32 unlockRequestedAt);

        /// Returns the addresses of the native verifier contracts.
        function getNativeVerifiers() external view returns (address k1, address p256Raw, address p256WebAuthn, address delegate);

        /// Maps a verifier type byte to the corresponding native verifier address.
        function getVerifierAddress(uint8 verifierType) external view returns (address);

        /// ERC-1271 style signature verification via the account's owner configuration.
        function verifySignature(address account, bytes32 hash, bytes calldata auth) external view returns (bool valid, bytes32 ownerId, address verifier);
    }
}

sol! {
    /// Nonce Manager precompile interface. Provides read-only access to 2D nonces.
    /// Writes are protocol-only (performed by the node during execution).
    interface INonceManager {
        /// Returns the current nonce sequence for an account's nonce channel.
        function getNonce(address account, uint256 nonceKey) external view returns (uint64);
    }
}

sol! {
    /// Call tuple for TxContext return values.
    struct CallTuple {
        address target;
        bytes data;
    }

    /// Transaction context precompile interface. Provides read-only access to
    /// the current AA transaction's metadata during execution.
    interface ITxContext {
        /// Returns the sender (`from`) of the current AA transaction.
        function getSender() external view returns (address);

        /// Returns the payer of the current AA transaction.
        function getPayer() external view returns (address);

        /// Returns the authenticated owner ID of the sender.
        function getOwnerId() external view returns (bytes32);

        /// Returns the full phased calls array.
        function getCalls() external view returns (CallTuple[][] memory);

        /// Returns the maximum ETH cost: `(gas_limit + intrinsic) * max_fee_per_gas`.
        function getMaxCost() external view returns (uint256);

        /// Returns the execution gas budget.
        function getGasLimit() external view returns (uint256);
    }
}
