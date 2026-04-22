// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import {IAnchorStateRegistry} from "interfaces/dispute/IAnchorStateRegistry.sol";
import {IDisputeGameFactory} from "interfaces/dispute/IDisputeGameFactory.sol";
import {IDisputeGame} from "interfaces/dispute/IDisputeGame.sol";
import {IFaultDisputeGame} from "interfaces/dispute/IFaultDisputeGame.sol";
import {ISystemConfig} from "interfaces/L1/ISystemConfig.sol";
import {ISuperchainConfig} from "interfaces/L1/ISuperchainConfig.sol";
import {IProxyAdmin} from "interfaces/universal/IProxyAdmin.sol";
import {GameType, Hash, Proposal} from "src/dispute/lib/Types.sol";

contract MockAnchorStateRegistry is IAnchorStateRegistry {
    struct Flags {
        bool registered;
        bool respected;
        bool blacklisted;
        bool retired;
        bool finalized;
        bool proper;
        bool claimValid;
    }

    uint8 public initVersion = 1;
    ISystemConfig public systemConfig;
    IDisputeGameFactory public disputeGameFactory;
    IFaultDisputeGame public anchorGame;
    Proposal internal anchorRoot;
    mapping(IDisputeGame => bool) public disputeGameBlacklist;
    mapping(address => Flags) internal flags;
    GameType public respectedGameType;
    uint64 public retirementTimestamp;
    bool public paused;
    bool public revertOnSetAnchorState;
    IDisputeGame public lastSetAnchorState;

    function initialize(
        ISystemConfig _systemConfig,
        IDisputeGameFactory _disputeGameFactory,
        Proposal memory _startingAnchorRoot,
        GameType _startingRespectedGameType
    )
        external
    {
        systemConfig = _systemConfig;
        disputeGameFactory = _disputeGameFactory;
        anchorRoot = _startingAnchorRoot;
        respectedGameType = _startingRespectedGameType;
    }

    function anchors(GameType) external view returns (Hash, uint256) {
        return (anchorRoot.root, anchorRoot.l2SequenceNumber);
    }

    function setAnchor(Hash root_, uint256 l2SequenceNumber_) external {
        anchorRoot = Proposal({root: root_, l2SequenceNumber: l2SequenceNumber_});
    }

    function setRespectedGameType(GameType _gameType) external {
        respectedGameType = _gameType;
    }

    function updateRetirementTimestamp() external {
        retirementTimestamp = uint64(block.timestamp);
    }

    function blacklistDisputeGame(IDisputeGame game) external {
        disputeGameBlacklist[game] = true;
        flags[address(game)].blacklisted = true;
    }

    function setPaused(bool value) external {
        paused = value;
    }

    function setRevertOnSetAnchorState(bool value) external {
        revertOnSetAnchorState = value;
    }

    function setGameFlags(
        IDisputeGame game,
        bool registered_,
        bool respected_,
        bool blacklisted_,
        bool retired_,
        bool finalized_,
        bool proper_,
        bool claimValid_
    )
        external
    {
        flags[address(game)] = Flags({
            registered: registered_,
            respected: respected_,
            blacklisted: blacklisted_,
            retired: retired_,
            finalized: finalized_,
            proper: proper_,
            claimValid: claimValid_
        });
        disputeGameBlacklist[game] = blacklisted_;
    }

    function isGameBlacklisted(IDisputeGame game) external view returns (bool) {
        return flags[address(game)].blacklisted;
    }

    function isGameProper(IDisputeGame game) external view returns (bool) {
        return flags[address(game)].proper;
    }

    function isGameRegistered(IDisputeGame game) external view returns (bool) {
        return flags[address(game)].registered;
    }

    function isGameResolved(IDisputeGame) external pure returns (bool) {
        return false;
    }

    function isGameRespected(IDisputeGame game) external view returns (bool) {
        return flags[address(game)].respected;
    }

    function isGameRetired(IDisputeGame game) external view returns (bool) {
        return flags[address(game)].retired;
    }

    function isGameFinalized(IDisputeGame game) external view returns (bool) {
        return flags[address(game)].finalized;
    }

    function isGameClaimValid(IDisputeGame game) external view returns (bool) {
        return flags[address(game)].claimValid;
    }

    function setAnchorState(IDisputeGame game) external {
        if (revertOnSetAnchorState) revert AnchorStateRegistry_InvalidAnchorGame();
        anchorGame = IFaultDisputeGame(address(game));
        lastSetAnchorState = game;
    }

    function getAnchorRoot() external view returns (Hash, uint256) {
        return (anchorRoot.root, anchorRoot.l2SequenceNumber);
    }

    function disputeGameFinalityDelaySeconds() external pure returns (uint256) {
        return 0;
    }

    function superchainConfig() external view returns (ISuperchainConfig) {
        return systemConfig.superchainConfig();
    }

    function version() external pure returns (string memory) {
        return "mock";
    }

    function proxyAdmin() external pure returns (IProxyAdmin) {
        return IProxyAdmin(address(0));
    }

    function proxyAdminOwner() external pure returns (address) {
        return address(0);
    }

    function __constructor__(uint256) external { }
}
