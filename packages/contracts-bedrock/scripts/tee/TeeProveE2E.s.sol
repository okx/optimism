// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import { Script, console2 } from "forge-std/Script.sol";
import { Vm } from "forge-std/Vm.sol";
import { IDisputeGameFactory } from "interfaces/dispute/IDisputeGameFactory.sol";
import { IDisputeGame } from "interfaces/dispute/IDisputeGame.sol";
import { TeeDisputeGame, TEE_DISPUTE_GAME_TYPE } from "src/dispute/tee/TeeDisputeGame.sol";
import { TeeProofVerifier } from "src/dispute/tee/TeeProofVerifier.sol";
import { Claim, GameType, GameStatus } from "src/dispute/lib/Types.sol";

/// @title TeeProveE2E
/// @notice End-to-end script: register enclave, create game, challenge, prove, resolve.
///         Two modes:
///           - Mock mode (default): ENCLAVE_KEY signs batch on-chain via vm.sign
///           - External mode: set BATCH_SIGNATURE to use a pre-built signature from TEE prover
///
/// @dev Prerequisites: Run DeployTeeMock.s.sol first, then set the env vars below.
///
///   # Required
///   PRIVATE_KEY=<owner key>
///   PROPOSER_KEY=<proposer key>
///   CHALLENGER_KEY=<challenger key>
///   TEE_PROOF_VERIFIER=<address>
///   DISPUTE_GAME_FACTORY=<address>
///
///   # Mock mode (script signs with enclave key):
///   ENCLAVE_KEY=0x...
///
///   # External mode (prover signs off-chain, passes signature in):
///   BATCH_SIGNATURE=0x<65-byte r+s+v hex>
///   ENCLAVE_ADDR=0x<registered enclave address, for skip-register check>
///   # ENCLAVE_KEY is not needed in this mode
///
///   # Optional: override batch data (defaults to mock values if not set)
///   # START_BLOCK_HASH=0x...     (default: keccak256("genesis-block"), must match anchor)
///   # START_STATE_HASH=0x...     (default: keccak256("genesis-state"), must match anchor)
///   # END_BLOCK_HASH=0x...       (default: keccak256("end-block-100"))
///   # END_STATE_HASH=0x...       (default: keccak256("end-state-100"))
///   # L2_SEQUENCE_NUMBER=100     (default: 100)
///
///   forge script scripts/tee/TeeProveE2E.s.sol --rpc-url http://localhost:8545 --broadcast
contract TeeProveE2E is Script {
    bytes32 private constant BATCH_PROOF_TYPEHASH = keccak256(
        "BatchProof(bytes32 startBlockHash,bytes32 startStateHash,bytes32 endBlockHash,bytes32 endStateHash,uint256 l2Block)"
    );

    GameType internal constant TEE_GAME_TYPE = GameType.wrap(TEE_DISPUTE_GAME_TYPE);

    // Stored after env reads, used across steps
    TeeProofVerifier internal teeProofVerifier;
    IDisputeGameFactory internal factory;
    TeeDisputeGame internal game;

    // Prove inputs
    bytes32 internal startBlockHash;
    bytes32 internal startStateHash;
    bytes32 internal endBlockHash;
    bytes32 internal endStateHash;
    uint256 internal l2SeqNum;

    function run() external {
        // --- Read env ---
        uint256 ownerKey = vm.envUint("PRIVATE_KEY");
        uint256 proposerKey = vm.envUint("PROPOSER_KEY");
        uint256 challengerKey = vm.envUint("CHALLENGER_KEY");

        teeProofVerifier = TeeProofVerifier(vm.envAddress("TEE_PROOF_VERIFIER"));
        factory = IDisputeGameFactory(vm.envAddress("DISPUTE_GAME_FACTORY"));

        // Prove inputs: env override or mock defaults
        startBlockHash = vm.envOr("START_BLOCK_HASH", keccak256("genesis-block"));
        startStateHash = vm.envOr("START_STATE_HASH", keccak256("genesis-state"));
        endBlockHash = vm.envOr("END_BLOCK_HASH", keccak256("end-block-100"));
        endStateHash = vm.envOr("END_STATE_HASH", keccak256("end-state-100"));
        l2SeqNum = vm.envOr("L2_SEQUENCE_NUMBER", uint256(100));

        // Determine mode: external signature or mock (enclave key signs locally)
        bytes memory externalSig = vm.envOr("BATCH_SIGNATURE", bytes(""));
        bool externalMode = externalSig.length > 0;

        // Step 1: Register enclave
        console2.log("=== Step 1: Register Enclave (mock attestation + mock ZK proof) ===");
        if (externalMode) {
            // External mode: enclave already registered by prover, just verify
            address enclaveAddr = vm.envAddress("ENCLAVE_ADDR");
            require(teeProofVerifier.isRegistered(enclaveAddr), "ENCLAVE_ADDR not registered");
            console2.log("Enclave verified registered:", enclaveAddr);
        } else {
            uint256 enclaveKey = vm.envUint("ENCLAVE_KEY");
            _registerEnclave(ownerKey, enclaveKey);
        }

        // Step 2: Create game
        console2.log("");
        console2.log("=== Step 2: Create Game (proposer) ===");
        _createGame(proposerKey);

        // Step 3: Challenge
        console2.log("");
        console2.log("=== Step 3: Challenge (challenger) ===");
        _challenge(challengerKey);

        // Step 4: Prove
        console2.log("");
        if (externalMode) {
            console2.log("=== Step 4: Prove - external signature from TEE prover ===");
            _proveExternal(proposerKey, externalSig);
        } else {
            console2.log("=== Step 4: Prove - mock mode (enclave key signs locally) ===");
            uint256 enclaveKey = vm.envUint("ENCLAVE_KEY");
            _proveMock(proposerKey, enclaveKey);
        }

        // Step 5: Resolve
        console2.log("");
        console2.log("=== Step 5: Resolve ===");
        _resolve(proposerKey);

        console2.log("");
        console2.log("=== E2E Complete (steps 1-5 passed) ===");
        console2.log("");
        console2.log("Note: claimCredit requires finality delay to pass (resolvedAt + delay < block.timestamp).");
        console2.log("In forge script all txns share the same block, so claimCredit must be called separately:");
        console2.log(
            "  cast send <game> 'claimCredit(address)' <proposer> --private-key <key> --rpc-url http://localhost:8545"
        );
    }

    // ----------------------------------------------------------------
    //  Step 1: Register enclave with mock attestation
    // ----------------------------------------------------------------

    function _registerEnclave(uint256 ownerKey, uint256 enclaveKey) internal {
        Vm.Wallet memory enclaveWallet = vm.createWallet(enclaveKey, "enclave");

        if (teeProofVerifier.isRegistered(enclaveWallet.addr)) {
            console2.log("Enclave already registered:", enclaveWallet.addr);
            return;
        }

        bytes memory pubKey =
            abi.encodePacked(bytes1(0x04), bytes32(enclaveWallet.publicKeyX), bytes32(enclaveWallet.publicKeyY));

        TeeProofVerifier.AttestationData memory att = TeeProofVerifier.AttestationData({
            timestampMs: uint64(block.timestamp * 1000),
            pcrHash: keccak256("mock-pcr-hash"),
            publicKey: pubKey,
            userData: ""
        });

        vm.broadcast(ownerKey);
        teeProofVerifier.register("", att);

        require(teeProofVerifier.isRegistered(enclaveWallet.addr), "register failed");
        console2.log("Enclave registered:", enclaveWallet.addr);
    }

    // ----------------------------------------------------------------
    //  Step 2: Create game
    // ----------------------------------------------------------------

    function _createGame(uint256 proposerKey) internal {
        uint256 defenderBond = factory.initBonds(TEE_GAME_TYPE);
        bytes memory extraData = abi.encodePacked(l2SeqNum, type(uint32).max, endBlockHash, endStateHash);
        Claim rootClaim = Claim.wrap(keccak256(abi.encode(endBlockHash, endStateHash)));

        vm.broadcast(proposerKey);
        game =
            TeeDisputeGame(payable(address(factory.create{ value: defenderBond }(TEE_GAME_TYPE, rootClaim, extraData))));

        console2.log("Game created:", address(game));
        console2.log("  l2SequenceNumber:", l2SeqNum);
        console2.log("  rootClaim:", vm.toString(rootClaim.raw()));
        console2.log("  proposer:", game.proposer());
    }

    // ----------------------------------------------------------------
    //  Step 3: Challenge
    // ----------------------------------------------------------------

    function _challenge(uint256 challengerKey) internal {
        uint256 challengerBond = vm.envOr("CHALLENGER_BOND", uint256(0.2 ether));
        vm.broadcast(challengerKey);
        game.challenge{ value: challengerBond }();
        console2.log("Game challenged by:", vm.addr(challengerKey));
    }

    // ----------------------------------------------------------------
    //  Step 4a: Prove with external signature (from TEE prover)
    // ----------------------------------------------------------------

    function _proveExternal(uint256 proposerKey, bytes memory signature) internal {
        require(signature.length == 65, "BATCH_SIGNATURE must be 65 bytes (r+s+v)");

        bytes32 domainSep = game.domainSeparator();
        console2.log("Domain separator:", vm.toString(domainSep));
        console2.log("Using external signature, length:", signature.length);

        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](1);
        proofs[0] = TeeDisputeGame.BatchProof({
            startBlockHash: startBlockHash,
            startStateHash: startStateHash,
            endBlockHash: endBlockHash,
            endStateHash: endStateHash,
            l2Block: l2SeqNum,
            signature: signature
        });

        vm.broadcast(proposerKey);
        game.prove(abi.encode(proofs));

        console2.log("Proof submitted successfully!");
    }

    // ----------------------------------------------------------------
    //  Step 4b: Prove with mock signing (enclave key signs locally)
    // ----------------------------------------------------------------

    function _proveMock(uint256 proposerKey, uint256 enclaveKey) internal {
        bytes32 domainSep = game.domainSeparator();
        console2.log("Domain separator:", vm.toString(domainSep));

        bytes32 digest = _buildBatchDigest(domainSep);
        console2.log("EIP-712 digest:", vm.toString(digest));

        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclaveKey, digest);
        bytes memory signature = abi.encodePacked(r, s, v);
        console2.log("Batch signed, signature length:", signature.length);

        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](1);
        proofs[0] = TeeDisputeGame.BatchProof({
            startBlockHash: startBlockHash,
            startStateHash: startStateHash,
            endBlockHash: endBlockHash,
            endStateHash: endStateHash,
            l2Block: l2SeqNum,
            signature: signature
        });

        vm.broadcast(proposerKey);
        game.prove(abi.encode(proofs));

        console2.log("Proof submitted successfully!");
    }

    function _buildBatchDigest(bytes32 domainSep) internal view returns (bytes32) {
        bytes32 structHash = keccak256(
            abi.encode(BATCH_PROOF_TYPEHASH, startBlockHash, startStateHash, endBlockHash, endStateHash, l2SeqNum)
        );
        return keccak256(abi.encodePacked("\x19\x01", domainSep, structHash));
    }

    // ----------------------------------------------------------------
    //  Step 5: Resolve
    // ----------------------------------------------------------------

    function _resolve(uint256 callerKey) internal {
        vm.broadcast(callerKey);
        GameStatus result = game.resolve();

        if (result == GameStatus.DEFENDER_WINS) {
            console2.log("Game resolved: DEFENDER_WINS");
        } else if (result == GameStatus.CHALLENGER_WINS) {
            console2.log("Game resolved: CHALLENGER_WINS");
        } else {
            console2.log("Game resolved: IN_PROGRESS");
        }
    }
}
