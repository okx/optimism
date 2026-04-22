// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import { Script, console2 } from "forge-std/Script.sol";
import { Proxy } from "src/universal/Proxy.sol";
import { AnchorStateRegistry } from "src/dispute/AnchorStateRegistry.sol";
import { TeeDisputeGame, TEE_DISPUTE_GAME_TYPE } from "src/dispute/tee/TeeDisputeGame.sol";
import { TeeProofVerifier } from "src/dispute/tee/TeeProofVerifier.sol";
import { AccessManager } from "src/dispute/tee/AccessManager.sol";
import { IDisputeGameFactory } from "interfaces/dispute/IDisputeGameFactory.sol";
import { IAnchorStateRegistry } from "interfaces/dispute/IAnchorStateRegistry.sol";
import { IRiscZeroVerifier } from "interfaces/dispute/IRiscZeroVerifier.sol";
import { ITeeProofVerifier } from "interfaces/dispute/ITeeProofVerifier.sol";
import { IAccessManager } from "interfaces/dispute/zk/IAccessManager.sol";
import { Duration } from "src/dispute/lib/Types.sol";

/// @title DeployTeePhase1
/// @notice Phase 1: Deploy all implementation contracts using the deployer key.
///         Phase 2 (multisig) is required to initialize the ASR proxy and register
///         the game type in DisputeGameFactory.
///
/// @dev Required env vars:
///   PRIVATE_KEY                    Deployer private key (0x-prefixed hex)
///   PROXY_ADMIN                    ProxyAdmin contract address (shared with xlayer)
///   DISPUTE_GAME_FACTORY           DisputeGameFactory proxy address
///   RISC_ZERO_VERIFIER             RiscZeroGroth16Verifier address
///   RISC_ZERO_IMAGE_ID             bytes32 guest image ID (0x-prefixed hex)
///   NITRO_ROOT_KEY                 AWS Nitro P384 root public key, 96 bytes hex (no 0x04 prefix)
///   DISPUTE_GAME_FINALITY_DELAY    AnchorStateRegistry finality delay in seconds
///   MAX_CHALLENGE_DURATION         Challenge window in seconds
///   MAX_PROVE_DURATION             Prove window in seconds
///   CHALLENGER_BOND                Challenger bond amount in wei
///   PROPOSERS                      Comma-separated allowed proposer addresses
///   CHALLENGERS                    Comma-separated allowed challenger addresses
///
/// @dev Optional env vars:
///   PROOF_VERIFIER_OWNER           TeeProofVerifier owner; defaults to deployer address
///
/// @dev Example:
///   source .env.tee
///   forge script scripts/deploy/DeployTeePhase1.s.sol \
///     --rpc-url $RPC_URL --broadcast --verify
contract DeployTeePhase1 is Script {
    struct Config {
        uint256 deployerKey;
        address deployer;
        // AnchorStateRegistry
        address proxyAdmin;
        uint256 disputeGameFinalityDelay;
        // AccessManager
        uint256 fallbackTimeout;
        // TeeProofVerifier
        IRiscZeroVerifier riscZeroVerifier;
        bytes32 imageId;
        bytes nitroRootKey;
        address proofVerifierOwner;
        address[] proposers;
        address[] challengers;
        // TeeDisputeGame
        IDisputeGameFactory disputeGameFactory;
        uint64 maxChallengeDuration;
        uint64 maxProveDuration;
        uint256 challengerBond;
    }

    function run()
        external
        returns (
            AnchorStateRegistry asrImpl,
            Proxy asrProxy,
            TeeProofVerifier teeProofVerifier,
            TeeDisputeGame teeDisputeGame
        )
    {
        Config memory cfg = _readConfig();

        vm.startBroadcast(cfg.deployerKey);

        // 1. Deploy AnchorStateRegistry implementation
        asrImpl = new AnchorStateRegistry(cfg.disputeGameFinalityDelay);

        // 2. Deploy AnchorStateRegistry Proxy (admin = ProxyAdmin, impl uninitialised)
        //    Initialisation is done in Phase 2 via ProxyAdmin.upgradeAndCall().
        asrProxy = new Proxy(cfg.proxyAdmin);

        // 3. Deploy AccessManager and TeeProofVerifier
        AccessManager accessManager = new AccessManager(cfg.fallbackTimeout, cfg.disputeGameFactory);
        for (uint256 i = 0; i < cfg.proposers.length; i++) {
            accessManager.setProposer(cfg.proposers[i], true);
        }
        for (uint256 i = 0; i < cfg.challengers.length; i++) {
            accessManager.setChallenger(cfg.challengers[i], true);
        }

        teeProofVerifier = new TeeProofVerifier(
            cfg.riscZeroVerifier, cfg.imageId, cfg.nitroRootKey, IAccessManager(address(accessManager))
        );

        // Transfer ownership before deploying TeeDisputeGame so the owner is set correctly
        // from the moment TeeProofVerifier is live.
        if (cfg.proofVerifierOwner != cfg.deployer) {
            teeProofVerifier.transferOwnership(cfg.proofVerifierOwner);
            accessManager.transferOwnership(cfg.proofVerifierOwner);
        }

        // 4. Deploy TeeDisputeGame implementation.
        //    ANCHOR_STATE_REGISTRY points to the proxy deployed above; it will be
        //    functional after Phase 2 initialises the proxy.
        teeDisputeGame = new TeeDisputeGame(
            Duration.wrap(cfg.maxChallengeDuration),
            Duration.wrap(cfg.maxProveDuration),
            cfg.disputeGameFactory,
            ITeeProofVerifier(address(teeProofVerifier)),
            cfg.challengerBond,
            IAnchorStateRegistry(address(asrProxy))
        );

        vm.stopBroadcast();

        _log(cfg, asrImpl, asrProxy, teeProofVerifier, teeDisputeGame);
    }

    function _readConfig() internal view returns (Config memory cfg) {
        cfg.deployerKey = vm.envUint("PRIVATE_KEY");
        cfg.deployer = vm.addr(cfg.deployerKey);

        cfg.proxyAdmin = vm.envAddress("PROXY_ADMIN");
        cfg.disputeGameFinalityDelay = vm.envUint("DISPUTE_GAME_FINALITY_DELAY");

        cfg.riscZeroVerifier = IRiscZeroVerifier(vm.envAddress("RISC_ZERO_VERIFIER"));
        cfg.imageId = vm.envBytes32("RISC_ZERO_IMAGE_ID");
        cfg.nitroRootKey = vm.envBytes("NITRO_ROOT_KEY");
        cfg.fallbackTimeout = vm.envOr("FALLBACK_TIMEOUT", uint256(7 days));
        cfg.proofVerifierOwner = vm.envOr("PROOF_VERIFIER_OWNER", cfg.deployer);
        cfg.proposers = vm.envAddress("PROPOSERS", ",");
        cfg.challengers = vm.envAddress("CHALLENGERS", ",");

        cfg.disputeGameFactory = IDisputeGameFactory(vm.envAddress("DISPUTE_GAME_FACTORY"));
        cfg.maxChallengeDuration = uint64(vm.envUint("MAX_CHALLENGE_DURATION"));
        cfg.maxProveDuration = uint64(vm.envUint("MAX_PROVE_DURATION"));
        cfg.challengerBond = vm.envUint("CHALLENGER_BOND");
    }

    function _log(
        Config memory cfg,
        AnchorStateRegistry asrImpl,
        Proxy asrProxy,
        TeeProofVerifier teeProofVerifier,
        TeeDisputeGame teeDisputeGame
    )
        internal
        view
    {
        console2.log("=== Phase 1 Deployed Addresses ===");
        console2.log("deployer                  :", cfg.deployer);
        console2.log("");
        console2.log("AnchorStateRegistry impl  :", address(asrImpl));
        console2.log("AnchorStateRegistry proxy :", address(asrProxy));
        console2.log("  proxy admin             :", cfg.proxyAdmin);
        console2.log("  (proxy NOT yet initialised - requires Phase 2 multisig)");
        console2.log("");
        console2.log("TeeProofVerifier          :", address(teeProofVerifier));
        console2.log("  owner                   :", cfg.proofVerifierOwner);
        console2.log("");
        console2.log("TeeDisputeGame impl       :", address(teeDisputeGame));
        console2.log("  gameType                :", TEE_DISPUTE_GAME_TYPE);
        console2.log("  maxChallengeDuration    :", cfg.maxChallengeDuration, "s");
        console2.log("  maxProveDuration        :", cfg.maxProveDuration, "s");
        console2.log("  challengerBond          :", cfg.challengerBond, "wei");
        console2.log("");
        console2.log("=== Phase 2 Multisig Actions Required ===");
        console2.log("1. ProxyAdmin.upgradeAndCall(asrProxy, asrImpl, initialize(...))");
        console2.log("2. DisputeGameFactory.setImplementation(1960, teeDisputeGame)");
        console2.log("3. DisputeGameFactory.setInitBond(1960, <initBond>)");
    }
}
