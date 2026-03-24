// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import {IRiscZeroVerifier} from "interfaces/dispute/IRiscZeroVerifier.sol";
import {ECDSA} from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";

/// @title TEE Proof Verifier for OP Stack DisputeGame
/// @notice Verifies TEE enclave identity via ZK proof (owner-gated registration) and
///         batch state transitions via ECDSA signature (permissionless verification).
/// @dev Two core responsibilities:
///      1. register(): Owner verifies ZK proof of Nitro attestation, binds EOA on-chain
///      2. verifyBatch(): ecrecover signature, check signer is a registered enclave
///
///      Uses generation-based revocation: incrementing enclaveGeneration invalidates
///      all previously registered enclaves in O(1).
///
///      Journal is reconstructed on-chain from AttestationData + expectedRootKey,
///      so rootKey mismatch causes ZK verify failure without explicit comparison.
contract TeeProofVerifier is Ownable {
    // ============ Structs ============

    /// @notice Attestation data from the RISC Zero guest program
    struct AttestationData {
        uint64 timestampMs;
        bytes32 pcrHash;
        bytes publicKey; // 65 bytes secp256k1 uncompressed (0x04 + x + y)
        bytes userData;
    }

    // ============ State ============

    /// @notice RISC Zero Groth16 verifier (only called during registration)
    IRiscZeroVerifier public riscZeroVerifier;

    /// @notice RISC Zero guest image ID (hash of the attestation verification guest ELF)
    bytes32 public imageId;

    /// @notice Expected AWS Nitro root public key (96 bytes, P384 without 0x04 prefix)
    bytes public expectedRootKey;

    /// @notice Current enclave generation (starts at 1, increments on bulk revocation)
    uint256 public enclaveGeneration;

    /// @notice Generation at which each enclave was registered
    mapping(address => uint256) public enclaveRegisteredGeneration;

    /// @notice PCR hash recorded for each enclave (on-chain record only, not validated)
    mapping(address => bytes32) public enclavePcrHash;

    // ============ Events ============

    event EnclaveRegistered(address indexed enclaveAddress, bytes32 indexed pcrHash, uint64 timestampMs);
    event EnclaveRevoked(address indexed enclaveAddress);
    event AllEnclavesRevoked(uint256 previousGeneration, uint256 newGeneration);
    event RiscZeroVerifierUpdated(IRiscZeroVerifier indexed oldVerifier, IRiscZeroVerifier indexed newVerifier);
    event ImageIdUpdated(bytes32 indexed oldImageId, bytes32 indexed newImageId);
    event ExpectedRootKeyUpdated(bytes oldKey, bytes newKey);

    // ============ Errors ============

    error InvalidProof();
    error InvalidPublicKey();
    error EnclaveAlreadyRegistered();
    error EnclaveNotRegistered();
    error InvalidSignature();

    // ============ Constructor ============

    /// @param _riscZeroVerifier RISC Zero verifier contract (Groth16 or mock)
    /// @param _imageId RISC Zero guest image ID
    /// @param _rootKey Expected AWS Nitro root public key (96 bytes)
    constructor(
        IRiscZeroVerifier _riscZeroVerifier,
        bytes32 _imageId,
        bytes memory _rootKey
    ) {
        riscZeroVerifier = _riscZeroVerifier;
        imageId = _imageId;
        expectedRootKey = _rootKey;
        enclaveGeneration = 1;
    }

    // ============ Registration (Owner Only) ============

    /// @notice Register a TEE enclave by verifying its ZK attestation proof.
    /// @dev Only callable by the owner. The journal is reconstructed on-chain from
    ///      attestationData + expectedRootKey. If the rootKey in the original attestation
    ///      differs from expectedRootKey, the reconstructed digest won't match and
    ///      the ZK proof verification will fail.
    /// @param seal The RISC Zero proof seal (Groth16)
    /// @param attestationData Attestation fields from the guest program
    function register(bytes calldata seal, AttestationData calldata attestationData) external onlyOwner {
        // 1. Validate public key length
        if (attestationData.publicKey.length != 65) {
            revert InvalidPublicKey();
        }

        // 2. Extract EOA address from secp256k1 public key
        address enclaveAddress = _extractAddress(attestationData.publicKey);

        // 3. Check not already registered in current generation
        if (enclaveRegisteredGeneration[enclaveAddress] == enclaveGeneration) {
            revert EnclaveAlreadyRegistered();
        }

        // 4. Reconstruct journal digest (rootKey baked in from chain state)
        bytes32 journalDigest = sha256(
            abi.encodePacked(
                attestationData.timestampMs,
                attestationData.pcrHash,
                expectedRootKey,
                uint8(attestationData.publicKey.length),
                attestationData.publicKey,
                uint16(attestationData.userData.length),
                attestationData.userData
            )
        );

        // 5. Verify ZK proof
        try riscZeroVerifier.verify(seal, imageId, journalDigest) {}
        catch {
            revert InvalidProof();
        }

        // 6. Store registration
        enclaveRegisteredGeneration[enclaveAddress] = enclaveGeneration;
        enclavePcrHash[enclaveAddress] = attestationData.pcrHash;

        emit EnclaveRegistered(enclaveAddress, attestationData.pcrHash, attestationData.timestampMs);
    }

    // ============ Batch Verification (Permissionless) ============

    /// @notice Verify a batch state transition signed by a registered TEE enclave.
    /// @param digest The hash of the batch data (pre_batch, txs, post_batch, etc.)
    /// @param signature ECDSA signature (65 bytes: r + s + v)
    /// @return signer The address of the verified enclave that signed the batch
    function verifyBatch(bytes32 digest, bytes calldata signature)
        external
        view
        returns (address signer)
    {
        (address recovered, ECDSA.RecoverError err) = ECDSA.tryRecover(digest, signature);
        if (err != ECDSA.RecoverError.NoError || recovered == address(0)) {
            revert InvalidSignature();
        }

        if (enclaveRegisteredGeneration[recovered] != enclaveGeneration) {
            revert EnclaveNotRegistered();
        }

        return recovered;
    }

    // ============ Query Functions ============

    /// @notice Check if an address is a registered enclave
    function isRegistered(address enclaveAddress) external view returns (bool) {
        return enclaveRegisteredGeneration[enclaveAddress] == enclaveGeneration;
    }

    // ============ Admin Functions ============

    /// @notice Revoke a single registered enclave
    function revoke(address enclaveAddress) external onlyOwner {
        if (enclaveRegisteredGeneration[enclaveAddress] != enclaveGeneration) {
            revert EnclaveNotRegistered();
        }
        enclaveRegisteredGeneration[enclaveAddress] = 0;
        emit EnclaveRevoked(enclaveAddress);
    }

    /// @notice Revoke all registered enclaves by incrementing the generation counter
    function revokeAll() external onlyOwner {
        uint256 previousGeneration = enclaveGeneration;
        enclaveGeneration = previousGeneration + 1;
        emit AllEnclavesRevoked(previousGeneration, enclaveGeneration);
    }

    /// @notice Update the RISC Zero verifier contract
    function setRiscZeroVerifier(IRiscZeroVerifier _verifier) external onlyOwner {
        IRiscZeroVerifier oldVerifier = riscZeroVerifier;
        riscZeroVerifier = _verifier;
        emit RiscZeroVerifierUpdated(oldVerifier, _verifier);
    }

    /// @notice Update the RISC Zero guest image ID
    function setImageId(bytes32 _imageId) external onlyOwner {
        bytes32 oldImageId = imageId;
        imageId = _imageId;
        emit ImageIdUpdated(oldImageId, _imageId);
    }

    /// @notice Update the expected AWS Nitro root public key
    function setExpectedRootKey(bytes memory _rootKey) external onlyOwner {
        bytes memory oldKey = expectedRootKey;
        expectedRootKey = _rootKey;
        emit ExpectedRootKeyUpdated(oldKey, _rootKey);
    }

    // ============ Internal Functions ============

    /// @notice Extract Ethereum address from secp256k1 uncompressed public key
    /// @param publicKey 65 bytes: 0x04 prefix + 32-byte x + 32-byte y
    function _extractAddress(bytes memory publicKey) internal pure returns (address) {
        bytes memory coordinates = new bytes(64);
        for (uint256 i = 0; i < 64; i++) {
            coordinates[i] = publicKey[i + 1];
        }
        return address(uint160(uint256(keccak256(coordinates))));
    }
}
