// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import { Vm } from "forge-std/Vm.sol";
import { TeeProofVerifier } from "src/dispute/tee/TeeProofVerifier.sol";
import { MockRiscZeroVerifier } from "test/dispute/tee/mocks/MockRiscZeroVerifier.sol";
import { TeeTestUtils } from "test/dispute/tee/helpers/TeeTestUtils.sol";

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

    // ============ Helper ============

    function _buildAttestationData(
        uint64 timestampMs,
        bytes32 pcrHash,
        bytes memory publicKey,
        bytes memory userData
    )
        internal
        pure
        returns (TeeProofVerifier.AttestationData memory)
    {
        return TeeProofVerifier.AttestationData({
            timestampMs: timestampMs,
            pcrHash: pcrHash,
            publicKey: publicKey,
            userData: userData
        });
    }

    function _registerEnclave() internal {
        TeeProofVerifier.AttestationData memory data =
            _buildAttestationData(1234, PCR_HASH, uncompressedPublicKey(enclaveWallet), "data");
        verifier.register(hex"1234", data);
    }

    // ============ Register Tests ============

    function test_register_succeeds() public {
        TeeProofVerifier.AttestationData memory data =
            _buildAttestationData(1234, PCR_HASH, uncompressedPublicKey(enclaveWallet), "data");

        verifier.register(hex"1234", data);

        assertEq(verifier.enclavePcrHash(enclaveWallet.addr), PCR_HASH);
        assertEq(verifier.enclaveRegisteredGeneration(enclaveWallet.addr), verifier.enclaveGeneration());
        assertTrue(verifier.isRegistered(enclaveWallet.addr));
    }

    function test_register_revertUnauthorizedCaller() public {
        TeeProofVerifier.AttestationData memory data =
            _buildAttestationData(1234, PCR_HASH, uncompressedPublicKey(enclaveWallet), "");

        vm.prank(makeAddr("attacker"));
        vm.expectRevert("Ownable: caller is not the owner");
        verifier.register(hex"1234", data);
    }

    function test_register_revertInvalidProof() public {
        riscZeroVerifier.setShouldRevert(true);
        TeeProofVerifier.AttestationData memory data =
            _buildAttestationData(1234, PCR_HASH, uncompressedPublicKey(enclaveWallet), "");

        vm.expectRevert(TeeProofVerifier.InvalidProof.selector);
        verifier.register(hex"1234", data);
    }

    function test_register_revertInvalidPublicKey() public {
        bytes memory shortPublicKey = abi.encodePacked(bytes32(uint256(1)), bytes32(uint256(2)));
        TeeProofVerifier.AttestationData memory data = _buildAttestationData(1234, PCR_HASH, shortPublicKey, "");

        vm.expectRevert(TeeProofVerifier.InvalidPublicKey.selector);
        verifier.register(hex"1234", data);
    }

    function test_register_revertDuplicateEnclave() public {
        _registerEnclave();

        TeeProofVerifier.AttestationData memory data =
            _buildAttestationData(1234, PCR_HASH, uncompressedPublicKey(enclaveWallet), "data");

        vm.expectRevert(TeeProofVerifier.EnclaveAlreadyRegistered.selector);
        verifier.register(hex"1234", data);
    }

    // ============ VerifyBatch Tests ============

    function test_verifyBatch_succeedsForRegisteredEnclave() public {
        _registerEnclave();

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
        _registerEnclave();

        vm.expectRevert(TeeProofVerifier.InvalidSignature.selector);
        verifier.verifyBatch(keccak256("batch"), hex"1234");
    }

    // ============ Revoke Tests ============

    function test_revoke_succeeds() public {
        _registerEnclave();

        verifier.revoke(enclaveWallet.addr);

        assertFalse(verifier.isRegistered(enclaveWallet.addr));
    }

    function test_revoke_revertWhenEnclaveMissing() public {
        vm.expectRevert(TeeProofVerifier.EnclaveNotRegistered.selector);
        verifier.revoke(enclaveWallet.addr);
    }

    function test_revoke_revertNonOwner() public {
        _registerEnclave();
        vm.prank(makeAddr("attacker"));
        vm.expectRevert("Ownable: caller is not the owner");
        verifier.revoke(enclaveWallet.addr);
    }

    function test_revoke_verifyBatchFailsAfterRevoke() public {
        _registerEnclave();
        bytes32 digest = keccak256("batch");
        bytes memory signature = signDigest(enclaveWallet.privateKey, digest);
        assertEq(verifier.verifyBatch(digest, signature), enclaveWallet.addr);

        verifier.revoke(enclaveWallet.addr);

        vm.expectRevert(TeeProofVerifier.EnclaveNotRegistered.selector);
        verifier.verifyBatch(digest, signature);
    }

    function test_revoke_doubleRevokeReverts() public {
        _registerEnclave();
        verifier.revoke(enclaveWallet.addr);

        vm.expectRevert(TeeProofVerifier.EnclaveNotRegistered.selector);
        verifier.revoke(enclaveWallet.addr);
    }

    function test_revoke_canReRegisterAfterRevoke() public {
        _registerEnclave();
        verifier.revoke(enclaveWallet.addr);
        assertFalse(verifier.isRegistered(enclaveWallet.addr));

        _registerEnclave();
        assertTrue(verifier.isRegistered(enclaveWallet.addr));
    }

    // ============ RevokeAll Tests ============

    function test_revokeAll_invalidatesAllEnclaves() public {
        _registerEnclave();
        assertTrue(verifier.isRegistered(enclaveWallet.addr));

        uint256 oldGen = verifier.enclaveGeneration();
        verifier.revokeAll();

        assertEq(verifier.enclaveGeneration(), oldGen + 1);
        assertFalse(verifier.isRegistered(enclaveWallet.addr));
    }

    function test_revokeAll_enclaveCanReRegister() public {
        _registerEnclave();
        verifier.revokeAll();
        assertFalse(verifier.isRegistered(enclaveWallet.addr));

        // Re-register after generation bump
        _registerEnclave();
        assertTrue(verifier.isRegistered(enclaveWallet.addr));
    }

    function test_revokeAll_revertNonOwner() public {
        vm.prank(makeAddr("attacker"));
        vm.expectRevert("Ownable: caller is not the owner");
        verifier.revokeAll();
    }

    function test_revokeAll_verifyBatchFailsAfterRevoke() public {
        _registerEnclave();
        bytes32 digest = keccak256("batch");
        bytes memory signature = signDigest(enclaveWallet.privateKey, digest);

        // Works before revokeAll
        assertEq(verifier.verifyBatch(digest, signature), enclaveWallet.addr);

        verifier.revokeAll();

        // Fails after revokeAll
        vm.expectRevert(TeeProofVerifier.EnclaveNotRegistered.selector);
        verifier.verifyBatch(digest, signature);
    }

    // ============ Immutability Tests ============

    function test_riscZeroVerifier_isImmutable() public view {
        assertEq(address(verifier.riscZeroVerifier()), address(riscZeroVerifier));
    }

    function test_imageId_isImmutable() public view {
        assertEq(verifier.imageId(), IMAGE_ID);
    }

    function test_expectedRootKey_isSetInConstructor() public view {
        assertEq(keccak256(verifier.expectedRootKey()), keccak256(expectedRootKey));
    }

    // ============ Ownership Tests ============

    function test_transferOwnership_updatesOwner() public {
        address newOwner = makeAddr("newOwner");
        verifier.transferOwnership(newOwner);
        assertEq(verifier.owner(), newOwner);
    }

    function test_transferOwnership_revertZeroAddress() public {
        vm.expectRevert("Ownable: new owner is the zero address");
        verifier.transferOwnership(address(0));
    }
}
