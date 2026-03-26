// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import { Script, console2 } from "forge-std/Script.sol";
import { Vm } from "forge-std/Vm.sol";
import { IDisputeGameFactory } from "interfaces/dispute/IDisputeGameFactory.sol";
import { TeeDisputeGame, TEE_DISPUTE_GAME_TYPE } from "src/dispute/tee/TeeDisputeGame.sol";
import { TeeProofVerifier } from "src/dispute/tee/TeeProofVerifier.sol";
import { Claim, GameType, GameStatus } from "src/dispute/lib/Types.sol";

/// @title TeeProveE2E
/// @notice Mock E2E: register enclave (empty seal), create game, challenge, prove, resolve.
///         All signing done locally with ENCLAVE_KEY. For real ZK proof testing, use TeeProveE2EFork.
///
/// @dev Usage:
///   PRIVATE_KEY=0xac09...ff80 PROPOSER_KEY=0x59c6...690d CHALLENGER_KEY=0x5de4...365a \
///   ENCLAVE_KEY=0x7c85...07a6 TEE_PROOF_VERIFIER=<addr> DISPUTE_GAME_FACTORY=<addr> \
///   forge script scripts/tee/TeeProveE2E.s.sol --rpc-url http://localhost:8545 --broadcast
contract TeeProveE2E is Script {
    bytes32 private constant BATCH_PROOF_TYPEHASH = keccak256(
        "BatchProof(bytes32 startBlockHash,bytes32 startStateHash,bytes32 endBlockHash,bytes32 endStateHash,uint256 l2Block)"
    );

    GameType internal constant TEE_GAME_TYPE = GameType.wrap(TEE_DISPUTE_GAME_TYPE);

    TeeProofVerifier internal teeProofVerifier;
    IDisputeGameFactory internal factory;
    TeeDisputeGame internal game;

    bytes32 internal startBlockHash;
    bytes32 internal startStateHash;
    bytes32 internal endBlockHash;
    bytes32 internal endStateHash;
    uint256 internal l2SeqNum;

    function run() external {
        uint256 ownerKey = vm.envUint("PRIVATE_KEY");
        uint256 proposerKey = vm.envUint("PROPOSER_KEY");
        uint256 challengerKey = vm.envUint("CHALLENGER_KEY");
        uint256 enclaveKey = vm.envUint("ENCLAVE_KEY");

        teeProofVerifier = TeeProofVerifier(vm.envAddress("TEE_PROOF_VERIFIER"));
        factory = IDisputeGameFactory(vm.envAddress("DISPUTE_GAME_FACTORY"));

        startBlockHash = vm.envOr("START_BLOCK_HASH", keccak256("genesis-block"));
        startStateHash = vm.envOr("START_STATE_HASH", keccak256("genesis-state"));
        endBlockHash = vm.envOr("END_BLOCK_HASH", keccak256("end-block-100"));
        endStateHash = vm.envOr("END_STATE_HASH", keccak256("end-state-100"));
        l2SeqNum = vm.envOr("L2_SEQUENCE_NUMBER", uint256(100));

        console2.log("=== Step 1: Register Enclave (mock) ===");
        _registerEnclave(ownerKey, enclaveKey);

        console2.log("");
        console2.log("=== Step 2: Create Game ===");
        _createGame(proposerKey);

        console2.log("");
        console2.log("=== Step 3: Challenge ===");
        _challenge(challengerKey);

        console2.log("");
        console2.log("=== Step 4: Prove (enclave key signs locally) ===");
        _prove(proposerKey, enclaveKey);

        console2.log("");
        console2.log("=== Step 5: Resolve ===");
        _resolve(proposerKey);

        console2.log("");
        console2.log("=== E2E Complete ===");
    }

    function _registerEnclave(uint256 ownerKey, uint256 enclaveKey) internal {
        Vm.Wallet memory w = vm.createWallet(enclaveKey, "enclave");
        if (teeProofVerifier.isRegistered(w.addr)) {
            console2.log("Already registered:", w.addr);
            return;
        }
        bytes memory pubKey = abi.encodePacked(bytes1(0x04), bytes32(w.publicKeyX), bytes32(w.publicKeyY));
        TeeProofVerifier.AttestationData memory att = TeeProofVerifier.AttestationData({
            timestampMs: uint64(block.timestamp * 1000),
            pcrHash: keccak256("mock-pcr-hash"),
            publicKey: pubKey,
            userData: ""
        });
        vm.broadcast(ownerKey);
        teeProofVerifier.register("", att);
        console2.log("Enclave registered:", w.addr);
    }

    function _createGame(uint256 proposerKey) internal {
        uint256 bond = factory.initBonds(TEE_GAME_TYPE);
        bytes memory extra = abi.encodePacked(l2SeqNum, type(uint32).max, endBlockHash, endStateHash);
        Claim root = Claim.wrap(keccak256(abi.encode(endBlockHash, endStateHash)));
        vm.broadcast(proposerKey);
        game = TeeDisputeGame(payable(address(factory.create{ value: bond }(TEE_GAME_TYPE, root, extra))));
        console2.log("Game created:", address(game));
        console2.log("  l2SequenceNumber:", l2SeqNum);
    }

    function _challenge(uint256 challengerKey) internal {
        uint256 bond = vm.envOr("CHALLENGER_BOND", uint256(0.2 ether));
        vm.broadcast(challengerKey);
        game.challenge{ value: bond }();
        console2.log("Challenged by:", vm.addr(challengerKey));
    }

    function _prove(uint256 proposerKey, uint256 enclaveKey) internal {
        bytes32 domainSep = game.domainSeparator();
        bytes32 structHash = keccak256(
            abi.encode(BATCH_PROOF_TYPEHASH, startBlockHash, startStateHash, endBlockHash, endStateHash, l2SeqNum)
        );
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", domainSep, structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(enclaveKey, digest);

        TeeDisputeGame.BatchProof[] memory proofs = new TeeDisputeGame.BatchProof[](1);
        proofs[0] = TeeDisputeGame.BatchProof({
            startBlockHash: startBlockHash,
            startStateHash: startStateHash,
            endBlockHash: endBlockHash,
            endStateHash: endStateHash,
            l2Block: l2SeqNum,
            signature: abi.encodePacked(r, s, v)
        });
        vm.broadcast(proposerKey);
        game.prove(abi.encode(proofs));
        console2.log("Proof submitted!");
    }

    function _resolve(uint256 callerKey) internal {
        vm.broadcast(callerKey);
        GameStatus result = game.resolve();
        if (result == GameStatus.DEFENDER_WINS) console2.log("DEFENDER_WINS");
        else if (result == GameStatus.CHALLENGER_WINS) console2.log("CHALLENGER_WINS");
        else console2.log("IN_PROGRESS");
    }
}
