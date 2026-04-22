// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import { ECDSA } from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import { ITeeProofVerifier } from "interfaces/dispute/ITeeProofVerifier.sol";

contract MockTeeProofVerifier is ITeeProofVerifier {
    error EnclaveNotRegistered();
    error InvalidSignature();

    mapping(address => bool) public registered;
    mapping(address => bool) public allowedProposers;
    mapping(address => bool) public allowedChallengers;
    bytes32 public lastDigest;
    bytes public lastSignature;

    function setRegistered(address enclave, bool value) external {
        registered[enclave] = value;
    }

    function setAllowedProposer(address proposer, bool value) external {
        allowedProposers[proposer] = value;
    }

    function setAllowedChallenger(address challenger, bool value) external {
        allowedChallengers[challenger] = value;
    }

    function verifyBatch(bytes32 digest, bytes calldata signature) external view returns (address signer) {
        (address recovered, ECDSA.RecoverError err) = ECDSA.tryRecover(digest, signature);
        if (err != ECDSA.RecoverError.NoError || recovered == address(0)) revert InvalidSignature();
        if (!registered[recovered]) revert EnclaveNotRegistered();
        return recovered;
    }

    function verifyBatchAndRecord(bytes32 digest, bytes calldata signature) external returns (address signer) {
        lastDigest = digest;
        lastSignature = signature;
        return this.verifyBatch(digest, signature);
    }

    function isRegistered(address enclaveAddress) external view returns (bool) {
        return registered[enclaveAddress];
    }
}
