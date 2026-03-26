// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";
import {GameType, Claim} from "src/dispute/lib/Types.sol";
import {IDisputeGameFactory} from "interfaces/dispute/IDisputeGameFactory.sol";
import {IDisputeGame} from "interfaces/dispute/IDisputeGame.sol";
import {IDisputeGameFactoryRouter} from "interfaces/dispute/IDisputeGameFactoryRouter.sol";

/// @title DisputeGameFactoryRouter
/// @notice Routes dispute game creation to the correct zone's DisputeGameFactory.
/// @dev Each zone (identified by a uint256 zoneId) maps to a DisputeGameFactory address.
contract DisputeGameFactoryRouter is Ownable, IDisputeGameFactoryRouter {
    /// @custom:semver 1.0.0
    string public constant version = "1.0.0";

    /// @notice Mapping of zoneId to DisputeGameFactory address.
    mapping(uint256 => address) public factories;

    constructor() {}

    ////////////////////////////////////////////////////////////////
    //                    Zone Management                         //
    ////////////////////////////////////////////////////////////////

    /// @notice Register a new zone with its factory address.
    function registerZone(uint256 zoneId, address factory) external onlyOwner {
        if (factory == address(0)) revert ZeroAddress();
        if (factories[zoneId] != address(0)) revert ZoneAlreadyRegistered(zoneId);
        factories[zoneId] = factory;
        emit ZoneRegistered(zoneId, factory);
    }

    /// @notice Update an existing zone's factory address.
    function updateZone(uint256 zoneId, address factory) external onlyOwner {
        if (factory == address(0)) revert ZeroAddress();
        address oldFactory = factories[zoneId];
        if (oldFactory == address(0)) revert ZoneNotRegistered(zoneId);
        factories[zoneId] = factory;
        emit ZoneUpdated(zoneId, oldFactory, factory);
    }

    /// @notice Remove a zone.
    function removeZone(uint256 zoneId) external onlyOwner {
        address factory = factories[zoneId];
        if (factory == address(0)) revert ZoneNotRegistered(zoneId);
        delete factories[zoneId];
        emit ZoneRemoved(zoneId, factory);
    }

    ////////////////////////////////////////////////////////////////
    //                    Game Creation                           //
    ////////////////////////////////////////////////////////////////

    /// @notice Create a single dispute game in the specified zone.
    function create(
        uint256 zoneId,
        GameType gameType,
        Claim rootClaim,
        bytes calldata extraData
    ) external payable returns (address proxy) {
        address factory = factories[zoneId];
        if (factory == address(0)) revert ZoneNotRegistered(zoneId);

        IDisputeGame game = IDisputeGameFactory(factory).create{value: msg.value}(
            gameType, rootClaim, extraData
        );
        proxy = address(game);
        emit GameCreated(zoneId, proxy);
    }

    /// @notice Create dispute games across multiple zones in a single transaction.
    /// @dev The sum of all params[i].bond must equal msg.value.
    function createBatch(CreateParams[] calldata params) external payable returns (address[] memory proxies) {
        if (params.length == 0) revert BatchEmpty();

        uint256 totalBonds;
        for (uint256 i = 0; i < params.length; i++) {
            totalBonds += params[i].bond;
        }
        if (totalBonds != msg.value) revert BatchBondMismatch(totalBonds, msg.value);

        proxies = new address[](params.length);
        for (uint256 i = 0; i < params.length; i++) {
            address factory = factories[params[i].zoneId];
            if (factory == address(0)) revert ZoneNotRegistered(params[i].zoneId);

            IDisputeGame game = IDisputeGameFactory(factory).create{value: params[i].bond}(
                params[i].gameType, params[i].rootClaim, params[i].extraData
            );
            proxies[i] = address(game);
            emit GameCreated(params[i].zoneId, proxies[i]);
        }

        emit BatchGamesCreated(params.length);
    }

    ////////////////////////////////////////////////////////////////
    //                    View Functions                          //
    ////////////////////////////////////////////////////////////////

    /// @notice Get the factory address for a zone.
    function getFactory(uint256 zoneId) external view returns (address) {
        return factories[zoneId];
    }
}
