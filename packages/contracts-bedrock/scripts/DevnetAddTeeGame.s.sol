// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import {Script, console2} from "forge-std/Script.sol";
import {IDisputeGameFactory} from "interfaces/dispute/IDisputeGameFactory.sol";
import {IAnchorStateRegistry} from "interfaces/dispute/IAnchorStateRegistry.sol";
import {ISystemConfig} from "interfaces/L1/ISystemConfig.sol";
import {GameType, Duration, Hash, Proposal} from "src/dispute/lib/Types.sol";
import {AnchorStateRegistry} from "src/dispute/AnchorStateRegistry.sol";
import {IRiscZeroVerifier} from "interfaces/dispute/IRiscZeroVerifier.sol";
import {ITeeProofVerifier} from "interfaces/dispute/ITeeProofVerifier.sol";
import {IAccessManager} from "interfaces/dispute/zk/IAccessManager.sol";
import {TeeDisputeGame} from "src/dispute/tee/TeeDisputeGame.sol";
import {TeeProofVerifier} from "src/dispute/tee/TeeProofVerifier.sol";
import {AccessManager} from "src/dispute/tee/AccessManager.sol";
import {MockRiscZeroVerifier} from "test/dispute/tee/mocks/MockRiscZeroVerifier.sol";
import {MockTeeProofVerifier} from "test/dispute/tee/mocks/MockTeeProofVerifier.sol";
import {Proxy} from "src/universal/Proxy.sol";
import {ProxyAdmin} from "src/universal/ProxyAdmin.sol";

/// @notice Deploys TeeDisputeGame and a new AnchorStateRegistry onto an existing devnet,
///         wiring the new ASR to the **existing** DisputeGameFactory (so isGameProper() works
///         for games created by that factory).
///
///         Intentionally does NOT call setImplementation / setInitBond on the existing DGF
///         (those require TRANSACTOR or Safe, handled by the bash wrapper).
///         setRespectedGameType(1960) is called during initialize() to avoid a
///         partial-failure window where the ASR exists with game type 0.
///
/// Required env vars (set by add-tee-game-type.sh, which sources devnet/.env):
///   PRIVATE_KEY                          deployer private key
///   EXISTING_DGF                         existing devnet DGF address
///   EXISTING_ASR                         existing AnchorStateRegistry (source for starting anchor)
///   SYSTEM_CONFIG_ADDRESS                SystemConfig proxy address
///   DISPUTE_GAME_FINALITY_DELAY_SECONDS  finality delay for new ASR
///   MAX_CHALLENGE_DURATION               seconds
///   MAX_PROVE_DURATION                   seconds
///   CHALLENGER_BOND                      wei
///   INIT_BOND                            wei (informational — not set here)
///
/// Optional:
///   PROPOSER_ADDRESS          address to whitelist as proposer (defaults to deployer)
///   CHALLENGER_ADDRESS        address to whitelist as challenger (defaults to address(0) = permissionless)
///
/// Optional (mock mode):
///   USE_MOCK_VERIFIER         if "true", deploy MockTeeProofVerifier instead of
///                             MockRiscZeroVerifier + TeeProofVerifier. Useful when
///                             you want a fully mock verifier (no ECDSA needed).
contract DevnetAddTeeGame is Script {
    uint32 internal constant TEE_GAME_TYPE = 1960;

    struct Config {
        uint256 deployerKey;
        address deployer;
        address existingDgf;
        address anchorStateRegistry;
        address systemConfig;
        uint256 disputeGameFinalityDelaySeconds;
        uint64 maxChallengeDuration;
        uint64 maxProveDuration;
        uint256 challengerBond;
        address proposer;
        address challenger;
        bool useMockVerifier;
    }

    function run() external {
        Config memory cfg = _readConfig();

        vm.startBroadcast(cfg.deployerKey);

        // 1. Select verifier:
        //    --mock-verifier flag → MockTeeProofVerifier (no ECDSA needed, fully open)
        //    default             → MockRiscZeroVerifier + TeeProofVerifier (accepts any RZ proof)
        address verifier;
        if (cfg.useMockVerifier) {
            verifier = address(new MockTeeProofVerifier());
            console2.log("Verifier mode: MockTeeProofVerifier (mock)");
        } else {
            address mockRisc = address(new MockRiscZeroVerifier());
            AccessManager accessManager = new AccessManager(7 days, IDisputeGameFactory(cfg.existingDgf));
            if (cfg.proposer != address(0)) accessManager.setProposer(cfg.proposer, true);
            accessManager.setChallenger(cfg.challenger, true);
            verifier = address(new TeeProofVerifier(
                IRiscZeroVerifier(mockRisc),
                bytes32(0), // dummy imageId for devnet
                bytes(""),  // dummy nitroRootKey for devnet
                IAccessManager(address(accessManager))
            ));
            console2.log("Verifier mode: TeeProofVerifier + MockRiscZeroVerifier");
        }

        // 2. Deploy a fresh ProxyAdmin owned by the deployer for TEE-specific proxies.
        //    The devnet's existing ProxyAdmin is owned by the TRANSACTOR contract,
        //    not the deployer EOA, so we can't call upgrade() through it directly.
        //    With our own ProxyAdmin: proxyAdminOwner() == deployer == msg.sender, so
        //    ProxyAdminOwnedBase._assertOnlyProxyAdminOrProxyAdminOwner() passes in initialize().
        address teeProxyAdmin = address(new ProxyAdmin(cfg.deployer));

        // 3. New AnchorStateRegistry (impl + Proxy), pointing at the EXISTING DGF.
        //    This lets isGameProper() recognise games created by the existing factory.
        address tzAsr = _deployAsr(cfg, teeProxyAdmin);

        // 4. TeeDisputeGame impl (no proxy — it's the impl)
        address teeDisputeGame = _deployTeeStack(cfg, verifier, tzAsr);

        vm.stopBroadcast();

        _printResults(tzAsr, teeDisputeGame);
    }

    // ── helpers ──────────────────────────────────────────────────────────────

    function _readConfig() internal view returns (Config memory cfg) {
        cfg.deployerKey = vm.envUint("PRIVATE_KEY");
        cfg.deployer = vm.addr(cfg.deployerKey);
        cfg.existingDgf = vm.envAddress("EXISTING_DGF");
        cfg.systemConfig = vm.envAddress("SYSTEM_CONFIG_ADDRESS");
        cfg.disputeGameFinalityDelaySeconds = vm.envUint("DISPUTE_GAME_FINALITY_DELAY_SECONDS");
        cfg.maxChallengeDuration = uint64(vm.envUint("MAX_CHALLENGE_DURATION"));
        cfg.maxProveDuration = uint64(vm.envUint("MAX_PROVE_DURATION"));
        cfg.challengerBond = vm.envUint("CHALLENGER_BOND");
        cfg.proposer = vm.envOr("PROPOSER_ADDRESS", cfg.deployer);
        cfg.challenger = vm.envOr("CHALLENGER_ADDRESS", address(0));
        cfg.useMockVerifier = vm.envOr("USE_MOCK_VERIFIER", false);
    }

    function _deployAsr(Config memory cfg, address proxyAdmin) internal returns (address) {
        // Deploy impl
        AnchorStateRegistry asrImpl = new AnchorStateRegistry(cfg.disputeGameFinalityDelaySeconds);

        // Deploy proxy backed by our fresh ProxyAdmin
        Proxy asrProxy = new Proxy(payable(proxyAdmin));
        ProxyAdmin(payable(proxyAdmin)).upgrade(payable(address(asrProxy)), address(asrImpl));

        // Copy starting anchor root from existing ASR (game type 0 = CannonFaultDisputeGame)
        Proposal memory startingAnchorRoot = Proposal({root: Hash.wrap(bytes32(0)), l2SequenceNumber: 0});

        // Initialize pointing at the EXISTING DGF with respectedGameType=1960 from the start,
        // avoiding a partial-failure window where the ASR exists but has game type 0.
        AnchorStateRegistry(address(asrProxy)).initialize(
            ISystemConfig(cfg.systemConfig),
            IDisputeGameFactory(cfg.existingDgf),
            startingAnchorRoot,
            GameType.wrap(TEE_GAME_TYPE)
        );

        return address(asrProxy);
    }

    function _deployTeeStack(Config memory cfg, address verifier, address tzAsr)
        internal
        returns (address teeDisputeGame)
    {
        teeDisputeGame = address(
            new TeeDisputeGame(
                Duration.wrap(cfg.maxChallengeDuration),
                Duration.wrap(cfg.maxProveDuration),
                IDisputeGameFactory(cfg.existingDgf),
                ITeeProofVerifier(verifier),
                cfg.challengerBond,
                IAnchorStateRegistry(tzAsr)
            )
        );
    }

    function _printResults(address tzAsr, address teeDisputeGame) internal pure {
        console2.log("");
        console2.log("=== TEE Game Deployment Results ===");
        console2.log("TeeDisputeGame impl:    ", teeDisputeGame);
        console2.log("New AnchorStateRegistry:", tzAsr);
        console2.log("");
        console2.log("Next steps (handled by bash wrapper):");
        console2.log("  1. setRespectedGameType(1960) on new ASR (deployer is guardian)");
        console2.log("  2. setImplementation(1960, impl) on existing DGF (via TRANSACTOR/Safe)");
        console2.log("  3. setInitBond(1960, bond) on existing DGF (via TRANSACTOR/Safe)");
    }
}
