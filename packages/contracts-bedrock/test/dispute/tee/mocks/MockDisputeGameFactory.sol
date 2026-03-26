// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import {LibClone} from "@solady/utils/LibClone.sol";
import {IDisputeGame} from "interfaces/dispute/IDisputeGame.sol";
import {GameType, Claim, Timestamp} from "src/dispute/lib/Types.sol";

contract MockDisputeGameFactory {
    using LibClone for address;

    struct StoredGame {
        GameType gameType;
        Timestamp timestamp;
        IDisputeGame proxy;
        Claim rootClaim;
        bytes extraData;
    }

    struct Lookup {
        IDisputeGame proxy;
        Timestamp timestamp;
    }

    error IncorrectBondAmount();
    error NoImplementation(GameType gameType);

    address public owner;

    mapping(GameType => IDisputeGame) public gameImpls;
    mapping(GameType => uint256) public initBonds;
    mapping(GameType => bytes) public gameArgs;

    StoredGame[] internal storedGames;
    mapping(bytes32 => Lookup) internal storedLookups;

    modifier onlyOwner() {
        require(msg.sender == owner, "MockDisputeGameFactory: not owner");
        _;
    }

    constructor() {
        owner = msg.sender;
    }

    function create(GameType _gameType, Claim _rootClaim, bytes calldata _extraData)
        external
        payable
        returns (IDisputeGame proxy_)
    {
        IDisputeGame impl = gameImpls[_gameType];
        if (address(impl) == address(0)) revert NoImplementation(_gameType);
        if (msg.value != initBonds[_gameType]) revert IncorrectBondAmount();

        bytes32 parentHash = blockhash(block.number - 1);
        if (gameArgs[_gameType].length == 0) {
            proxy_ = IDisputeGame(
                address(impl).clone(abi.encodePacked(msg.sender, _rootClaim, parentHash, _extraData))
            );
        } else {
            proxy_ = IDisputeGame(
                address(impl).clone(
                    abi.encodePacked(msg.sender, _rootClaim, parentHash, _gameType, _extraData, gameArgs[_gameType])
                )
            );
        }

        proxy_.initialize{value: msg.value}();
        _storeGame(_gameType, _rootClaim, _extraData, proxy_, uint64(block.timestamp));
    }

    function pushGame(
        GameType _gameType,
        uint64 _timestamp,
        IDisputeGame _proxy,
        Claim _rootClaim,
        bytes memory _extraData
    )
        external
    {
        _storeGame(_gameType, _rootClaim, _extraData, _proxy, _timestamp);
    }

    function setImplementation(GameType _gameType, IDisputeGame _impl) external onlyOwner {
        gameImpls[_gameType] = _impl;
    }

    function setImplementation(GameType _gameType, IDisputeGame _impl, bytes calldata _args) external onlyOwner {
        gameImpls[_gameType] = _impl;
        gameArgs[_gameType] = _args;
    }

    function setInitBond(GameType _gameType, uint256 _initBond) external onlyOwner {
        initBonds[_gameType] = _initBond;
    }

    function games(GameType _gameType, Claim _rootClaim, bytes calldata _extraData)
        external
        view
        returns (IDisputeGame proxy_, Timestamp timestamp_)
    {
        Lookup memory lookup = storedLookups[_uuid(_gameType, _rootClaim, _extraData)];
        return (lookup.proxy, lookup.timestamp);
    }

    function findLatestGames(GameType, uint256, uint256) external pure returns (bytes memory) {
        revert("MockDisputeGameFactory: not implemented");
    }

    function gameAtIndex(uint256 _index)
        external
        view
        returns (GameType gameType_, Timestamp timestamp_, IDisputeGame proxy_)
    {
        StoredGame storage game = storedGames[_index];
        return (game.gameType, game.timestamp, game.proxy);
    }

    function gameCount() external view returns (uint256) {
        return storedGames.length;
    }

    function transferOwnership(address newOwner) external onlyOwner {
        owner = newOwner;
    }

    function initialize(address newOwner) external {
        owner = newOwner;
    }

    function proxyAdmin() external pure returns (address) {
        return address(0);
    }

    function proxyAdminOwner() external pure returns (address) {
        return address(0);
    }

    function initVersion() external pure returns (uint8) {
        return 1;
    }

    function renounceOwnership() external onlyOwner {
        owner = address(0);
    }

    function version() external pure returns (string memory) {
        return "mock";
    }

    function __constructor__() external { }

    function _storeGame(
        GameType _gameType,
        Claim _rootClaim,
        bytes memory _extraData,
        IDisputeGame _proxy,
        uint64 _timestamp
    )
        internal
    {
        Timestamp timestamp = Timestamp.wrap(_timestamp);
        storedGames.push(
            StoredGame({
                gameType: _gameType,
                timestamp: timestamp,
                proxy: _proxy,
                rootClaim: _rootClaim,
                extraData: _extraData
            })
        );
        storedLookups[_uuid(_gameType, _rootClaim, _extraData)] = Lookup({proxy: _proxy, timestamp: timestamp});
    }

    function _uuid(GameType _gameType, Claim _rootClaim, bytes memory _extraData) internal pure returns (bytes32) {
        return keccak256(abi.encode(_gameType, _rootClaim, _extraData));
    }
}
