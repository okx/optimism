// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

/// @title IRiscZeroVerifier
/// @notice Minimal interface for the RISC Zero Groth16 verifier.
interface IRiscZeroVerifier {
    /// @notice Verify a RISC Zero proof.
    /// @param seal The proof seal (Groth16).
    /// @param imageId The guest image ID.
    /// @param journalDigest The SHA-256 digest of the journal.
    function verify(bytes calldata seal, bytes32 imageId, bytes32 journalDigest) external view;
}
