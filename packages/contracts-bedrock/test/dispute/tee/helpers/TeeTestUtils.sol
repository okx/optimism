// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import { Test } from "forge-std/Test.sol";
import { Vm } from "forge-std/Vm.sol";
import { Claim } from "src/dispute/lib/Types.sol";
import { TeeDisputeGame } from "src/dispute/tee/TeeDisputeGame.sol";

abstract contract TeeTestUtils is Test {
    uint256 internal constant DEFAULT_PROPOSER_KEY = 0xA11CE;
    uint256 internal constant DEFAULT_CHALLENGER_KEY = 0xB0B;
    uint256 internal constant DEFAULT_EXECUTOR_KEY = 0xC0DE;
    uint256 internal constant DEFAULT_THIRD_PARTY_PROVER_KEY = 0xD00D;

    struct BatchInput {
        bytes32 startBlockHash;
        bytes32 startStateHash;
        bytes32 endBlockHash;
        bytes32 endStateHash;
        uint256 l2Block;
    }

    function buildExtraData(
        uint256 l2SequenceNumber,
        uint32 parentIndex,
        bytes32 blockHash_,
        bytes32 stateHash_
    )
        internal
        pure
        returns (bytes memory)
    {
        return abi.encodePacked(l2SequenceNumber, parentIndex, blockHash_, stateHash_);
    }

    function computeRootClaim(bytes32 blockHash_, bytes32 stateHash_) internal pure returns (Claim) {
        return Claim.wrap(keccak256(abi.encode(blockHash_, stateHash_)));
    }

    bytes32 private constant BATCH_PROOF_TYPEHASH = keccak256(
        "BatchProof(bytes32 startBlockHash,bytes32 startStateHash,bytes32 endBlockHash,bytes32 endStateHash,uint256 l2Block)"
    );

    bytes32 private constant DOMAIN_TYPEHASH =
        keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)");
    bytes32 private constant DOMAIN_NAME_HASH = keccak256("TeeDisputeGame");
    bytes32 private constant DOMAIN_VERSION_HASH = keccak256("1");

    function computeBatchStructHash(BatchInput memory batch) internal pure returns (bytes32) {
        return keccak256(
            abi.encode(
                BATCH_PROOF_TYPEHASH,
                batch.startBlockHash,
                batch.startStateHash,
                batch.endBlockHash,
                batch.endStateHash,
                batch.l2Block
            )
        );
    }

    function computeDomainSeparator(address verifier) internal view returns (bytes32) {
        return keccak256(abi.encode(DOMAIN_TYPEHASH, DOMAIN_NAME_HASH, DOMAIN_VERSION_HASH, block.chainid, verifier));
    }

    function computeEIP712Digest(BatchInput memory batch, bytes32 domainSeparator) internal pure returns (bytes32) {
        return keccak256(abi.encodePacked("\x19\x01", domainSeparator, computeBatchStructHash(batch)));
    }

    function signDigest(uint256 privateKey, bytes32 digest) internal pure returns (bytes memory) {
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(privateKey, digest);
        return abi.encodePacked(r, s, v);
    }

    function buildBatchProof(
        BatchInput memory batch,
        uint256 privateKey,
        bytes32 domainSeparator
    )
        internal
        returns (TeeDisputeGame.BatchProof memory)
    {
        return TeeDisputeGame.BatchProof({
            startBlockHash: batch.startBlockHash,
            startStateHash: batch.startStateHash,
            endBlockHash: batch.endBlockHash,
            endStateHash: batch.endStateHash,
            l2Block: batch.l2Block,
            signature: signDigest(privateKey, computeEIP712Digest(batch, domainSeparator))
        });
    }

    function buildBatchProofWithSignature(
        BatchInput memory batch,
        bytes memory signature
    )
        internal
        pure
        returns (TeeDisputeGame.BatchProof memory)
    {
        return TeeDisputeGame.BatchProof({
            startBlockHash: batch.startBlockHash,
            startStateHash: batch.startStateHash,
            endBlockHash: batch.endBlockHash,
            endStateHash: batch.endStateHash,
            l2Block: batch.l2Block,
            signature: signature
        });
    }

    function makeWallet(uint256 privateKey, string memory label) internal returns (Vm.Wallet memory wallet) {
        wallet = vm.createWallet(privateKey, label);
    }

    function uncompressedPublicKey(Vm.Wallet memory wallet) internal pure returns (bytes memory) {
        return abi.encodePacked(bytes1(0x04), bytes32(wallet.publicKeyX), bytes32(wallet.publicKeyY));
    }

    function buildJournal(
        uint64 timestampMs,
        bytes32 pcrHash,
        bytes memory rootKey,
        bytes memory publicKey,
        bytes memory userData
    )
        internal
        pure
        returns (bytes memory)
    {
        return abi.encodePacked(
            bytes8(timestampMs),
            pcrHash,
            rootKey,
            bytes1(uint8(publicKey.length)),
            publicKey,
            bytes2(uint16(userData.length)),
            userData
        );
    }
}
