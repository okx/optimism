// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";
import {IDisputeGameFactory} from "interfaces/dispute/IDisputeGameFactory.sol";
import {GameType, Timestamp} from "src/dispute/lib/Types.sol";

/// @dev Game type constant for TEE Dispute Game.
uint32 constant TEE_DISPUTE_GAME_TYPE = 1960;

/// @title AccessManager
/// @notice Manages permissions for dispute game proposers and challengers.
contract AccessManager is Ownable {
    ////////////////////////////////////////////////////////////////
    //                         Events                             //
    ////////////////////////////////////////////////////////////////

    /// @notice Event emitted when proposer permissions are updated.
    event ProposerPermissionUpdated(address indexed proposer, bool allowed);

    /// @notice Event emitted when challenger permissions are updated.
    event ChallengerPermissionUpdated(address indexed challenger, bool allowed);

    ////////////////////////////////////////////////////////////////
    //                         State Vars                         //
    ////////////////////////////////////////////////////////////////

    /// @notice Tracks whitelisted proposers.
    mapping(address => bool) public proposers;

    /// @notice Tracks whitelisted challengers.
    mapping(address => bool) public challengers;

    /// @notice The timeout (in seconds) after which permissionless proposing is allowed (immutable).
    uint256 public immutable FALLBACK_TIMEOUT;

    /// @notice The dispute game factory address.
    IDisputeGameFactory public immutable DISPUTE_GAME_FACTORY;

    /// @notice The timestamp of this contract's creation. Used for permissionless fallback proposals.
    uint256 public immutable DEPLOYMENT_TIMESTAMP;

    ////////////////////////////////////////////////////////////////
    //                      Constructor                           //
    ////////////////////////////////////////////////////////////////

    /// @notice Constructor sets the fallback timeout and initializes timestamp.
    /// @param _fallbackTimeout The timeout in seconds after last proposal when permissionless mode activates.
    /// @param _disputeGameFactory The dispute game factory address.
    constructor(uint256 _fallbackTimeout, IDisputeGameFactory _disputeGameFactory) {
        FALLBACK_TIMEOUT = _fallbackTimeout;
        DISPUTE_GAME_FACTORY = _disputeGameFactory;
        DEPLOYMENT_TIMESTAMP = block.timestamp;
    }

    ////////////////////////////////////////////////////////////////
    //                      Functions                             //
    ////////////////////////////////////////////////////////////////

    /// @notice Allows the owner to whitelist or un-whitelist proposers.
    /// @param _proposer The address to set in the proposers mapping.
    /// @param _allowed True if whitelisting, false otherwise.
    function setProposer(address _proposer, bool _allowed) external onlyOwner {
        proposers[_proposer] = _allowed;
        emit ProposerPermissionUpdated(_proposer, _allowed);
    }

    /// @notice Allows the owner to whitelist or un-whitelist challengers.
    /// @param _challenger The address to set in the challengers mapping.
    /// @param _allowed True if whitelisting, false otherwise.
    function setChallenger(address _challenger, bool _allowed) external onlyOwner {
        challengers[_challenger] = _allowed;
        emit ChallengerPermissionUpdated(_challenger, _allowed);
    }

    /// @notice Returns the last proposal timestamp.
    /// @return The last proposal timestamp.
    function getLastProposalTimestamp() public view returns (uint256) {
        GameType gameType = GameType.wrap(TEE_DISPUTE_GAME_TYPE);
        uint256 numGames = DISPUTE_GAME_FACTORY.gameCount();

        if (numGames == 0) {
            return DEPLOYMENT_TIMESTAMP;
        }

        uint256 i = numGames - 1;
        while (true) {
            (GameType gameTypeAtIndex, Timestamp timestamp,) = DISPUTE_GAME_FACTORY.gameAtIndex(i);
            uint256 gameTimestamp = uint256(timestamp.raw());

            if (gameTimestamp < DEPLOYMENT_TIMESTAMP) {
                return DEPLOYMENT_TIMESTAMP;
            }

            if (gameTypeAtIndex.raw() == gameType.raw()) {
                return gameTimestamp;
            }

            if (i == 0) {
                break;
            }

            unchecked {
                --i;
            }
        }

        return DEPLOYMENT_TIMESTAMP;
    }

    /// @notice Returns whether proposal fallback timeout has elapsed.
    /// @return Whether permissionless proposing is active.
    function isProposalPermissionlessMode() public view returns (bool) {
        if (proposers[address(0)]) {
            return true;
        }

        uint256 lastProposalTimestamp = getLastProposalTimestamp();
        return block.timestamp - lastProposalTimestamp > FALLBACK_TIMEOUT;
    }

    /// @notice Checks if an address is allowed to propose.
    /// @param _proposer The address to check.
    /// @return allowed_ Whether the address is allowed to propose.
    function isAllowedProposer(address _proposer) external view returns (bool allowed_) {
        allowed_ = proposers[_proposer] || isProposalPermissionlessMode();
    }

    /// @notice Checks if an address is allowed to challenge.
    /// @param _challenger The address to check.
    /// @return allowed_ Whether the address is allowed to challenge.
    function isAllowedChallenger(address _challenger) external view returns (bool allowed_) {
        allowed_ = challengers[address(0)] || challengers[_challenger];
    }
}
