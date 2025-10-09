// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import { Script } from "forge-std/Script.sol";
import { console2 as console } from "forge-std/console2.sol";

import { IProxyAdmin } from "interfaces/universal/IProxyAdmin.sol";
import { Predeploys } from "src/libraries/Predeploys.sol";

/// @title TransferProxyAdminL2
/// @notice Script to transfer L2 ProxyAdmin ownership to an aliased L1 address.
///         This script should be run AFTER TransferProxyAdminL1.s.sol has successfully
///         deployed the ProxyAdminOwnerSafe on L1 and transferred L1 ProxyAdmin ownership.
///
/// @dev Usage:
///      PROXY_ADMIN_OWNER_SAFE=<l1_safe_address> \
///      forge script scripts/transfer-owner/TransferProxyAdminL2.s.sol:TransferProxyAdminL2 \
///        --rpc-url $L2_RPC_URL \
///        --private-key $DEPLOYER_PRIVATE_KEY \
///        --broadcast
///
///      The L2 ProxyAdmin will be transferred to the ALIASED version of the L1 ProxyAdminOwnerSafe.
///      This allows the L1 Safe to control L2 ProxyAdmin via cross-domain messages in the future.
contract TransferProxyAdminL2 is Script {
    // L2 ProxyAdmin is at a fixed predeploy address
    address public constant L2_PROXY_ADMIN = Predeploys.PROXY_ADMIN;

    // Aliasing offset for L2 addresses
    uint160 public constant ALIASING_OFFSET = uint160(0x1111000000000000000000000000000000001111);

    /// @notice Modifier that wraps a function in broadcasting.
    modifier broadcast() {
        vm.startBroadcast(msg.sender);
        _;
        vm.stopBroadcast();
    }

    /// @notice Main execution function.
    function run() public {
        console.log("====================================================");
        console.log("L2 ProxyAdmin Ownership Transfer");
        console.log("====================================================\n");

        // Read the L1 ProxyAdminOwnerSafe address from environment
        address l1ProxyAdminOwnerSafe = vm.envAddress("PROXY_ADMIN_OWNER_SAFE");

        console.log("Configuration:");
        console.log("  L2 ProxyAdmin:                %s", L2_PROXY_ADMIN);
        console.log("  L1 ProxyAdminOwnerSafe:       %s", l1ProxyAdminOwnerSafe);
        console.log();

        // Transfer L2 ProxyAdmin ownership
        transferL2ProxyAdminOwnership(l1ProxyAdminOwnerSafe);

        console.log("====================================================");
        console.log("L2 ProxyAdmin Ownership Transfer Complete");
        console.log("====================================================\n");
    }

    /// @notice Compute the aliased address for L2
    /// @param _l1Address The L1 address to alias
    /// @return The aliased L2 address
    function computeAliasedAddress(address _l1Address) internal pure returns (address) {
        return address(uint160(_l1Address) + ALIASING_OFFSET);
    }

    /// @notice Transfer L2 ProxyAdmin ownership to the aliased L1 ProxyAdminOwnerSafe.
    /// @dev The L2 ProxyAdmin owner is NOT aliased during genesis - it's the same address as the deployer.
    ///      Since we have the private key (deployer), we transfer directly on L2.
    ///      The new owner will be the aliased ProxyAdminOwnerSafe address, allowing future
    ///      cross-domain control from L1.
    /// @param _l1ProxyAdminOwner The L1 ProxyAdminOwnerSafe address (will be aliased on L2 to become new owner).
    function transferL2ProxyAdminOwnership(address _l1ProxyAdminOwner) internal broadcast {
        console.log("Step 1: Checking current L2 ProxyAdmin ownership...");

        IProxyAdmin l2ProxyAdmin = IProxyAdmin(L2_PROXY_ADMIN);
        address currentL2Owner = l2ProxyAdmin.owner();
        console.log("  Current L2 ProxyAdmin owner:  %s", currentL2Owner);

        // Verify the deployer is the current owner
        require(currentL2Owner == msg.sender, "TransferProxyAdminL2: msg.sender is not L2 ProxyAdmin owner");

        // Check deployer balance on L2
        uint256 deployerL2Balance = msg.sender.balance;
        console.log("  Deployer L2 balance:          %s ETH", deployerL2Balance / 1e18);

        // Require minimum balance for gas (0.001 ETH should be more than enough)
        uint256 minBalance = 0.001 ether;
        require(deployerL2Balance >= minBalance, "TransferProxyAdminL2: Insufficient L2 balance for gas fees");

        console.log();
        console.log("Step 2: Computing aliased L2 owner address...");

        // Compute the aliased L2 owner address (what the new L2 ProxyAdmin owner will be)
        address aliasedL2Owner = computeAliasedAddress(_l1ProxyAdminOwner);
        console.log("  Aliased L2 Owner (new):       %s", aliasedL2Owner);

        // Verify we're not transferring to the same address
        require(currentL2Owner != aliasedL2Owner, "TransferProxyAdminL2: already owned by target address");

        console.log();
        console.log("Step 3: Transferring L2 ProxyAdmin ownership...");

        // Transfer ownership to the aliased L1 address
        l2ProxyAdmin.transferOwnership(aliasedL2Owner);

        // Verify the transfer
        address newL2Owner = l2ProxyAdmin.owner();
        require(newL2Owner == aliasedL2Owner, "TransferProxyAdminL2: L2 ownership transfer failed");

        console.log("  L2 ProxyAdmin ownership successfully transferred!");
        console.log();
        console.log("Summary:");
        console.log("  Old owner: %s (deployer)", currentL2Owner);
        console.log("  New owner: %s (aliased L1 Safe)", newL2Owner);
        console.log();
        console.log("Notes:");
        console.log("  - The new owner is the ALIASED version of the L1 ProxyAdminOwnerSafe");
        console.log("  - Future upgrades can be controlled from L1 via cross-domain messages");
        console.log("  - The L1 Safe must send cross-domain messages to act as this aliased address");
    }
}
