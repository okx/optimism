// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import { Script } from "forge-std/Script.sol";
import { console } from "forge-std/console.sol";
import { Vm } from "forge-std/Vm.sol";
import { Strings } from "@openzeppelin/contracts/utils/Strings.sol";

// Interfaces
import { ISemver } from "interfaces/universal/ISemver.sol";

// Contracts
import { SuperchainConfig } from "../src/L1/SuperchainConfig.sol";
import { ProtocolVersions } from "../src/L1/ProtocolVersions.sol";
import { ProxyAdmin } from "../src/universal/ProxyAdmin.sol";
import { SystemConfig } from "../src/L1/SystemConfig.sol";
import { OptimismPortal2 } from "../src/L1/OptimismPortal2.sol";
import { DepositedOKBAdapter } from "../src/L1/DepositedOKBAdapter.sol";
import { DisputeGameFactory } from "../src/dispute/DisputeGameFactory.sol";
import { PermissionedDisputeGame } from "../src/dispute/PermissionedDisputeGame.sol";
import { L1StandardBridge } from "../src/L1/L1StandardBridge.sol";
import { L1ERC721Bridge } from "../src/L1/L1ERC721Bridge.sol";
import { L1CrossDomainMessenger } from "../src/L1/L1CrossDomainMessenger.sol";
import { DelayedWETH } from "../src/dispute/DelayedWETH.sol";
import { ETHLockbox } from "../src/L1/ETHLockbox.sol";
import { PreimageOracle } from "../src/cannon/PreimageOracle.sol";
import { MIPS64 } from "../src/cannon/MIPS64.sol";
import { OptimismMintableERC20Factory } from "../src/universal/OptimismMintableERC20Factory.sol";
import { AddressManager } from "../src/legacy/AddressManager.sol";
import { AnchorStateRegistry } from "../src/dispute/AnchorStateRegistry.sol";
import { OptimismPortalInterop } from "../src/L1/OptimismPortalInterop.sol";
import { IDisputeGameFactory } from "interfaces/dispute/IDisputeGameFactory.sol";
import { IPreimageOracle } from "interfaces/cannon/IPreimageOracle.sol";

// OPCM contracts
import {
    OPContractsManager,
    OPContractsManagerContractsContainer,
    OPContractsManagerGameTypeAdder,
    OPContractsManagerDeployer,
    OPContractsManagerUpgrader,
    OPContractsManagerInteropMigrator
} from "../src/L1/OPContractsManager.sol";

import { OPContractsManagerStandardValidator } from "../src/L1/OPContractsManagerStandardValidator.sol";

contract MockOKB {
    function totalSupply() external pure returns (uint256) {
        return 1000000 ether;
    }
}

contract CheckContracts is Script {
    using Strings for string;

    struct ContractEntry {
        string name;
        address proxy;
        address impl;
        bool hasSemver;
        string localVersion;
    }

    ContractEntry[] public entries;

    bytes32 constant IMPLEMENTATION_SLOT = 0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc;

    function setUp() public {
        // Helper to get version from a new instance

        // SuperchainConfig (No args)
        addEntry(
            "SuperchainConfig",
            0x6a95D7aaC3d41761426761Af031C5034B7b347d4,
            0xb08Cc720F511062537ca78BdB0AE691F04F5a957,
            true,
            tryGetVersion(address(new SuperchainConfig()))
        );

        // ProtocolVersions (No args)
        addEntry(
            "ProtocolVersions",
            0xC1Fb115d8249a7e6b27c8Bc6914Cab7eDF0b0F7E,
            0x1f734B89Bb1B422B9910118fb8d44C06E33d4DdA,
            true,
            tryGetVersion(address(new ProtocolVersions()))
        );

        // ProxyAdmin (Takes owner)
        addEntry(
            "ProxyAdmin (Superchain)",
            address(0),
            0xC6901aBf8D39079d6b028dA550BB643f10840552,
            true,
            tryGetVersion(address(new ProxyAdmin(address(1))))
        );

        // ProxyAdmin (chain)
        addEntry(
            "ProxyAdmin (Chain)",
            address(0),
            0x313ce9Cec2070B519f13BDaFe07eabb4f215FEE6,
            true,
            tryGetVersion(address(new ProxyAdmin(address(1))))
        );

        // SystemConfig (No args)
        addEntry(
            "SystemConfig",
            0x5065809Af286321a05fBF85713B5D5De7C8f0433,
            0xee63dC4B835b2790A171fd0149566B1D51E5ae73,
            true,
            tryGetVersion(address(new SystemConfig()))
        );

        // OptimismPortal2
        // Constructor: (uint256 _proofMaturityDelaySeconds)
        OptimismPortal2 op2 = new OptimismPortal2(7 days);
        addEntry(
            "OptimismPortal2",
            0x64057ad1DdAc804d0D26A7275b193D9DACa19993,
            0xa0fEfC3A457F6A1aE2d81FC172D6dE090a9F4033,
            true,
            tryGetVersion(address(op2))
        );

        // DepositedOKBAdapter (okb, portal, owner)
        MockOKB mockOKB = new MockOKB();
        DepositedOKBAdapter dokb = new DepositedOKBAdapter(address(mockOKB), payable(address(1)), address(1));
        addEntry("DepositedOKBAdapter", address(0), 0x6CB0E5cC796B9dd4436A72963EdFb6cA8Ba01A9d, false, "N/A");

        // DisputeGameFactory (No args)
        addEntry(
            "DisputeGameFactory",
            0x9D4c8FAEadDdDeeE1Ed0c92dAbAD815c2484f675,
            0x74Fac1D45B98bae058F8F566201c9A81B85C7D50,
            true,
            tryGetVersion(address(new DisputeGameFactory()))
        );

        // PermissionedDisputeGame
        // Complex args. Skipping local version for now.
        addEntry(
            "PermissionedDisputeGame",
            address(0),
            0xEeDa796a23bc98726e47934ca9B54fDDa5a608e8,
            true,
            "SKIPPED (Complex Constructor)"
        );

        // L1StandardBridge (No args)
        addEntry(
            "L1StandardBridge",
            0xAecF995ABf9E7eDE7ae0CE65E60622C9eD84823a,
            0x2978527d5D1372C32fEdC182FDE7559c0471d051,
            true,
            tryGetVersion(address(new L1StandardBridge()))
        );

        // L1ERC721Bridge (No args)
        addEntry(
            "L1ERC721Bridge",
            0x85d37236f063C687d056b3604CBEe4B60d124858,
            0xFbd06fCb2a023d89a7ae9BeE89d157C5264cf42b,
            true,
            tryGetVersion(address(new L1ERC721Bridge()))
        );

        // L1CrossDomainMessenger (No args)
        addEntry(
            "L1CrossDomainMessenger",
            0xF94B553F3602a03931e5D10CaB343C0968D793e3,
            0xb686F13AfF1e427a1f993F29ab0F2E7383729FE0,
            true,
            tryGetVersion(address(new L1CrossDomainMessenger()))
        );

        // DelayedWETH (delay)
        addEntry(
            "DelayedWETH",
            0x1B8A252A71bC8997d3871aF420895B5845212fC6,
            0x33Dadc2d1aA9BB613A7AE6B28425eA00D44c6998,
            true,
            tryGetVersion(address(new DelayedWETH(1 days)))
        );

        // ETHLockbox (No args)
        addEntry(
            "ETHLockbox",
            0xE7518EC4128B171521522119783EBfBf97c1464c,
            0x784d2F03593A42A6E4676A012762F18775ecbBe6,
            true,
            tryGetVersion(address(new ETHLockbox()))
        );

        // PreimageOracle (minProposalSize, challengePeriod)
        addEntry(
            "PreimageOracle",
            address(0),
            0x1fb8cdFc6831fc866Ed9C51aF8817Da5c287aDD3,
            true,
            tryGetVersion(address(new PreimageOracle(100, 100)))
        );

        // MIPS64 (Constructor: oracle, stateVersion)
        addEntry(
            "MIPS",
            address(0),
            0x305D1C0EED9a0291686f3BfDf1F5E54aaeeF80e4,
            true,
            tryGetVersion(address(new MIPS64(IPreimageOracle(address(new PreimageOracle(100, 100))), 7)))
        );

        // OptimismMintableERC20Factory (No args)
        addEntry(
            "OptimismMintableERC20Factory",
            0x62e1Aaeba9A8AA4654980653dB4B21FC82C61c15,
            0x8ee6fB13c6c9a7e401531168E196Fbf8b05cEabB,
            true,
            tryGetVersion(address(new OptimismMintableERC20Factory()))
        );

        // AddressManager (No args)
        addEntry("AddressManager", address(0), 0xE88CfA9D4a4fae1413914baD9796A72D13d035b9, false, "N/A");

        // AnchorStateRegistry
        addEntry(
            "AnchorStateRegistry",
            0x000590BB65ab1864a7AD46d6B957cC9a4F2C149d,
            0xeb69cC681E8D4a557b30DFFBAd85aFfD47a2CF2E,
            true,
            tryGetVersion(address(new AnchorStateRegistry(100)))
        );

        // OptimismPortalInterop
        addEntry("OptimismPortalInterop", 0x5cb365A10e99335d8feDFA225AaC5E21287302Dd, address(0), true, "SKIPPED"); // Complex
            // args

        // OPCM contracts
        addEntry("Opcm", address(0), 0xf40d4Efc33ed52AAD1638Eee8aC6393408A4FE63, true, "SKIPPED");
        addEntry("OpcmContractsContainer", address(0), 0x24756E266DC921f37939ec97299e6b5C18471689, false, "N/A");
        addEntry("OpcmGameTypeAdder", address(0), 0xbEa9D7d6C27BCdD57B85B249Fdfd5BF816852339, false, "N/A");
        addEntry("OpcmDeployer", address(0), 0x8E3Ebbfe5d46020Ef17592C32750eDBfFE011718, true, "SKIPPED");
        addEntry("OpcmUpgrader", address(0), 0x3f350B37A2cf02e2143d2644916037A86cf9940c, true, "SKIPPED");
        addEntry("OpcmInteropMigrator", address(0), 0x802067B4c2fb199988F4582A358e863EAF8dE50C, true, "SKIPPED");
        addEntry("OpcmStandardValidator", address(0), 0x15ee9632b1bAF925902b5b5B1074D3eb5aa81Fc0, true, "SKIPPED");
    }

    function tryGetVersion(address impl) internal returns (string memory) {
        try ISemver(impl).version() returns (string memory v) {
            return v;
        } catch {
            return "FAILED";
        }
    }

    function addEntry(
        string memory name,
        address proxy,
        address impl,
        bool hasSemver,
        string memory localVersion
    )
        internal
    {
        entries.push(ContractEntry(name, proxy, impl, hasSemver, localVersion));
    }

    function run() public {
        console.log("Starting Contract Checks (Proxy and Version Consistency)...");

        for (uint256 i = 0; i < entries.length; i++) {
            ContractEntry memory entry = entries[i];
            console.log("---------------------------------------------------");
            console.log("Checking:", entry.name);

            if (entry.impl == address(0) && entry.proxy == address(0)) {
                console.log("  [SKIP] No addresses provided");
                continue;
            }

            // 1. Proxy Check
            if (entry.proxy != address(0) && entry.impl != address(0)) {
                address currentImpl;

                bytes32 implSlot = vm.load(entry.proxy, IMPLEMENTATION_SLOT);
                currentImpl = address(uint160(uint256(implSlot)));

                if (currentImpl == address(0)) {
                    bytes32 addrManagerSlot = keccak256(abi.encode(address(entry.proxy), 1));
                    address addrManager = address(uint160(uint256(vm.load(entry.proxy, addrManagerSlot))));
                    if (addrManager != address(0)) {
                        console.log("  [INFO] Standard Proxy Slot empty. Found Legacy AddressManager at:", addrManager);
                    }
                }

                if (currentImpl == entry.impl) {
                    console.log("  [PASS] Proxy points to Implementation");
                } else if (currentImpl != address(0)) {
                    console.log("  [FAIL] Proxy mismatch!");
                    console.log("    Expected:", entry.impl);
                    console.log("    Actual:  ", currentImpl);
                }
            }

            if (entry.impl == address(0)) continue;

            // 2. Version Check
            if (entry.hasSemver) {
                string memory onchainVersion = "UNKNOWN";
                try ISemver(entry.impl).version() returns (string memory v) {
                    onchainVersion = v;
                } catch {
                    console.log("  [WARN] Failed to get onchain version");
                }

                if (keccak256(bytes(onchainVersion)) == keccak256(bytes(entry.localVersion))) {
                    console.log("  [PASS] Version Match:", onchainVersion);
                } else {
                    console.log("  [WARN] Version Mismatch");
                    console.log("    Onchain:", onchainVersion);
                    console.log("    Local:  ", entry.localVersion);
                }
            } else {
                console.log("  [INFO] No Semver for this contract");
            }
        }
        console.log("---------------------------------------------------");
        console.log("Checks Completed.");
    }
}
