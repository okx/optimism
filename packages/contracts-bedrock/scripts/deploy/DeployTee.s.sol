// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import {Script, console2} from "forge-std/Script.sol";
import {IDisputeGameFactory} from "interfaces/dispute/IDisputeGameFactory.sol";
import {IAnchorStateRegistry} from "interfaces/dispute/IAnchorStateRegistry.sol";
import {Duration} from "src/dispute/lib/Types.sol";
import {IRiscZeroVerifier} from "interfaces/dispute/IRiscZeroVerifier.sol";
import {ITeeProofVerifier} from "interfaces/dispute/ITeeProofVerifier.sol";
import {DisputeGameFactoryRouter} from "src/dispute/DisputeGameFactoryRouter.sol";
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
        bool deployRouter;
        address proofVerifierOwner;
        address proposer;
        address challenger;
        uint256[] zoneIds;
        address[] routerFactories;
        address routerOwner;
    }

    function run()
        external
        returns (
            TeeProofVerifier teeProofVerifier,
            TeeDisputeGame teeDisputeGame,
            DisputeGameFactoryRouter router
        )
    {
        DeployConfig memory cfg = _readConfig();

        if (cfg.deployRouter) {
            require(cfg.zoneIds.length == cfg.routerFactories.length, "Deploy: router zone/factory length mismatch");
        }

        vm.startBroadcast(cfg.deployerKey);

        teeProofVerifier = new TeeProofVerifier(cfg.riscZeroVerifier, cfg.imageId, cfg.nitroRootKey);
        if (cfg.proofVerifierOwner != cfg.deployer) {
            teeProofVerifier.transferOwnership(cfg.proofVerifierOwner);
        }

        teeDisputeGame = new TeeDisputeGame(
            Duration.wrap(cfg.maxChallengeDuration),
            Duration.wrap(cfg.maxProveDuration),
            cfg.disputeGameFactory,
            ITeeProofVerifier(address(teeProofVerifier)),
            cfg.challengerBond,
            cfg.anchorStateRegistry,
            cfg.proposer,
            cfg.challenger
        );

        if (cfg.deployRouter) {
            router = _deployRouter(cfg.routerOwner, cfg.deployer, cfg.zoneIds, cfg.routerFactories);
        }

        vm.stopBroadcast();

        console2.log("deployer", cfg.deployer);
        console2.log("teeProofVerifier", address(teeProofVerifier));
        console2.log("teeDisputeGame", address(teeDisputeGame));
        if (cfg.deployRouter) {
            console2.log("router", address(router));
        }
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
        cfg.deployRouter = vm.envOr("DEPLOY_ROUTER", false);
        cfg.proofVerifierOwner = vm.envOr("PROOF_VERIFIER_OWNER", cfg.deployer);
        cfg.proposer = vm.envAddress("PROPOSER");
        cfg.challenger = vm.envAddress("CHALLENGER");
        cfg.zoneIds = _envUintArray("ROUTER_ZONE_IDS");
        cfg.routerFactories = _envAddressArray("ROUTER_FACTORIES");
        cfg.routerOwner = vm.envOr("ROUTER_OWNER", cfg.deployer);
    }

    function _deployRouter(
        address routerOwner,
        address,
        uint256[] memory zoneIds,
        address[] memory routerFactories
    )
        internal
        returns (DisputeGameFactoryRouter router)
    {
        router = new DisputeGameFactoryRouter(routerOwner);
        for (uint256 i = 0; i < zoneIds.length; i++) {
            router.setZone(zoneIds[i], routerFactories[i]);
        }
    }

    function _envAddressArray(string memory name) internal view returns (address[] memory values) {
        if (!vm.envExists(name)) return new address[](0);
        return vm.envAddress(name, ",");
    }

    function _envUintArray(string memory name) internal view returns (uint256[] memory values) {
        if (!vm.envExists(name)) return new uint256[](0);
        return vm.envUint(name, ",");
    }
}
