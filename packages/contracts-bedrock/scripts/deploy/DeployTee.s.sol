// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import {Script, console2} from "forge-std/Script.sol";
import {IDisputeGameFactory} from "interfaces/dispute/IDisputeGameFactory.sol";
import {IAnchorStateRegistry} from "interfaces/dispute/IAnchorStateRegistry.sol";
import {Duration} from "src/dispute/lib/Types.sol";
import {IRiscZeroVerifier} from "interfaces/dispute/IRiscZeroVerifier.sol";
import {ITeeProofVerifier} from "interfaces/dispute/ITeeProofVerifier.sol";
import {TeeDisputeGame} from "src/dispute/tee/TeeDisputeGame.sol";
import {TeeProofVerifier} from "src/dispute/tee/TeeProofVerifier.sol";

contract Deploy is Script {
    struct DeployConfig {
        uint256 deployerKey;
        address deployer;
        IRiscZeroVerifier riscZeroVerifier;
        bytes32 imageId;
        bytes nitroRootKey;
        IDisputeGameFactory disputeGameFactory;
        IAnchorStateRegistry anchorStateRegistry;
        uint64 maxChallengeDuration;
        uint64 maxProveDuration;
        uint256 challengerBond;
        address proofVerifierOwner;
        address[] proposers;
        address[] challengers;
    }

    function run() external returns (TeeProofVerifier teeProofVerifier, TeeDisputeGame teeDisputeGame) {
        DeployConfig memory cfg = _readConfig();

        vm.startBroadcast(cfg.deployerKey);

        teeProofVerifier = new TeeProofVerifier(cfg.riscZeroVerifier, cfg.imageId, cfg.nitroRootKey);

        for (uint256 i = 0; i < cfg.proposers.length; i++) {
            teeProofVerifier.addProposer(cfg.proposers[i]);
        }
        for (uint256 i = 0; i < cfg.challengers.length; i++) {
            teeProofVerifier.addChallenger(cfg.challengers[i]);
        }

        if (cfg.proofVerifierOwner != cfg.deployer) {
            teeProofVerifier.transferOwnership(cfg.proofVerifierOwner);
        }

        teeDisputeGame = new TeeDisputeGame(
            Duration.wrap(cfg.maxChallengeDuration),
            Duration.wrap(cfg.maxProveDuration),
            cfg.disputeGameFactory,
            ITeeProofVerifier(address(teeProofVerifier)),
            cfg.challengerBond,
            cfg.anchorStateRegistry
        );

        vm.stopBroadcast();

        console2.log("deployer", cfg.deployer);
        console2.log("teeProofVerifier", address(teeProofVerifier));
        console2.log("teeDisputeGame", address(teeDisputeGame));
    }

    function _readConfig() internal view returns (DeployConfig memory cfg) {
        cfg.deployerKey = vm.envUint("PRIVATE_KEY");
        cfg.deployer = vm.addr(cfg.deployerKey);
        cfg.riscZeroVerifier = IRiscZeroVerifier(vm.envAddress("RISC_ZERO_VERIFIER"));
        cfg.imageId = vm.envBytes32("RISC_ZERO_IMAGE_ID");
        cfg.nitroRootKey = vm.envBytes("NITRO_ROOT_KEY");
        cfg.disputeGameFactory = IDisputeGameFactory(vm.envAddress("DISPUTE_GAME_FACTORY"));
        cfg.anchorStateRegistry = IAnchorStateRegistry(vm.envAddress("ANCHOR_STATE_REGISTRY"));
        cfg.maxChallengeDuration = uint64(vm.envUint("MAX_CHALLENGE_DURATION"));
        cfg.maxProveDuration = uint64(vm.envUint("MAX_PROVE_DURATION"));
        cfg.challengerBond = vm.envUint("CHALLENGER_BOND");
        cfg.proofVerifierOwner = vm.envOr("PROOF_VERIFIER_OWNER", cfg.deployer);
        cfg.proposers = vm.envAddress("PROPOSERS", ",");
        cfg.challengers = vm.envAddress("CHALLENGERS", ",");
    }
}
