// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import {Clone} from "@solady/utils/Clone.sol";
import {IDisputeGame} from "interfaces/dispute/IDisputeGame.sol";
import {Timestamp, GameStatus, GameType, Claim, Hash} from "src/dispute/lib/Types.sol";

contract MockCloneableDisputeGame is Clone, IDisputeGame {
    bool public initialized;
    bool public wasRespectedGameTypeWhenCreated;
    Timestamp public createdAt;
    Timestamp public resolvedAt;
    GameStatus public status;

    function initialize() external payable {
        require(!initialized, "MockCloneableDisputeGame: already initialized");
        initialized = true;
        createdAt = Timestamp.wrap(uint64(block.timestamp));
        status = GameStatus.IN_PROGRESS;
        msg.value;
    }

    function resolve() external returns (GameStatus status_) {
        status = GameStatus.DEFENDER_WINS;
        resolvedAt = Timestamp.wrap(uint64(block.timestamp));
        return status;
    }

    function gameType() external pure returns (GameType gameType_) {
        return GameType.wrap(0);
    }

    function gameCreator() external pure returns (address creator_) {
        return address(0);
    }

    function rootClaim() external pure returns (Claim rootClaim_) {
        return Claim.wrap(bytes32(0));
    }

    function l1Head() external pure returns (Hash l1Head_) {
        return Hash.wrap(bytes32(0));
    }

    function l2SequenceNumber() external pure returns (uint256 l2SequenceNumber_) {
        return 0;
    }

    function extraData() external pure returns (bytes memory extraData_) {
        return bytes("");
    }

    function gameData() external pure returns (GameType gameType_, Claim rootClaim_, bytes memory extraData_) {
        return (GameType.wrap(0), Claim.wrap(bytes32(0)), bytes(""));
    }
}
