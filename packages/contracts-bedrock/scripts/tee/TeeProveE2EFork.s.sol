// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import { Script, console2 } from "forge-std/Script.sol";
import { IDisputeGameFactory } from "interfaces/dispute/IDisputeGameFactory.sol";
import { TeeDisputeGame, TEE_DISPUTE_GAME_TYPE } from "src/dispute/tee/TeeDisputeGame.sol";
import { TeeProofVerifier } from "src/dispute/tee/TeeProofVerifier.sol";
import { Claim, GameType, GameStatus } from "src/dispute/lib/Types.sol";

/// @title TeeProveE2EFork
/// @notice E2E on forked mainnet: register enclave with real ZK proof, then
///         create game, challenge, prove (external signature), resolve.
///
/// @dev Usage:
///   # 1. Deploy first:
///   forge script scripts/tee/DeployTeeFork.s.sol --rpc-url http://localhost:8545 --broadcast
///
///   # 2. Run E2E:
///   PRIVATE_KEY=<owner>             \
///   PROPOSER_KEY=<proposer>         \
///   CHALLENGER_KEY=<challenger>     \
///   TEE_PROOF_VERIFIER=<addr>       \
///   DISPUTE_GAME_FACTORY=<addr>     \
///   SEAL=0x<groth16 seal hex>       \
///   ATT_TIMESTAMP_MS=<uint64>       \
///   ATT_PCR_HASH=0x<bytes32>        \
///   ATT_PUBLIC_KEY=0x<65 bytes>     \
///   ATT_USER_DATA=0x                \
///   BATCH_SIGNATURE=0x<65 bytes r+s+v> \
///   END_BLOCK_HASH=0x...            \
///   END_STATE_HASH=0x...            \
///   L2_SEQUENCE_NUMBER=100          \
///   forge script scripts/tee/TeeProveE2EFork.s.sol --rpc-url http://localhost:8545 --broadcast
contract TeeProveE2EFork is Script {
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

        teeProofVerifier = TeeProofVerifier(vm.envAddress("TEE_PROOF_VERIFIER"));
        factory = IDisputeGameFactory(vm.envAddress("DISPUTE_GAME_FACTORY"));

        startBlockHash = vm.envOr("START_BLOCK_HASH", keccak256("genesis-block"));
        startStateHash = vm.envOr("START_STATE_HASH", keccak256("genesis-state"));
        endBlockHash = vm.envOr("END_BLOCK_HASH", keccak256("end-block-100"));
        endStateHash = vm.envOr("END_STATE_HASH", keccak256("end-state-100"));
        l2SeqNum = vm.envOr("L2_SEQUENCE_NUMBER", uint256(100));


        // ---- Step 1: Register enclave with real ZK proof ----
        console2.log("=== Step 1: Register Enclave (real ZK proof) ===");
        bytes memory registerCallData = vm.envOr("REGISTER_CALLDATA", bytes(""));
        if (registerCallData.length > 0) {
            _registerEnclaveWithCallData(ownerKey, registerCallData);
        } else {
            _registerEnclave(ownerKey);
        }

        // ---- Step 2: Create game ----
        console2.log("");
        console2.log("=== Step 2: Create Game ===");
        _createGame(proposerKey);

        // ---- Step 3: Challenge ----
        console2.log("");
        console2.log("=== Step 3: Challenge ===");
        _challenge(challengerKey);

        // ---- Step 4: Prove with external signature ----
        console2.log("");
        console2.log("=== Step 4: Prove (external signature) ===");
        bytes memory proofBytes = vm.envBytes("PROOF_BYTES");
        if (proofBytes.length > 0) {
            _proveWithCallData(proposerKey, proofBytes);
        } else {
            _prove(proposerKey, vm.envBytes("BATCH_SIGNATURE"));
        }

        // ---- Step 5: Resolve ----
        console2.log("");
        console2.log("=== Step 5: Resolve ===");
        _resolve(proposerKey);

        console2.log("");
        console2.log("=== E2E Complete (fork mode) ===");
    }

    // ----------------------------------------------------------------
    //  Step 1: Register enclave with real seal from Boundless
    // ----------------------------------------------------------------

    function _registerEnclave(uint256 ownerKey) internal {
        bytes memory seal = vm.envBytes("SEAL");
        uint64 timestampMs = uint64(vm.envUint("ATT_TIMESTAMP_MS"));
        bytes32 pcrHash = vm.envBytes32("ATT_PCR_HASH");
        bytes memory publicKey = vm.envBytes("ATT_PUBLIC_KEY");
        bytes memory userData = vm.envOr("ATT_USER_DATA", bytes(""));

        require(publicKey.length == 65, "ATT_PUBLIC_KEY must be 65 bytes");

        // Derive enclave address from public key
        address enclaveAddr = _pubKeyToAddr(publicKey);

        if (teeProofVerifier.isRegistered(enclaveAddr)) {
            console2.log("Already registered:", enclaveAddr);
            return;
        }

        TeeProofVerifier.AttestationData memory att = TeeProofVerifier.AttestationData({
            timestampMs: timestampMs,
            pcrHash: pcrHash,
            publicKey: publicKey,
            userData: userData
        });

        console2.log("Registering with real seal...");
        console2.log("  seal length:", seal.length);
        console2.log("  enclave address:", enclaveAddr);

        vm.broadcast(ownerKey);
        teeProofVerifier.register(seal, att);

        require(teeProofVerifier.isRegistered(enclaveAddr), "register failed");
        console2.log("Enclave registered:", enclaveAddr);
    }

    // ----------------------------------------------------------------
    //  Step 1 (alt): Register enclave with raw calldata
    // ----------------------------------------------------------------

    function _registerEnclaveWithCallData(uint256 ownerKey, bytes memory callData) internal {

        console2.log("Registering with raw calldata...");
        console2.log("  calldata length:", callData.length);

        vm.broadcast(ownerKey);
        (bool success,) = address(teeProofVerifier).call(callData);
        require(success, "register call failed");

        console2.log("Register call succeeded");
    }

    // ----------------------------------------------------------------
    //  Step 2: Create game
    // ----------------------------------------------------------------

    function _createGame(uint256 proposerKey) internal {
        uint256 bond = factory.initBonds(TEE_GAME_TYPE);
        bytes memory extra = abi.encodePacked(l2SeqNum, type(uint32).max, endBlockHash, endStateHash);
        Claim root = Claim.wrap(keccak256(abi.encode(endBlockHash, endStateHash)));

        vm.broadcast(proposerKey);
        game = TeeDisputeGame(payable(address(factory.create{ value: bond }(TEE_GAME_TYPE, root, extra))));

        console2.log("Game created:", address(game));
        console2.log("  l2SequenceNumber:", l2SeqNum);
        console2.log("  domainSeparator:", vm.toString(game.domainSeparator()));
    }

    // ----------------------------------------------------------------
    //  Step 3: Challenge
    // ----------------------------------------------------------------

    function _challenge(uint256 challengerKey) internal {
        uint256 bond = vm.envOr("CHALLENGER_BOND", uint256(0.2 ether));
        vm.broadcast(challengerKey);
        game.challenge{ value: bond }();
        console2.log("Challenged by:", vm.addr(challengerKey));
    }

    // ----------------------------------------------------------------
    //  Step 4: Prove with external signature from TEE prover
    // ----------------------------------------------------------------

    function _prove(uint256 proposerKey, bytes memory signature) internal {
        require(signature.length == 65, "BATCH_SIGNATURE must be 65 bytes");
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

        console2.log("Proof submitted!");
    }

    // ----------------------------------------------------------------
    //  Step 4 (alt): Prove with raw calldata
    // ----------------------------------------------------------------

    function _proveWithCallData(uint256 proposerKey, bytes memory proofBytes) internal {
        vm.broadcast(proposerKey);
        game.prove(proofBytes);

        console2.log("Proof submitted!");
    }

    // ----------------------------------------------------------------
    //  Step 5: Resolve
    // ----------------------------------------------------------------

    function _resolve(uint256 callerKey) internal {
        vm.broadcast(callerKey);
        GameStatus result = game.resolve();
        if (result == GameStatus.DEFENDER_WINS) console2.log("DEFENDER_WINS");
        else if (result == GameStatus.CHALLENGER_WINS) console2.log("CHALLENGER_WINS");
        else console2.log("IN_PROGRESS");
    }

    // ----------------------------------------------------------------
    //  Helpers
    // ----------------------------------------------------------------

    function _pubKeyToAddr(bytes memory publicKey) internal pure returns (address) {
        bytes memory coords = new bytes(64);
        for (uint256 i = 0; i < 64; i++) {
            coords[i] = publicKey[i + 1];
        }
        return address(uint160(uint256(keccak256(coords))));
    }
}
