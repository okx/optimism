// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import { Script, console2 } from "forge-std/Script.sol";
import { Proxy } from "src/universal/Proxy.sol";
import { DisputeGameFactory } from "src/dispute/DisputeGameFactory.sol";
import { AnchorStateRegistry } from "src/dispute/AnchorStateRegistry.sol";
import { IDisputeGameFactory } from "interfaces/dispute/IDisputeGameFactory.sol";
import { IDisputeGame } from "interfaces/dispute/IDisputeGame.sol";
import { IAnchorStateRegistry } from "interfaces/dispute/IAnchorStateRegistry.sol";
import { ISystemConfig } from "interfaces/L1/ISystemConfig.sol";
import { ITeeProofVerifier } from "interfaces/dispute/ITeeProofVerifier.sol";
import { IRiscZeroVerifier } from "interfaces/dispute/IRiscZeroVerifier.sol";
import { TeeDisputeGame, TEE_DISPUTE_GAME_TYPE } from "src/dispute/tee/TeeDisputeGame.sol";
import { TeeProofVerifier } from "src/dispute/tee/TeeProofVerifier.sol";
import { AccessManager } from "src/dispute/tee/AccessManager.sol";
import { IAccessManager } from "interfaces/dispute/zk/IAccessManager.sol";
import { MockSystemConfig } from "test/dispute/tee/mocks/MockSystemConfig.sol";
import { Duration, GameType, Hash, Proposal } from "src/dispute/lib/Types.sol";

/// @title DeployTeeFork
/// @notice Deploys TEE dispute game stack on a forked mainnet, using the real
///         RiscZeroVerifierRouter for ZK proof verification during enclave registration.
///
/// @dev Usage:
///   anvil --fork-url $ETH_RPC_URL --block-time 1
///
///   PRIVATE_KEY=0xac09...ff80 \
///   PROPOSERS=0x7099...79C8 \
///   CHALLENGERS=0x3C44...93BC \
///   RISC_ZERO_VERIFIER=0x8EaB2D97Dfce405A1692a21b3ff3A172d593D319 \
///   RISC_ZERO_IMAGE_ID=0x<guest image id> \
///   NITRO_ROOT_KEY=0x<96 bytes P384 root key hex> \
///   forge script scripts/tee/DeployTeeFork.s.sol --rpc-url http://localhost:8545 --broadcast
contract DeployTeeFork is Script {
    uint256 internal constant DEFENDER_BOND = 0.1 ether;
    uint256 internal constant CHALLENGER_BOND = 0.2 ether;
    uint64 internal constant MAX_CHALLENGE_DURATION = 300;
    uint64 internal constant MAX_PROVE_DURATION = 300;

    GameType internal constant TEE_GAME_TYPE = GameType.wrap(TEE_DISPUTE_GAME_TYPE);

    bytes32 internal constant DEFAULT_ANCHOR_BLOCK_HASH = keccak256("genesis-block");
    bytes32 internal constant DEFAULT_ANCHOR_STATE_HASH = keccak256("genesis-state");
    uint256 internal constant DEFAULT_ANCHOR_L2_BLOCK = 0;

    function run() external {
        uint256 deployerKey = vm.envUint("PRIVATE_KEY");
        address deployer = vm.addr(deployerKey);
        address[] memory proposers_ = vm.envAddress("PROPOSERS", ",");
        address[] memory challengers_ = vm.envAddress("CHALLENGERS", ",");

        vm.startBroadcast(deployerKey);

        bytes32 anchorBlockHash = vm.envOr("ANCHOR_BLOCK_HASH", DEFAULT_ANCHOR_BLOCK_HASH);
        bytes32 anchorStateHash = vm.envOr("ANCHOR_STATE_HASH", DEFAULT_ANCHOR_STATE_HASH);
        uint256 anchorL2Block = vm.envOr("ANCHOR_L2_BLOCK", DEFAULT_ANCHOR_L2_BLOCK);

        DisputeGameFactory factory = _deployFactory(deployer);
        AccessManager accessManager = new AccessManager(7 days, IDisputeGameFactory(address(factory)));
        for (uint256 i = 0; i < proposers_.length; i++) accessManager.setProposer(proposers_[i], true);
        for (uint256 i = 0; i < challengers_.length; i++) accessManager.setChallenger(challengers_[i], true);
        TeeProofVerifier teeProofVerifier = _deployVerifier();
        AnchorStateRegistry asr = _deployASR(deployer, factory);
        TeeDisputeGame impl = _deployGame(factory, teeProofVerifier, accessManager, asr);

        vm.stopBroadcast();

        console2.log("=== Deployed (fork mode) ===");
        console2.log("AnchorStateRegistry  :", address(asr));
        console2.log("TeeDisputeGame impl  :", address(impl));
        console2.log("TeeProofVerifier     :", address(teeProofVerifier));
        console2.log("DisputeGameFactory   :", address(factory));
    }

    function _deployVerifier() internal returns (TeeProofVerifier) {
        IRiscZeroVerifier rv = IRiscZeroVerifier(vm.envAddress("RISC_ZERO_VERIFIER"));
        bytes32 imageId = vm.envBytes32("RISC_ZERO_IMAGE_ID");
        bytes memory rootKey = vm.envBytes("NITRO_ROOT_KEY");
        console2.log("RiscZeroVerifier     :", address(rv));
        console2.log("imageId              :", vm.toString(imageId));
        return new TeeProofVerifier(rv, imageId, rootKey);
    }

    function _deployFactory(address deployer) internal returns (DisputeGameFactory) {
        DisputeGameFactory factoryImpl = new DisputeGameFactory();
        Proxy p = new Proxy(deployer);
        p.upgradeToAndCall(address(factoryImpl), abi.encodeCall(factoryImpl.initialize, (deployer)));
        return DisputeGameFactory(address(p));
    }

    function _deployASR(
        address deployer,
        DisputeGameFactory factory,
        bytes32 anchorBlockHash,
        bytes32 anchorStateHash,
        uint256 anchorL2Block
    )
        internal
        returns (AnchorStateRegistry)
    {
        MockSystemConfig sc = new MockSystemConfig(deployer);
        AnchorStateRegistry asrImpl = new AnchorStateRegistry(0);
        Proxy p = new Proxy(deployer);
        p.upgradeToAndCall(
            address(asrImpl),
            abi.encodeCall(
                asrImpl.initialize,
                (
                    ISystemConfig(address(sc)),
                    IDisputeGameFactory(address(factory)),
                    Proposal({
                        root: Hash.wrap(keccak256(abi.encode(anchorBlockHash, anchorStateHash))),
                        l2SequenceNumber: anchorL2Block
                    }),
                    TEE_GAME_TYPE
                )
            )
        );
        return AnchorStateRegistry(address(p));
    }

    function _deployGame(
        DisputeGameFactory factory,
        TeeProofVerifier verifier,
        AccessManager _accessManager,
        AnchorStateRegistry asr
    )
        internal
        returns (TeeDisputeGame)
    {
        TeeDisputeGame impl = new TeeDisputeGame(
            Duration.wrap(MAX_CHALLENGE_DURATION),
            Duration.wrap(MAX_PROVE_DURATION),
            IDisputeGameFactory(address(factory)),
            ITeeProofVerifier(address(verifier)),
            IAccessManager(address(_accessManager)),
            CHALLENGER_BOND,
            IAnchorStateRegistry(address(asr))
        );
        factory.setImplementation(TEE_GAME_TYPE, IDisputeGame(address(impl)), bytes(""));
        factory.setInitBond(TEE_GAME_TYPE, DEFENDER_BOND);
        return impl;
    }
}
