// SPDX-License-Identifier: MIT
pragma solidity ^0.8.15;

import {IRiscZeroVerifier} from "interfaces/dispute/IRiscZeroVerifier.sol";

contract MockRiscZeroVerifier is IRiscZeroVerifier {
    bool public shouldRevert;
    bytes public lastSeal;
    bytes32 public lastImageId;
    bytes32 public lastJournalDigest;

    function setShouldRevert(bool value) external {
        shouldRevert = value;
    }

    function verify(bytes calldata seal, bytes32 imageId, bytes32 journalDigest) external view {
        if (shouldRevert) revert("MockRiscZeroVerifier: invalid proof");
        seal;
        imageId;
        journalDigest;
    }

    function verifyAndRecord(bytes calldata seal, bytes32 imageId, bytes32 journalDigest) external {
        if (shouldRevert) revert("MockRiscZeroVerifier: invalid proof");
        lastSeal = seal;
        lastImageId = imageId;
        lastJournalDigest = journalDigest;
    }
}
