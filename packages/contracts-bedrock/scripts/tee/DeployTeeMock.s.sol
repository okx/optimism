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
import { TeeDisputeGame, TEE_DISPUTE_GAME_TYPE } from "src/dispute/tee/TeeDisputeGame.sol";
import { TeeProofVerifier } from "src/dispute/tee/TeeProofVerifier.sol";
import { AccessManager } from "src/dispute/tee/AccessManager.sol";
import { IAccessManager } from "interfaces/dispute/zk/IAccessManager.sol";
import { MockRiscZeroVerifier } from "test/dispute/tee/mocks/MockRiscZeroVerifier.sol";
import { MockSystemConfig } from "test/dispute/tee/mocks/MockSystemConfig.sol";
import { Duration, GameType, Hash, Proposal } from "src/dispute/lib/Types.sol";

/// @title DeployTeeMock
/// @notice Deploys the full TEE dispute game stack with MockRiscZeroVerifier for local testing.
///         register() accepts empty seal in this mode.
///
/// @dev Usage:
///   anvil --block-time 1
///
///   PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
///   PROPOSERS=0x70997970C51812dc3A010C7d01b50e0d17dc79C8 \
///   CHALLENGERS=0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC \
///   forge script scripts/tee/DeployTeeMock.s.sol --rpc-url http://localhost:8545 --broadcast
contract DeployTeeMock is Script {
    uint256 internal constant DEFENDER_BOND = 0.1 ether;
    uint256 internal constant CHALLENGER_BOND = 0.2 ether;
    uint64 internal constant MAX_CHALLENGE_DURATION = 300; // 5 min
    uint64 internal constant MAX_PROVE_DURATION = 300; // 5 min

    GameType internal constant TEE_GAME_TYPE = GameType.wrap(TEE_DISPUTE_GAME_TYPE);

    bytes32 internal constant ANCHOR_BLOCK_HASH = keccak256("genesis-block");
    bytes32 internal constant ANCHOR_STATE_HASH = keccak256("genesis-state");
    uint256 internal constant ANCHOR_L2_BLOCK = 0;

    function run() external {
        uint256 deployerKey = vm.envUint("PRIVATE_KEY");
        address deployer = vm.addr(deployerKey);
        address[] memory proposers_ = vm.envAddress("PROPOSERS", ",");
        address[] memory challengers_ = vm.envAddress("CHALLENGERS", ",");

        vm.startBroadcast(deployerKey);

        // 1. MockRiscZeroVerifier -- verify() is a no-op
        MockRiscZeroVerifier mockRiscZero = new MockRiscZeroVerifier();

        // 2. DisputeGameFactory (via Proxy) -- needed before AccessManager
        DisputeGameFactory factory = _deployFactory(deployer);

        // 3. AccessManager + TeeProofVerifier with mock verifier + dummy imageId/rootKey
        AccessManager accessManager = new AccessManager(7 days, IDisputeGameFactory(address(factory)));
        for (uint256 i = 0; i < proposers_.length; i++) accessManager.setProposer(proposers_[i], true);
        for (uint256 i = 0; i < challengers_.length; i++) accessManager.setChallenger(challengers_[i], true);
        bytes32 imageId = keccak256("mock-image-id");
        bytes memory rootKey = abi.encodePacked(bytes32(uint256(1)), bytes32(uint256(2)), bytes32(uint256(3)));
        TeeProofVerifier teeProofVerifier =
            new TeeProofVerifier(mockRiscZero, imageId, rootKey, IAccessManager(address(accessManager)));

        // 4. AnchorStateRegistry (via Proxy)
        AnchorStateRegistry anchorStateRegistry = _deployAnchorStateRegistry(deployer, factory);


        // 5. TeeDisputeGame implementation + register in factory
        TeeDisputeGame teeDisputeGame = _deployAndRegisterGame(factory, teeProofVerifier, anchorStateRegistry);

        vm.stopBroadcast();

        console2.log("=== Deployed Addresses ===");
        console2.log("MockRiscZeroVerifier :", address(mockRiscZero));
        console2.log("TeeProofVerifier     :", address(teeProofVerifier));
        console2.log("DisputeGameFactory   :", address(factory));
        console2.log("AnchorStateRegistry  :", address(anchorStateRegistry));
        console2.log("TeeDisputeGame impl  :", address(teeDisputeGame));
        console2.log("");
        console2.log("=== Config ===");
        console2.log("DEFENDER_BOND        :", DEFENDER_BOND);
        console2.log("CHALLENGER_BOND      :", CHALLENGER_BOND);
        console2.log("TEE_GAME_TYPE        :", TEE_DISPUTE_GAME_TYPE);
        console2.log("ANCHOR_L2_BLOCK      :", ANCHOR_L2_BLOCK);
    }

    function _deployFactory(address deployer) internal returns (DisputeGameFactory) {
        DisputeGameFactory factoryImpl = new DisputeGameFactory();
        Proxy factoryProxy = new Proxy(deployer);
        factoryProxy.upgradeToAndCall(address(factoryImpl), abi.encodeCall(factoryImpl.initialize, (deployer)));
        return DisputeGameFactory(address(factoryProxy));
    }

    function _deployAnchorStateRegistry(
        address deployer,
        DisputeGameFactory factory
    )
        internal
        returns (AnchorStateRegistry)
    {
        MockSystemConfig systemConfig = new MockSystemConfig(deployer);
        AnchorStateRegistry asrImpl = new AnchorStateRegistry(0);
        Proxy asrProxy = new Proxy(deployer);
        asrProxy.upgradeToAndCall(
            address(asrImpl),
            abi.encodeCall(
                asrImpl.initialize,
                (
                    ISystemConfig(address(systemConfig)),
                    IDisputeGameFactory(address(factory)),
                    Proposal({
                        root: Hash.wrap(keccak256(abi.encode(ANCHOR_BLOCK_HASH, ANCHOR_STATE_HASH))),
                        l2SequenceNumber: ANCHOR_L2_BLOCK
                    }),
                    TEE_GAME_TYPE
                )
            )
        );
        return AnchorStateRegistry(address(asrProxy));
    }

    function _deployAndRegisterGame(
        DisputeGameFactory factory,
        TeeProofVerifier teeProofVerifier,
        AnchorStateRegistry anchorStateRegistry
    )
        internal
        returns (TeeDisputeGame)
    {
        TeeDisputeGame teeDisputeGame = new TeeDisputeGame(
            Duration.wrap(MAX_CHALLENGE_DURATION),
            Duration.wrap(MAX_PROVE_DURATION),
            IDisputeGameFactory(address(factory)),
            ITeeProofVerifier(address(teeProofVerifier)),
            CHALLENGER_BOND,
            IAnchorStateRegistry(address(anchorStateRegistry))
        );

        factory.setImplementation(TEE_GAME_TYPE, IDisputeGame(address(teeDisputeGame)), bytes(""));
        factory.setInitBond(TEE_GAME_TYPE, DEFENDER_BOND);

        return teeDisputeGame;
    }
}
