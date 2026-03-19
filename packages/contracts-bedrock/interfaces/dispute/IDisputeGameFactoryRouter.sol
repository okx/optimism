// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import {GameType, Claim} from "src/dispute/lib/Types.sol";

/// @title IDisputeGameFactoryRouter
/// @notice Interface for routing dispute game creation across multiple zone factories.
interface IDisputeGameFactoryRouter {
    /// @notice Parameters for creating a single dispute game in a batch.
    struct CreateParams {
        uint256 zoneId;
        GameType gameType;
        Claim rootClaim;
        bytes extraData;
        uint256 bond;
    }

    // ============ Events ============

    event ZoneSet(uint256 indexed zoneId, address indexed oldFactory, address indexed newFactory);
    event GameCreated(uint256 indexed zoneId, address indexed proxy);
    event BatchGamesCreated(uint256 count);

    // ============ Errors ============

    error ZoneNotRegistered(uint256 zoneId);
    error BatchEmpty();
    error BatchBondMismatch(uint256 totalBonds, uint256 msgValue);

    // ============ Functions ============

    function setZone(uint256 zoneId, address factory) external;
    function create(uint256 zoneId, GameType gameType, Claim rootClaim, bytes calldata extraData) external payable returns (address proxy);
    function createBatch(CreateParams[] calldata params) external payable returns (address[] memory proxies);
}
