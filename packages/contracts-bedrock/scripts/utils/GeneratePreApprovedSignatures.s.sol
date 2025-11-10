// SPDX-License-Identifier: MIT
pragma solidity 0.8.15;

import { Script } from "forge-std/Script.sol";
import { console2 as console } from "forge-std/console2.sol";
import { Vm } from "forge-std/Vm.sol";

/// @title GeneratePreApprovedSignatures
/// @notice Script to generate pre-approved signatures for Gnosis Safe transactions
/// @dev Usage: OWNERS=0xAddr1,0xAddr2,0xAddr3 forge script scripts/utils/GeneratePreApprovedSignatures.s.sol
contract GeneratePreApprovedSignatures is Script {
    /// @notice Build pre-approved signatures (v=1) for given owners
    /// @dev Each signature is 65 bytes: r=ownerAddress, s=0, v=1
    function _buildPreApprovedSignatures(address[] memory _owners) internal pure returns (bytes memory) {
        bytes memory signatures;
        for (uint256 i = 0; i < _owners.length; i++) {
            signatures = abi.encodePacked(
                signatures,
                bytes32(uint256(uint160(_owners[i]))), // r = owner address
                bytes32(0), // s = 0
                uint8(1) // v = 1 (pre-approved hash)
            );
        }
        return signatures;
    }

    /// @notice Parse comma-separated addresses from a string
    /// @param _input String containing comma-separated addresses
    /// @return Array of parsed addresses
    function _parseAddresses(string memory _input) internal pure returns (address[] memory) {
        bytes memory inputBytes = bytes(_input);

        // Count commas to determine array size
        uint256 count = 1;
        for (uint256 i = 0; i < inputBytes.length; i++) {
            if (inputBytes[i] == ",") {
                count++;
            }
        }

        address[] memory addresses = new address[](count);
        uint256 addressIndex = 0;
        uint256 startIndex = 0;

        for (uint256 i = 0; i <= inputBytes.length; i++) {
            if (i == inputBytes.length || inputBytes[i] == ",") {
                // Extract substring
                bytes memory addrBytes = new bytes(i - startIndex);
                for (uint256 j = 0; j < i - startIndex; j++) {
                    addrBytes[j] = inputBytes[startIndex + j];
                }

                // Trim whitespace
                addrBytes = _trim(addrBytes);

                // Parse address
                addresses[addressIndex] = _parseAddress(string(addrBytes));
                addressIndex++;
                startIndex = i + 1;
            }
        }

        return addresses;
    }

    /// @notice Trim whitespace from bytes
    function _trim(bytes memory _input) internal pure returns (bytes memory) {
        uint256 start = 0;
        uint256 end = _input.length;

        // Find first non-whitespace
        while (start < end && (_input[start] == " " || _input[start] == "\t" || _input[start] == "\n")) {
            start++;
        }

        // Find last non-whitespace
        while (end > start && (_input[end - 1] == " " || _input[end - 1] == "\t" || _input[end - 1] == "\n")) {
            end--;
        }

        bytes memory result = new bytes(end - start);
        for (uint256 i = 0; i < end - start; i++) {
            result[i] = _input[start + i];
        }

        return result;
    }

    /// @notice Parse a hex string to an address
    function _parseAddress(string memory _addr) internal pure returns (address) {
        bytes memory addrBytes = bytes(_addr);
        require(addrBytes.length == 42 || addrBytes.length == 40, "Invalid address length");

        uint256 offset = addrBytes.length == 42 ? 2 : 0; // Skip "0x" if present
        uint160 result = 0;

        for (uint256 i = offset; i < addrBytes.length; i++) {
            uint8 digit = uint8(addrBytes[i]);
            uint8 value;

            if (digit >= 48 && digit <= 57) {
                value = digit - 48; // 0-9
            } else if (digit >= 65 && digit <= 70) {
                value = digit - 55; // A-F
            } else if (digit >= 97 && digit <= 102) {
                value = digit - 87; // a-f
            } else {
                revert("Invalid hex character");
            }

            result = result * 16 + value;
        }

        return address(result);
    }

    /// @notice Sort addresses in ascending order (required by Gnosis Safe)
    /// @param _addresses Array of addresses to sort
    function _sortAddresses(address[] memory _addresses) internal pure returns (address[] memory) {
        uint256 n = _addresses.length;

        // Bubble sort (simple implementation for small arrays)
        for (uint256 i = 0; i < n - 1; i++) {
            for (uint256 j = 0; j < n - i - 1; j++) {
                if (uint160(_addresses[j]) > uint160(_addresses[j + 1])) {
                    // Swap
                    address temp = _addresses[j];
                    _addresses[j] = _addresses[j + 1];
                    _addresses[j + 1] = temp;
                }
            }
        }

        return _addresses;
    }

    /// @notice Main script entry point
    function run() external view {
        // Read OWNERS environment variable
        string memory ownersEnv = vm.envString("OWNERS");
        require(bytes(ownersEnv).length > 0, "OWNERS environment variable not set");

        // Parse addresses
        address[] memory owners = _parseAddresses(ownersEnv);
        // Sort addresses (required by Gnosis Safe!)
        address[] memory sortedOwners = _sortAddresses(owners);
        console.log("Sorted owners (ascending order):");
        for (uint256 i = 0; i < sortedOwners.length; i++) {
            console.log("  [%d] %s", i, sortedOwners[i]);
        }
        console.log("");

        // Generate signatures
        bytes memory signatures = _buildPreApprovedSignatures(sortedOwners);

        console.log("Generated Pre-Approved Signatures:");
        console.log("----------------------------------");
        console.logBytes(signatures);
        console.log("");
    }
}
