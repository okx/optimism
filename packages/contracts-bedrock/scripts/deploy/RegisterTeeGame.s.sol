// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import {Script, console2} from "forge-std/Script.sol";
import {IDisputeGameFactory} from "interfaces/dispute/IDisputeGameFactory.sol";
import {IAnchorStateRegistry} from "interfaces/dispute/IAnchorStateRegistry.sol";
import {IDisputeGame} from "interfaces/dispute/IDisputeGame.sol";
import {GameType} from "src/dispute/lib/Types.sol";

contract RegisterTeeGame is Script {
    uint32 internal constant TEE_GAME_TYPE = 1960;
    string internal constant GAME_ARGS_UNSUPPORTED =
        "RegisterTeeGame: GAME_ARGS is unsupported for gameType=1960";

    function run() external {
        uint256 deployerKey = vm.envUint("PRIVATE_KEY");
        IDisputeGameFactory disputeGameFactory = IDisputeGameFactory(vm.envAddress("DISPUTE_GAME_FACTORY"));
        IDisputeGame teeDisputeGame = IDisputeGame(vm.envAddress("TEE_DISPUTE_GAME_IMPL"));
        uint256 initBond = vm.envUint("INIT_BOND");
        bytes memory gameArgs = vm.envOr("GAME_ARGS", bytes(""));
        bool setRespectedGameType = vm.envOr("SET_RESPECTED_GAME_TYPE", false);

        require(gameArgs.length == 0, GAME_ARGS_UNSUPPORTED);

        vm.startBroadcast(deployerKey);

        disputeGameFactory.setImplementation(GameType.wrap(TEE_GAME_TYPE), teeDisputeGame);
        disputeGameFactory.setInitBond(GameType.wrap(TEE_GAME_TYPE), initBond);

        if (setRespectedGameType) {
            IAnchorStateRegistry anchorStateRegistry = IAnchorStateRegistry(vm.envAddress("ANCHOR_STATE_REGISTRY"));
            anchorStateRegistry.setRespectedGameType(GameType.wrap(TEE_GAME_TYPE));
            console2.log("anchorStateRegistry respected game type set", TEE_GAME_TYPE);
        }

        vm.stopBroadcast();

        console2.log("registered gameType", TEE_GAME_TYPE);
        console2.log("teeDisputeGame", address(teeDisputeGame));
        console2.log("initBond", initBond);
    }
}
