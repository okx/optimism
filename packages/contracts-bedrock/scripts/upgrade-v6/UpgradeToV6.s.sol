// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import { Script } from "forge-std/Script.sol";
import { console } from "forge-std/console.sol";

import { DisputeGameFactory } from "src/dispute/DisputeGameFactory.sol";
import { AnchorStateRegistry } from "src/dispute/AnchorStateRegistry.sol";
import { FaultDisputeGameV2 } from "src/dispute/v2/FaultDisputeGameV2.sol";
import { PermissionedDisputeGameV2 } from "src/dispute/v2/PermissionedDisputeGameV2.sol";
import { IDisputeGame } from "interfaces/dispute/IDisputeGame.sol";
import { IDisputeGameFactory } from "interfaces/dispute/IDisputeGameFactory.sol";
import { IFaultDisputeGame } from "interfaces/dispute/IFaultDisputeGame.sol";
import { IProxyAdmin } from "interfaces/universal/IProxyAdmin.sol";
import { ISystemConfig } from "interfaces/L1/ISystemConfig.sol";
import { IAnchorStateRegistry } from "interfaces/dispute/IAnchorStateRegistry.sol";
import { IOptimismPortal2 } from "interfaces/L1/IOptimismPortal2.sol";
import { Duration, GameType, GameTypes, Claim } from "src/dispute/lib/Types.sol";
import { LibGameArgs } from "src/dispute/lib/LibGameArgs.sol";

/// @title UpgradeToV6
/// @notice Upgrades XLayer L1 contracts from v5 to v6 (op-contracts/v6.0.0-rc.2)
/// @dev Steps:
///      1. Deploy new DGF impl (1.4.0) and ASR impl (3.7.0)
///      2. Deploy FaultDisputeGameV2 and PermissionedDisputeGameV2
///      3. Upgrade DGF and ASR proxies via ProxyAdmin
///      4. Register CANNON_KONA (type 8) in DGF with FDGv2 impl + gameArgs
///      5. Does NOT change respectedGameType — existing PermissionedDisputeGame (type 1) stays active
///
///      Required env vars:
///        SYSTEM_CONFIG_PROXY_ADDRESS
///        DISPUTE_GAME_FINALITY_DELAY_SECONDS  (mainnet: 302400)
///        FAULT_GAME_MAX_DEPTH                 (mainnet: 73)
///        FAULT_GAME_SPLIT_DEPTH               (mainnet: 30)
///        FAULT_GAME_CLOCK_EXTENSION           (mainnet: 10800)
///        FAULT_GAME_MAX_CLOCK_DURATION        (mainnet: 302400)
///        CANNON_KONA_ABSOLUTE_PRESTATE        (bytes32, kona prestate hash)
///        CANNON_KONA_INIT_BOND                (wei, e.g. 80000000000000000 = 0.08 ETH)
contract UpgradeToV6 is Script {
    // Deployed implementations
    DisputeGameFactory public newDGFImpl;
    AnchorStateRegistry public newASRImpl;
    FaultDisputeGameV2 public fdgV2Impl;
    PermissionedDisputeGameV2 public pdgV2Impl;

    function run() external {
        // --- Load config ---
        address systemConfigProxy = vm.envAddress("SYSTEM_CONFIG_PROXY_ADDRESS");
        uint256 disputeGameFinalityDelay = vm.envUint("DISPUTE_GAME_FINALITY_DELAY_SECONDS");
        uint256 maxGameDepth = vm.envUint("FAULT_GAME_MAX_DEPTH");
        uint256 splitDepth = vm.envUint("FAULT_GAME_SPLIT_DEPTH");
        uint64 clockExtension = uint64(vm.envUint("FAULT_GAME_CLOCK_EXTENSION"));
        uint64 maxClockDuration = uint64(vm.envUint("FAULT_GAME_MAX_CLOCK_DURATION"));
        bytes32 cannonKonaPrestate = vm.envBytes32("CANNON_KONA_ABSOLUTE_PRESTATE");
        uint256 cannonKonaInitBond = vm.envUint("CANNON_KONA_INIT_BOND");

        // --- Derive addresses from SystemConfig ---
        ISystemConfig sysConfig = ISystemConfig(systemConfigProxy);
        address optimismPortal = sysConfig.optimismPortal();
        address dgfProxy = sysConfig.disputeGameFactory();
        address asrProxy = address(IOptimismPortal2(payable(optimismPortal)).anchorStateRegistry());

        // --- Get ProxyAdmin from EIP-1967 admin slot ---
        bytes32 adminSlot = 0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103;
        address proxyAdmin = address(uint160(uint256(vm.load(dgfProxy, adminSlot))));

        // --- Read existing game params for CANNON_KONA registration ---
        IFaultDisputeGame existingPDG = IFaultDisputeGame(
            address(IDisputeGameFactory(dgfProxy).gameImpls(GameTypes.PERMISSIONED_CANNON))
        );
        address vmAddr = address(existingPDG.vm());
        address wethAddr = address(existingPDG.weth());
        uint256 l2ChainId = sysConfig.l2ChainId();

        _logConfig(systemConfigProxy, optimismPortal, dgfProxy, asrProxy, proxyAdmin, vmAddr, wethAddr, l2ChainId);

        // --- Pre-upgrade versions ---
        console.log("\n=== Pre-upgrade versions ===");
        console.log("DGF:", DisputeGameFactory(dgfProxy).version());
        console.log("ASR:", AnchorStateRegistry(asrProxy).version());

        // --- Deploy and upgrade ---
        vm.startBroadcast();

        // Step 1: Deploy new implementations
        newDGFImpl = new DisputeGameFactory();
        newASRImpl = new AnchorStateRegistry(disputeGameFinalityDelay);

        FaultDisputeGameV2.GameConstructorParams memory gameParams = FaultDisputeGameV2.GameConstructorParams({
            maxGameDepth: maxGameDepth,
            splitDepth: splitDepth,
            clockExtension: Duration.wrap(clockExtension),
            maxClockDuration: Duration.wrap(maxClockDuration)
        });
        fdgV2Impl = new FaultDisputeGameV2(gameParams);
        pdgV2Impl = new PermissionedDisputeGameV2(gameParams);

        console.log("\n=== Deployed implementations ===");
        console.log("DGF impl:", address(newDGFImpl), newDGFImpl.version());
        console.log("ASR impl:", address(newASRImpl), newASRImpl.version());
        console.log("FDGv2 impl:", address(fdgV2Impl), fdgV2Impl.version());
        console.log("PDGv2 impl:", address(pdgV2Impl), pdgV2Impl.version());

        // Step 2: Upgrade DGF and ASR proxies
        IProxyAdmin(proxyAdmin).upgrade(payable(dgfProxy), address(newDGFImpl));
        console.log("\nUpgraded DGF proxy");

        IProxyAdmin(proxyAdmin).upgrade(payable(asrProxy), address(newASRImpl));
        console.log("Upgraded ASR proxy");

        // Step 3: Register CANNON_KONA (type 8) in DGF
        // Encode gameArgs for permissionless V2 game (no proposer/challenger)
        bytes memory cannonKonaArgs = LibGameArgs.encode(
            LibGameArgs.GameArgs({
                absolutePrestate: cannonKonaPrestate,
                vm: vmAddr,
                anchorStateRegistry: asrProxy,
                weth: wethAddr,
                l2ChainId: l2ChainId,
                proposer: address(0),
                challenger: address(0)
            })
        );

        DisputeGameFactory(dgfProxy).setImplementation(
            GameTypes.CANNON_KONA,
            IDisputeGame(address(fdgV2Impl)),
            cannonKonaArgs
        );
        console.log("Registered CANNON_KONA (type 8) with FDGv2 impl");

        DisputeGameFactory(dgfProxy).setInitBond(GameTypes.CANNON_KONA, cannonKonaInitBond);
        console.log("Set CANNON_KONA init bond:", cannonKonaInitBond);

        vm.stopBroadcast();

        // --- Post-upgrade verification ---
        _verify(dgfProxy, asrProxy);
    }

    function _logConfig(
        address systemConfigProxy,
        address optimismPortal,
        address dgfProxy,
        address asrProxy,
        address proxyAdmin,
        address vmAddr,
        address wethAddr,
        uint256 l2ChainId
    ) internal pure {
        console.log("=== Configuration ===");
        console.log("SystemConfig:", systemConfigProxy);
        console.log("OptimismPortal:", optimismPortal);
        console.log("DGF proxy:", dgfProxy);
        console.log("ASR proxy:", asrProxy);
        console.log("ProxyAdmin:", proxyAdmin);
        console.log("VM:", vmAddr);
        console.log("WETH:", wethAddr);
        console.log("L2 Chain ID:", l2ChainId);
    }

    function _verify(address dgfProxy, address asrProxy) internal view {
        console.log("\n=== Post-upgrade verification ===");

        // Check versions
        string memory dgfVersion = DisputeGameFactory(dgfProxy).version();
        string memory asrVersion = AnchorStateRegistry(asrProxy).version();
        console.log("DGF:", dgfVersion);
        console.log("ASR:", asrVersion);
        require(keccak256(bytes(dgfVersion)) == keccak256(bytes("1.4.0")), "DGF version != 1.4.0");
        require(keccak256(bytes(asrVersion)) == keccak256(bytes("3.7.0")), "ASR version != 3.7.0");

        // Check DGF owner preserved
        address dgfOwner = DisputeGameFactory(dgfProxy).owner();
        require(dgfOwner != address(0), "DGF owner lost");
        console.log("DGF owner:", dgfOwner);

        // Check existing PERMISSIONED_CANNON (type 1) preserved
        address existingPDG = address(IDisputeGameFactory(dgfProxy).gameImpls(GameTypes.PERMISSIONED_CANNON));
        require(existingPDG != address(0), "PERMISSIONED_CANNON (type 1) lost");
        console.log("PERMISSIONED_CANNON impl:", existingPDG);

        // Check CANNON_KONA (type 8) registered
        address cannonKonaImpl = address(IDisputeGameFactory(dgfProxy).gameImpls(GameTypes.CANNON_KONA));
        require(cannonKonaImpl != address(0), "CANNON_KONA (type 8) not registered");
        console.log("CANNON_KONA impl:", cannonKonaImpl);

        // Check gameArgs set for CANNON_KONA
        bytes memory args = DisputeGameFactory(dgfProxy).gameArgs(GameTypes.CANNON_KONA);
        require(args.length > 0, "CANNON_KONA gameArgs not set");
        console.log("CANNON_KONA gameArgs length:", args.length);

        console.log("\n=== Upgrade v6 completed successfully ===");
        console.log("FDGv2 impl:", address(fdgV2Impl));
        console.log("PDGv2 impl:", address(pdgV2Impl));
        console.log("NOTE: respectedGameType NOT changed - still PERMISSIONED_CANNON (type 1)");
    }
}
