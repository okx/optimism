// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import {IAnchorStateRegistry} from "interfaces/dispute/IAnchorStateRegistry.sol";
import {Timestamp, GameStatus, GameType, Claim, Hash} from "src/dispute/lib/Types.sol";

contract MockStatusDisputeGame {
    Timestamp public createdAt;
    Timestamp public resolvedAt;
    GameStatus public status;
    GameType internal _gameType;
    Claim internal _rootClaim;
    Hash internal _l1Head;
    uint256 internal _l2SequenceNumber;
    bytes internal _extraData;
    address internal _gameCreator;
    bool public wasRespectedGameTypeWhenCreated;
    IAnchorStateRegistry public anchorStateRegistry;

    constructor(
        address creator_,
        GameType gameType_,
        Claim rootClaim_,
        uint256 l2SequenceNumber_,
        bytes memory extraData_,
        GameStatus status_,
        uint64 createdAt_,
        uint64 resolvedAt_,
        bool respected_,
        IAnchorStateRegistry anchorStateRegistry_
    ) {
        _gameCreator = creator_;
        _gameType = gameType_;
        _rootClaim = rootClaim_;
        _l2SequenceNumber = l2SequenceNumber_;
        _extraData = extraData_;
        status = status_;
        createdAt = Timestamp.wrap(createdAt_);
        resolvedAt = Timestamp.wrap(resolvedAt_);
        wasRespectedGameTypeWhenCreated = respected_;
        anchorStateRegistry = anchorStateRegistry_;
    }

    function initialize() external payable { }

    function resolve() external view returns (GameStatus status_) {
        return status;
    }

    function setStatus(GameStatus status_) external {
        status = status_;
    }

    function setResolvedAt(uint64 resolvedAt_) external {
        resolvedAt = Timestamp.wrap(resolvedAt_);
    }

    function gameData() external view returns (GameType gameType_, Claim rootClaim_, bytes memory extraData_) {
        return (_gameType, _rootClaim, _extraData);
    }

    function gameType() external view returns (GameType gameType_) {
        return _gameType;
    }

    function rootClaim() external view returns (Claim rootClaim_) {
        return _rootClaim;
    }

    function l1Head() external view returns (Hash l1Head_) {
        return _l1Head;
    }

    function l2SequenceNumber() external view returns (uint256 l2SequenceNumber_) {
        return _l2SequenceNumber;
    }

    function extraData() external view returns (bytes memory extraData_) {
        return _extraData;
    }

    function gameCreator() external view returns (address creator_) {
        return _gameCreator;
    }
}
