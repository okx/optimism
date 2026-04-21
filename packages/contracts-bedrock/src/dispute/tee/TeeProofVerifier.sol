// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

// Libraries
import { ECDSA } from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import { Ownable } from "@openzeppelin/contracts/access/Ownable.sol";

// Interfaces
import { IRiscZeroVerifier } from "interfaces/dispute/IRiscZeroVerifier.sol";

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
    ////////////////////////////////////////////////////////////////
    //                         Structs                            //
    ////////////////////////////////////////////////////////////////

    /// @notice Attestation data from the RISC Zero guest program
    struct AttestationData {
        uint64 timestampMs;
        bytes32 pcrHash;
        bytes publicKey; // 65 bytes secp256k1 uncompressed (0x04 + x + y)
        bytes userData;
    }

    ////////////////////////////////////////////////////////////////
    //                         State Vars                         //
    ////////////////////////////////////////////////////////////////

    /// @notice RISC Zero Groth16 verifier (only called during registration, immutable after deployment)
    IRiscZeroVerifier public immutable riscZeroVerifier;

    /// @notice RISC Zero guest image ID (hash of the attestation verification guest ELF, immutable after deployment)
    bytes32 public immutable imageId;

    /// @notice Expected AWS Nitro root public key (96 bytes, P384 without 0x04 prefix).
    ///         Set in constructor and never changed. Cannot use `immutable` keyword because Solidity
    ///         does not support immutable for dynamic `bytes` type.
    bytes public expectedRootKey;

    /// @notice Current enclave generation (starts at 1, increments on bulk revocation)
    uint256 public enclaveGeneration;

    /// @notice Generation at which each enclave was registered
    mapping(address => uint256) public enclaveRegisteredGeneration;

    /// @notice PCR hash recorded for each enclave (on-chain record only, not validated)
    mapping(address => bytes32) public enclavePcrHash;

    /// @notice Allowed proposer addresses.
    mapping(address => bool) public allowedProposers;

    /// @notice Allowed challenger addresses.
    mapping(address => bool) public allowedChallengers;

    ////////////////////////////////////////////////////////////////
    //                         Events                             //
    ////////////////////////////////////////////////////////////////

    event EnclaveRegistered(address indexed enclaveAddress, bytes32 indexed pcrHash, uint64 timestampMs);
    event EnclaveRevoked(address indexed enclaveAddress);
    event AllEnclavesRevoked(uint256 previousGeneration, uint256 newGeneration);
    event ProposerAdded(address indexed proposer);
    event ProposerRemoved(address indexed proposer);
    event ChallengerAdded(address indexed challenger);
    event ChallengerRemoved(address indexed challenger);

    ////////////////////////////////////////////////////////////////
    //                         Errors                             //
    ////////////////////////////////////////////////////////////////

    error InvalidProof();
    error InvalidPublicKey();
    error InvalidUserData();
    error EnclaveAlreadyRegistered();
    error EnclaveNotRegistered();
    error InvalidSignature();
    error AddressAlreadyAllowed();
    error AddressNotAllowed();

    ////////////////////////////////////////////////////////////////
    //                       Constructor                          //
    ////////////////////////////////////////////////////////////////

    /// @param _riscZeroVerifier RISC Zero verifier contract (Groth16 or mock)
    /// @param _imageId RISC Zero guest image ID
    /// @param _rootKey Expected AWS Nitro root public key (96 bytes)
    constructor(IRiscZeroVerifier _riscZeroVerifier, bytes32 _imageId, bytes memory _rootKey) {
        riscZeroVerifier = _riscZeroVerifier;
        imageId = _imageId;
        expectedRootKey = _rootKey;
        enclaveGeneration = 1;
    }

    ////////////////////////////////////////////////////////////////
    //                    Registration (Owner Only)               //
    ////////////////////////////////////////////////////////////////

    /// @notice Register a TEE enclave by verifying its ZK attestation proof.
    /// @dev Only callable by the owner. The journal is reconstructed on-chain from
    ///      attestationData + expectedRootKey. If the rootKey in the original attestation
    ///      differs from expectedRootKey, the reconstructed digest won't match and
    ///      the ZK proof verification will fail.
    /// @param seal The RISC Zero proof seal (Groth16)
    /// @param attestationData Attestation fields from the guest program
    function register(bytes calldata seal, AttestationData calldata attestationData) external onlyOwner {
        // 1. Validate public key length and uncompressed point prefix
        if (attestationData.publicKey.length != 65) {
            revert InvalidPublicKey();
        }
        if (attestationData.publicKey[0] != 0x04) {
            revert InvalidPublicKey();
        }

        // 2. Extract EOA address from secp256k1 public key
        address enclaveAddress = _extractAddress(attestationData.publicKey);

        // 3. Check not already registered in current generation
        if (enclaveRegisteredGeneration[enclaveAddress] == enclaveGeneration) {
            revert EnclaveAlreadyRegistered();
        }

        // 4. Validate userData length fits in uint16 before journal reconstruction
        if (attestationData.userData.length > type(uint16).max) {
            revert InvalidUserData();
        }

        // 5. Reconstruct journal digest (rootKey baked in from chain state)
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

        // 6. Verify ZK proof
        try riscZeroVerifier.verify(seal, imageId, journalDigest) { }
        catch {
            revert InvalidProof();
        }

        // 7. Store registration
        enclaveRegisteredGeneration[enclaveAddress] = enclaveGeneration;
        enclavePcrHash[enclaveAddress] = attestationData.pcrHash;

        emit EnclaveRegistered(enclaveAddress, attestationData.pcrHash, attestationData.timestampMs);
    }

    ////////////////////////////////////////////////////////////////
    //                 Batch Verification (Permissionless)        //
    ////////////////////////////////////////////////////////////////

    /// @notice Verify a batch state transition signed by a registered TEE enclave.
    /// @param digest The hash of the batch data (pre_batch, txs, post_batch, etc.)
    /// @param signature ECDSA signature (65 bytes: r + s + v)
    /// @return signer The address of the verified enclave that signed the batch
    function verifyBatch(bytes32 digest, bytes calldata signature) external view returns (address signer) {
        (address recovered, ECDSA.RecoverError err) = ECDSA.tryRecover(digest, signature);
        if (err != ECDSA.RecoverError.NoError || recovered == address(0)) {
            revert InvalidSignature();
        }

        if (enclaveRegisteredGeneration[recovered] != enclaveGeneration) {
            revert EnclaveNotRegistered();
        }

        return recovered;
    }

    ////////////////////////////////////////////////////////////////
    //                    Query Functions                         //
    ////////////////////////////////////////////////////////////////

    /// @notice Check if an address is a registered enclave
    function isRegistered(address enclaveAddress) external view returns (bool) {
        return enclaveRegisteredGeneration[enclaveAddress] == enclaveGeneration;
    }

    ////////////////////////////////////////////////////////////////
    //                    Admin Functions                         //
    ////////////////////////////////////////////////////////////////

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

    /// @notice Add an address to the proposer whitelist.
    function addProposer(address _proposer) external onlyOwner {
        if (allowedProposers[_proposer]) revert AddressAlreadyAllowed();
        allowedProposers[_proposer] = true;
        emit ProposerAdded(_proposer);
    }

    /// @notice Remove an address from the proposer whitelist.
    function removeProposer(address _proposer) external onlyOwner {
        if (!allowedProposers[_proposer]) revert AddressNotAllowed();
        allowedProposers[_proposer] = false;
        emit ProposerRemoved(_proposer);
    }

    /// @notice Add an address to the challenger whitelist.
    function addChallenger(address _challenger) external onlyOwner {
        if (allowedChallengers[_challenger]) revert AddressAlreadyAllowed();
        allowedChallengers[_challenger] = true;
        emit ChallengerAdded(_challenger);
    }

    /// @notice Remove an address from the challenger whitelist.
    function removeChallenger(address _challenger) external onlyOwner {
        if (!allowedChallengers[_challenger]) revert AddressNotAllowed();
        allowedChallengers[_challenger] = false;
        emit ChallengerRemoved(_challenger);
    }

    ////////////////////////////////////////////////////////////////
    //                    Internal Functions                      //
    ////////////////////////////////////////////////////////////////

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
