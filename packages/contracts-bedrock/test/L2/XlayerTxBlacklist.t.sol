// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import { Test } from "forge-std/Test.sol";
import { TxBlacklistTestable } from "src/L2/XlayerTxBlacklist.sol";

contract XlayerTxBlacklistTest is Test {
    TxBlacklistTestable internal bl;

    address internal operator = makeAddr("operator");
    address internal stranger = makeAddr("stranger");

    bytes32 internal constant H1 = bytes32(uint256(1));
    bytes32 internal constant H2 = bytes32(uint256(2));
    bytes32 internal constant ZERO = bytes32(0);

    event OperatorSet(address indexed operator, bool allowed);
    event Blacklisted(bytes32 indexed hash);
    event RemovedFromBlacklist(bytes32 indexed hash);

    function setUp() public {
        bl = new TxBlacklistTestable();
    }

    // --- selector guard: exec layer depends on 0x6b623bbe ---
    function testIsBlacklistedSelectorIsStable() public pure {
        assertEq(TxBlacklistTestable.isBlacklisted.selector, bytes4(0x6b623bbe));
    }

    // --- operator management is permissionless (test-only) ---
    function testSetOperatorPermissionlessAndEmits() public {
        vm.expectEmit(true, false, false, true);
        emit OperatorSet(operator, true);
        vm.prank(stranger); // anyone may set operators
        bl.setOperator(operator, true);
        assertTrue(bl.operators(operator));
    }

    function testSetOperatorsBatch() public {
        address[] memory ops = new address[](2);
        ops[0] = operator;
        ops[1] = stranger;
        bl.setOperators(ops, true);
        assertTrue(bl.operators(operator));
        assertTrue(bl.operators(stranger));
    }

    // --- add/remove gated on operator ---
    function testAddToBlacklistRevertsForNonOperator() public {
        vm.expectRevert(abi.encodeWithSelector(TxBlacklistTestable.NotOperator.selector, stranger));
        vm.prank(stranger);
        bl.addToBlacklist(H1);
    }

    function testAddAndIsBlacklistedAndCount() public {
        bl.setOperator(operator, true);
        vm.expectEmit(true, false, false, false);
        emit Blacklisted(H1);
        vm.prank(operator);
        bl.addToBlacklist(H1);
        assertTrue(bl.isBlacklisted(H1));
        assertEq(bl.blacklistCount(), 1);
    }

    function testAddIsIdempotentForCount() public {
        bl.setOperator(operator, true);
        vm.startPrank(operator);
        bl.addToBlacklist(H1);
        bl.addToBlacklist(H1); // second add must not double-count
        vm.stopPrank();
        assertEq(bl.blacklistCount(), 1);
    }

    function testAddRejectsZeroHash() public {
        bl.setOperator(operator, true);
        vm.expectRevert(TxBlacklistTestable.ZeroHash.selector);
        vm.prank(operator);
        bl.addToBlacklist(ZERO);
    }

    function testRemoveFromBlacklist() public {
        bl.setOperator(operator, true);
        vm.startPrank(operator);
        bl.addToBlacklist(H1);
        vm.expectEmit(true, false, false, false);
        emit RemovedFromBlacklist(H1);
        bl.removeFromBlacklist(H1);
        vm.stopPrank();
        assertFalse(bl.isBlacklisted(H1));
        assertEq(bl.blacklistCount(), 0);
    }

    function testBatchAddAndAreBlacklisted() public {
        bl.setOperator(operator, true);
        bytes32[] memory hs = new bytes32[](2);
        hs[0] = H1;
        hs[1] = H2;
        vm.prank(operator);
        bl.batchAddToBlacklist(hs);
        bool[] memory got = bl.areBlacklisted(hs);
        assertTrue(got[0]);
        assertTrue(got[1]);
        assertEq(bl.blacklistCount(), 2);
    }

    function testBatchRemove() public {
        bl.setOperator(operator, true);
        bytes32[] memory hs = new bytes32[](2);
        hs[0] = H1;
        hs[1] = H2;
        vm.startPrank(operator);
        bl.batchAddToBlacklist(hs);
        bl.batchRemoveFromBlacklist(hs);
        vm.stopPrank();
        assertEq(bl.blacklistCount(), 0);
    }

    function testBatchRejectsOversize() public {
        bl.setOperator(operator, true);
        uint256 tooBig = bl.MAX_BATCH_SIZE() + 1;
        bytes32[] memory hs = new bytes32[](tooBig);
        vm.expectRevert(abi.encodeWithSelector(TxBlacklistTestable.BatchTooLarge.selector, tooBig));
        vm.prank(operator);
        bl.batchAddToBlacklist(hs);
    }

    function testMaxBatchSizeConstant() public view {
        assertEq(bl.MAX_BATCH_SIZE(), 500);
    }
}
