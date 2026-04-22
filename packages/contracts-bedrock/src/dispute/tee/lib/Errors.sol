// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

////////////////////////////////////////////////////////////////
//              `TeeDisputeGame` Errors                       //
////////////////////////////////////////////////////////////////

/// @notice Thrown when the claim has already been challenged.
error ClaimAlreadyChallenged();

/// @notice Thrown when the parent game is invalid.
error InvalidParentGame();

/// @notice Thrown when the parent game is not resolved.
error ParentGameNotResolved();

/// @notice Thrown when the game is over.
error GameOver();

/// @notice Thrown when the game is not over.
error GameNotOver();

/// @notice Thrown when the proposal status is invalid.
error InvalidProposalStatus();

/// @notice Thrown when the game is initialized by an incorrect factory.
error IncorrectDisputeGameFactory();
