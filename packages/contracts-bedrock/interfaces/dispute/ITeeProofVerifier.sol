// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

/// @title ITeeProofVerifier
/// @notice Interface for the TEE Proof Verifier contract.
interface ITeeProofVerifier {
    /// @notice Verify a batch state transition signed by a registered TEE enclave.
    /// @param digest The hash of the batch data.
    /// @param signature ECDSA signature (65 bytes: r + s + v).
    /// @return signer The address of the verified enclave.
    function verifyBatch(bytes32 digest, bytes calldata signature) external view returns (address signer);

    /// @notice Check if an address is a registered enclave.
    /// @param enclaveAddress The address to check.
    /// @return True if the address is registered.
    function isRegistered(address enclaveAddress) external view returns (bool);
}
