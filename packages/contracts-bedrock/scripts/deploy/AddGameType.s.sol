// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

// Forge
import { Script } from "forge-std/Script.sol";
import { console } from "forge-std/console.sol";

// Scripts
import { DeployUtils } from "scripts/libraries/DeployUtils.sol";

// Interfaces
import { OPContractsManager } from "src/L1/OPContractsManager.sol";
import { ISystemConfig } from "interfaces/L1/ISystemConfig.sol";
import { IProxyAdmin } from "interfaces/universal/IProxyAdmin.sol";

import { IDelayedWETH } from "interfaces/dispute/IDelayedWETH.sol";
import { IBigStepper } from "interfaces/dispute/IBigStepper.sol";
import { GameType, Duration, Claim } from "src/dispute/lib/Types.sol";
import { IFaultDisputeGame } from "interfaces/dispute/IFaultDisputeGame.sol";



/// @title AddGameType
contract AddGameType is Script {
    struct Input {
        // Address that will be used for the DummyCaller contract
        address prank;
        // OPCM contract address
        OPContractsManager opcmImpl;
        // SystemConfig contract address
        ISystemConfig systemConfigProxy;
        // ProxyAdmin contract address
        IProxyAdmin opChainProxyAdmin;
        // DelayedWETH contract address (optional)
        IDelayedWETH delayedWETHProxy;
        // Game type to add
        GameType disputeGameType;
        // Absolute prestate for the game
        Claim disputeAbsolutePrestate;
        // Maximum game depth
        uint256 disputeMaxGameDepth;
        // Split depth for the game
        uint256 disputeSplitDepth;
        // Clock extension duration
        Duration disputeClockExtension;
        // Maximum clock duration
        Duration disputeMaxClockDuration;
        // Initial bond amount
        uint256 initialBond;
        // VM contract address
        IBigStepper vm;
        // Whether this is a permissioned game
        bool permissioned;
        // Salt mixer for deterministic addresses
        string saltMixer;
    }

    struct Output {
        IDelayedWETH delayedWETHProxy;
        IFaultDisputeGame faultDisputeGameProxy;
    }

    function run(Input memory _agi) public returns (Output memory) {
        console.log("=== AddGameType Script Started ===");
        console.log("OPCM Address:", address(_agi.opcmImpl));
        console.log("SystemConfig Address:", address(_agi.systemConfigProxy));
        console.log("ProxyAdmin Address:", address(_agi.opChainProxyAdmin));
        console.log("DelayedWETH Address:", address(_agi.delayedWETHProxy));
        console.log("VM Address:", address(_agi.vm));
        console.log("Prank Address:", _agi.prank);
        console.log("Salt Mixer:", _agi.saltMixer);
        console.log("Game Type:", _agi.disputeGameType.raw());
        console.log("Permissioned:", _agi.permissioned);
        console.log("Max Game Depth:", _agi.disputeMaxGameDepth);
        console.log("Split Depth:", _agi.disputeSplitDepth);
        console.log("Initial Bond:", _agi.initialBond);

        // Check contract code sizes
        console.log("=== Contract Code Size Checks ===");
        console.log("OPCM code size:", getCodeSize(address(_agi.opcmImpl)));
        console.log("SystemConfig code size:", getCodeSize(address(_agi.systemConfigProxy)));
        console.log("ProxyAdmin code size:", getCodeSize(address(_agi.opChainProxyAdmin)));
        console.log("VM code size:", getCodeSize(address(_agi.vm)));

        console.log("Deploying DummyCaller...");
        vm.broadcast(_agi.prank);
        DummyCaller dummyCaller = new DummyCaller(_agi.prank);

        vm.broadcast(_agi.prank);
        dummyCaller.setOpcmAddress(address(_agi.opcmImpl));

        console.log("DummyCaller deployed at:", address(dummyCaller));
        console.log("DummyCaller owner:", dummyCaller.owner());
        // Create the game input
        console.log("=== Creating Game Config ===");
        OPContractsManager.AddGameInput[] memory gameConfigs = new OPContractsManager.AddGameInput[](1);
        gameConfigs[0] = OPContractsManager.AddGameInput({
            saltMixer: _agi.saltMixer,
            systemConfig: _agi.systemConfigProxy,
            proxyAdmin: _agi.opChainProxyAdmin,
            delayedWETH: _agi.delayedWETHProxy,
            disputeGameType: _agi.disputeGameType,
            disputeAbsolutePrestate: _agi.disputeAbsolutePrestate,
            disputeMaxGameDepth: _agi.disputeMaxGameDepth,
            disputeSplitDepth: _agi.disputeSplitDepth,
            disputeClockExtension: _agi.disputeClockExtension,
            disputeMaxClockDuration: _agi.disputeMaxClockDuration,
            initialBond: _agi.initialBond,
            vm: _agi.vm,
            permissioned: _agi.permissioned
        });
        console.log("Game config created successfully");

        console.log("=== Calling addGameType via DummyCaller ===");

        // Get DisputeGameFactory address from SystemConfig and transfer ownership
        address disputeGameFactory = _agi.systemConfigProxy.disputeGameFactory();
        console.log("DisputeGameFactory address from SystemConfig:", disputeGameFactory);
        console.log("Transferring ownership to DummyCaller temporarily...");

        // Transfer ProxyAdmin ownership
        vm.broadcast(_agi.prank);
        IProxyAdmin(_agi.opChainProxyAdmin).transferOwnership(address(dummyCaller));

        // Transfer DisputeGameFactory ownership
        vm.broadcast(_agi.prank);
        (bool success1,) = disputeGameFactory.call(abi.encodeWithSignature("transferOwnership(address)", address(dummyCaller)));
        require(success1, "Failed to transfer DisputeGameFactory ownership");

        // Step 2: Execute addGameType operation
        console.log("Executing addGameType operation...");
        vm.broadcast(_agi.prank);
        (bool success, bytes memory result) = dummyCaller.addGameType(gameConfigs);

        if (!success) {
            console.log("=== CALL FAILED ===");
            if (result.length > 0) {
                console.log("Revert data:");
                console.logBytes(result);
            } else {
                console.log("No revert data available");
            }
        }
        require(success, "AddGameType: addGameType failed");

        // Decode the result and set it in the output
        console.log("=== Decoding Results ===");
        console.log("Decoding addGameType result...");
        OPContractsManager.AddGameOutput[] memory outputs = abi.decode(result, (OPContractsManager.AddGameOutput[]));
        console.log("Decoded successfully. Number of outputs:", outputs.length);
        require(outputs.length == 1, "AddGameType: unexpected number of outputs");

        console.log("DelayedWETH address:", address(outputs[0].delayedWETH));
        console.log("FaultDisputeGame address:", address(outputs[0].faultDisputeGame));

                // Step 3: DummyCaller releases ownership back to original owner
        console.log("=== Releasing ownership back ===");
        vm.broadcast(_agi.prank);
        dummyCaller.releaseOwnership(address(_agi.opChainProxyAdmin), disputeGameFactory, _agi.prank);

        console.log("Verified ownership restoration:");
        console.log("  ProxyAdmin owner:", IProxyAdmin(_agi.opChainProxyAdmin).owner());
        (bool success3, bytes memory ownerResult) = disputeGameFactory.call(abi.encodeWithSignature("owner()"));
        if (success3) {
            address dgfOwner = abi.decode(ownerResult, (address));
            console.log("  DisputeGameFactory owner:", dgfOwner);
        }

        console.log("=== AddGameType Script Completed Successfully ===");
        return Output({ delayedWETHProxy: outputs[0].delayedWETH, faultDisputeGameProxy: outputs[0].faultDisputeGame });
    }

    function checkOutput(Output memory _ago) internal view {
        DeployUtils.assertValidContractAddress(address(_ago.delayedWETHProxy));
        DeployUtils.assertValidContractAddress(address(_ago.faultDisputeGameProxy));
    }

    /// @notice Get contract code size using assembly
    function getCodeSize(address _addr) internal view returns (uint256 size) {
        assembly {
            size := extcodesize(_addr)
        }
    }
}

/// @title DummyCaller
contract DummyCaller {
    address internal _opcmAddr;
    address public owner;

    modifier onlyOwner() {
        require(msg.sender == owner, "DummyCaller: caller is not the owner");
        _;
    }

    constructor(address _owner) {
        owner = _owner;
    }

    function setOpcmAddress(address _opcm) external onlyOwner {
        _opcmAddr = _opcm;
    }

    /// @notice Release ownership back to original owner
    function releaseOwnership(address _proxyAdmin, address _disputeGameFactory, address _originalOwner) external onlyOwner {
        console.log("Releasing ownership back to original owner...");

        // Transfer ownership back
        IProxyAdmin(_proxyAdmin).transferOwnership(_originalOwner);
        (bool success,) = _disputeGameFactory.call(abi.encodeWithSignature("transferOwnership(address)", _originalOwner));
        require(success, "Failed to release DisputeGameFactory ownership");
    }

    function addGameType(OPContractsManager.AddGameInput[] memory _gameConfigs) external onlyOwner returns (bool, bytes memory) {
        console.log("=== DummyCaller.addGameType called ===");
        console.log("Number of game configs:", _gameConfigs.length);
                console.log("OPCM address stored:", _opcmAddr);

        address opcmAddr = _opcmAddr;
        uint256 codeSize;
        assembly {
            codeSize := extcodesize(opcmAddr)
        }
        console.log("OPCM code size:", codeSize);

        for (uint256 i = 0; i < _gameConfigs.length; i++) {
            console.log("Game config", i, ":");
            console.log("  Salt mixer:", _gameConfigs[i].saltMixer);
            console.log("  SystemConfig:", address(_gameConfigs[i].systemConfig));
            console.log("  ProxyAdmin:", address(_gameConfigs[i].proxyAdmin));
            console.log("  DelayedWETH:", address(_gameConfigs[i].delayedWETH));
            console.log("  Game type:", _gameConfigs[i].disputeGameType.raw());
            console.log("  Permissioned:", _gameConfigs[i].permissioned);
        }

        console.log("Encoding delegatecall data...");
        bytes memory data = abi.encodeCall(OPContractsManager.addGameType, _gameConfigs);
        console.log("Data length:", data.length);

        console.log("Performing delegatecall to OPCM...");
        (bool success, bytes memory result) = _opcmAddr.delegatecall(data);
        console.log("Delegatecall completed. Success:", success);
        console.log("Result length:", result.length);

        if (!success) {
            console.log("=== DELEGATECALL FAILED ===");
            if (result.length > 0) {
                console.log("Revert data from delegatecall:");
                console.logBytes(result);
            } else {
                console.log("No revert data from delegatecall");
            }
        } else {
            console.log("Delegatecall succeeded!");
        }

        return (success, result);
    }
}
