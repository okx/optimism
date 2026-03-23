// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import {IRiscZeroVerifier} from "interfaces/dispute/IRiscZeroVerifier.sol";
import {ECDSA} from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";

/// @title TEE Proof Verifier for OP Stack DisputeGame
/// @notice Verifies TEE enclave identity via ZK proof (owner-gated registration) and
///         batch state transitions via ECDSA signature (permissionless verification).
/// @dev Two core responsibilities:
///      1. register(): Owner verifies ZK proof of Nitro attestation, binds EOA <-> PCR on-chain
///      2. verifyBatch(): ecrecover signature, check signer is a registered enclave
///
///      Journal format (from RISC Zero guest program):
///      - 8 bytes: timestamp_ms (big-endian uint64)
///      - 32 bytes: pcr_hash = SHA256(PCR0)
///      - 96 bytes: root_pubkey (P384 without 0x04 prefix)
///      - 1 byte: pubkey_len
///      - pubkey_len bytes: pubkey (secp256k1 uncompressed, 65 bytes)
///      - 2 bytes: user_data_len (big-endian uint16)
///      - user_data_len bytes: user_data
contract TeeProofVerifier is Ownable {
    // ============ Immutables ============

    /// @notice RISC Zero Groth16 verifier (only called during registration)
    IRiscZeroVerifier public immutable riscZeroVerifier;

    /// @notice RISC Zero guest image ID (hash of the attestation verification guest ELF)
    bytes32 public immutable imageId;

    /// @notice Expected AWS Nitro root public key (96 bytes, P384 without 0x04 prefix)
    bytes public expectedRootKey;

    // ============ State ============

    struct EnclaveInfo {
        bytes32 pcrHash;
        uint64 registeredAt;
    }

    /// @notice Registered enclaves: EOA address => enclave info
    mapping(address => EnclaveInfo) public registeredEnclaves;

    // ============ Events ============

    event EnclaveRegistered(
        address indexed enclaveAddress, bytes32 indexed pcrHash, uint64 timestampMs
    );

    event EnclaveRevoked(address indexed enclaveAddress);

    // ============ Errors ============

    error InvalidProof();
    error InvalidRootKey();
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
    }

    // ============ Registration (Owner Only) ============

    /// @notice Register a TEE enclave by verifying its ZK attestation proof.
    /// @dev Only callable by the owner. The owner calling register() is the trust gate --
    ///      the PCR and EOA from the proof are automatically trusted upon registration.
    /// @param seal The RISC Zero proof seal (Groth16)
    /// @param journal The journal output from the guest program
    function register(bytes calldata seal, bytes calldata journal) external onlyOwner {
        // 1. Verify ZK proof
        bytes32 journalDigest = sha256(journal);
        try riscZeroVerifier.verify(seal, imageId, journalDigest) {}
        catch {
            revert InvalidProof();
        }

        // 2. Parse journal
        (
            uint64 timestampMs,
            bytes32 pcrHash,
            bytes memory rootKey,
            bytes memory publicKey,
        ) = _parseJournal(journal);

        // 3. Verify root key matches AWS Nitro official root
        if (keccak256(rootKey) != keccak256(expectedRootKey)) {
            revert InvalidRootKey();
        }

        // 4. Extract EOA address from secp256k1 public key (65 bytes: 0x04 + x + y)
        if (publicKey.length != 65) {
            revert InvalidPublicKey();
        }
        address enclaveAddress = _extractAddress(publicKey);

        // 5. Check not already registered
        if (registeredEnclaves[enclaveAddress].registeredAt != 0) {
            revert EnclaveAlreadyRegistered();
        }

        // 6. Store registration (PCR is implicitly trusted by owner's approval)
        registeredEnclaves[enclaveAddress] =
            EnclaveInfo({pcrHash: pcrHash, registeredAt: timestampMs});

        emit EnclaveRegistered(enclaveAddress, pcrHash, timestampMs);
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
        // 1. Recover signer from signature
        (address recovered, ECDSA.RecoverError err) = ECDSA.tryRecover(digest, signature);
        if (err != ECDSA.RecoverError.NoError || recovered == address(0)) {
            revert InvalidSignature();
        }

        // 2. Check signer is a registered enclave
        if (registeredEnclaves[recovered].registeredAt == 0) {
            revert EnclaveNotRegistered();
        }

        return recovered;
    }

    // ============ Query Functions ============

    /// @notice Check if an address is a registered enclave
    /// @param enclaveAddress The address to check
    /// @return True if the address is registered
    function isRegistered(address enclaveAddress) external view returns (bool) {
        return registeredEnclaves[enclaveAddress].registeredAt != 0;
    }

    // ============ Admin Functions ============

    /// @notice Revoke a registered enclave
    /// @param enclaveAddress The enclave address to revoke
    function revoke(address enclaveAddress) external onlyOwner {
        if (registeredEnclaves[enclaveAddress].registeredAt == 0) {
            revert EnclaveNotRegistered();
        }
        delete registeredEnclaves[enclaveAddress];
        emit EnclaveRevoked(enclaveAddress);
    }

    // ============ Internal Functions ============

    /// @notice Parse the journal bytes into attestation fields
    function _parseJournal(bytes calldata journal)
        internal
        pure
        returns (
            uint64 timestampMs,
            bytes32 pcrHash,
            bytes memory rootKey,
            bytes memory publicKey,
            bytes memory userData
        )
    {
        uint256 offset = 0;

        timestampMs = uint64(bytes8(journal[offset:offset + 8]));
        offset += 8;

        pcrHash = bytes32(journal[offset:offset + 32]);
        offset += 32;

        rootKey = journal[offset:offset + 96];
        offset += 96;

        uint8 pubkeyLen = uint8(journal[offset]);
        offset += 1;

        publicKey = journal[offset:offset + pubkeyLen];
        offset += pubkeyLen;

        uint16 userDataLen = uint16(bytes2(journal[offset:offset + 2]));
        offset += 2;

        userData = journal[offset:offset + userDataLen];
    }

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
