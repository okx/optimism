// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import {Vm} from "forge-std/Vm.sol";
import {TeeProofVerifier} from "src/dispute/tee/TeeProofVerifier.sol";
import {MockRiscZeroVerifier} from "test/dispute/tee/mocks/MockRiscZeroVerifier.sol";
import {TeeTestUtils} from "test/dispute/tee/helpers/TeeTestUtils.sol";

contract TeeProofVerifierTest is TeeTestUtils {
    MockRiscZeroVerifier internal riscZeroVerifier;
    TeeProofVerifier internal verifier;

    Vm.Wallet internal enclaveWallet;
    bytes32 internal constant IMAGE_ID = keccak256("tee-image");
    bytes32 internal constant PCR_HASH = keccak256("pcr-hash");
    bytes internal expectedRootKey;

    function setUp() public {
        riscZeroVerifier = new MockRiscZeroVerifier();
        expectedRootKey = abi.encodePacked(bytes32(uint256(1)), bytes32(uint256(2)), bytes32(uint256(3)));
        verifier = new TeeProofVerifier(riscZeroVerifier, IMAGE_ID, expectedRootKey);
        enclaveWallet = makeWallet(DEFAULT_EXECUTOR_KEY, "enclave");
    }

    function test_register_succeeds() public {
        bytes memory journal = buildJournal(1234, PCR_HASH, expectedRootKey, uncompressedPublicKey(enclaveWallet), "data");

        verifier.register(hex"1234", journal);

        (bytes32 pcrHash, uint64 registeredAt) = verifier.registeredEnclaves(enclaveWallet.addr);
        assertEq(pcrHash, PCR_HASH);
        assertEq(registeredAt, 1234);
        assertTrue(verifier.isRegistered(enclaveWallet.addr));
    }

    function test_register_revertUnauthorizedCaller() public {
        bytes memory journal = buildJournal(1234, PCR_HASH, expectedRootKey, uncompressedPublicKey(enclaveWallet), "");

        vm.prank(makeAddr("attacker"));
        vm.expectRevert(TeeProofVerifier.Unauthorized.selector);
        verifier.register(hex"1234", journal);
    }

    function test_register_revertInvalidProof() public {
        riscZeroVerifier.setShouldRevert(true);
        bytes memory journal = buildJournal(1234, PCR_HASH, expectedRootKey, uncompressedPublicKey(enclaveWallet), "");

        vm.expectRevert(TeeProofVerifier.InvalidProof.selector);
        verifier.register(hex"1234", journal);
    }

    function test_register_revertInvalidRootKey() public {
        bytes memory badRootKey = abi.encodePacked(bytes32(uint256(4)), bytes32(uint256(5)), bytes32(uint256(6)));
        bytes memory journal = buildJournal(1234, PCR_HASH, badRootKey, uncompressedPublicKey(enclaveWallet), "");

        vm.expectRevert(TeeProofVerifier.InvalidRootKey.selector);
        verifier.register(hex"1234", journal);
    }

    function test_register_revertInvalidPublicKey() public {
        bytes memory shortPublicKey = abi.encodePacked(bytes32(uint256(1)), bytes32(uint256(2)));
        bytes memory journal = buildJournal(1234, PCR_HASH, expectedRootKey, shortPublicKey, "");

        vm.expectRevert(TeeProofVerifier.InvalidPublicKey.selector);
        verifier.register(hex"1234", journal);
    }

    function test_register_revertDuplicateEnclave() public {
        bytes memory journal = buildJournal(1234, PCR_HASH, expectedRootKey, uncompressedPublicKey(enclaveWallet), "");
        verifier.register(hex"1234", journal);

        vm.expectRevert(TeeProofVerifier.EnclaveAlreadyRegistered.selector);
        verifier.register(hex"1234", journal);
    }

    function test_register_revertMalformedJournal() public {
        vm.expectRevert();
        verifier.register(hex"1234", hex"0001");
    }

    function test_verifyBatch_succeedsForRegisteredEnclave() public {
        bytes memory journal = buildJournal(1234, PCR_HASH, expectedRootKey, uncompressedPublicKey(enclaveWallet), "");
        verifier.register(hex"1234", journal);

        bytes32 digest = keccak256("batch");
        bytes memory signature = signDigest(enclaveWallet.privateKey, digest);

        assertEq(verifier.verifyBatch(digest, signature), enclaveWallet.addr);
    }

    function test_verifyBatch_revertForUnregisteredSigner() public {
        bytes32 digest = keccak256("batch");
        bytes memory signature = signDigest(enclaveWallet.privateKey, digest);

        vm.expectRevert(TeeProofVerifier.EnclaveNotRegistered.selector);
        verifier.verifyBatch(digest, signature);
    }

    function test_verifyBatch_revertForInvalidSignature() public {
        bytes memory journal = buildJournal(1234, PCR_HASH, expectedRootKey, uncompressedPublicKey(enclaveWallet), "");
        verifier.register(hex"1234", journal);

        vm.expectRevert(TeeProofVerifier.InvalidSignature.selector);
        verifier.verifyBatch(keccak256("batch"), hex"1234");
    }

    function test_revoke_succeeds() public {
        bytes memory journal = buildJournal(1234, PCR_HASH, expectedRootKey, uncompressedPublicKey(enclaveWallet), "");
        verifier.register(hex"1234", journal);

        verifier.revoke(enclaveWallet.addr);

        assertFalse(verifier.isRegistered(enclaveWallet.addr));
    }

    function test_revoke_revertWhenEnclaveMissing() public {
        vm.expectRevert(TeeProofVerifier.EnclaveNotRegistered.selector);
        verifier.revoke(enclaveWallet.addr);
    }

    function test_transferOwnership_updatesOwner() public {
        address newOwner = makeAddr("newOwner");
        verifier.transferOwnership(newOwner);
        assertEq(verifier.owner(), newOwner);
    }
}
