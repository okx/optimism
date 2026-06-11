// SPDX-License-Identifier: MIT
pragma solidity 0.8.15;

// Testing
import { CommonTest } from "test/setup/CommonTest.sol";

// Libraries
import { Predeploys } from "src/libraries/Predeploys.sol";

// Contracts
import { GaslessWhitelist } from "src/L2/XlayerGaslessWhitelist.sol";

/// @title GaslessWhitelist_Test
/// @notice Tests for the GaslessWhitelist predeploy.
contract GaslessWhitelist_Test is CommonTest {
    GaslessWhitelist internal whitelist;

    bytes4 internal constant TRANSFER_SELECTOR = 0xa9059cbb;
    bytes4 internal constant TRANSFER_FROM_SELECTOR = 0x23b872dd;
    bytes4 internal constant APPROVE_SELECTOR = 0x095ea7b3;
    uint64 internal constant FULL_GAS_LIMIT = 300_000;
    uint64 internal constant TRANSFER_GAS_LIMIT = 80_000;
    uint64 internal constant TRANSFER_FROM_GAS_LIMIT = 95_000;
    uint64 internal constant APPROVE_GAS_LIMIT = 65_000;

    address internal owner;
    address internal nonOwner = makeAddr("nonOwner");
    address internal router = makeAddr("router");
    address internal token = makeAddr("token");
    address internal tokenTwo = makeAddr("tokenTwo");
    address internal spender = makeAddr("spender");
    address internal spenderTwo = makeAddr("spenderTwo");

    event FullyGaslessTargetSet(address indexed target, bool allowed, uint64 gasLimit);
    event GaslessTransferTokenSet(address indexed token, bool allowed, uint64 gasLimit);
    event GaslessTransferFromTokenSet(address indexed token, bool allowed, uint64 gasLimit);
    event ApproveSpenderSet(address indexed token, address indexed spender, bool allowed, uint64 gasLimit);
    event GaslessEnabledSet(bool enabled);

    function setUp() public override {
        super.setUp();
        whitelist = GaslessWhitelist(Predeploys.GASLESS_WHITELIST);
        owner = whitelist.owner();
    }

    function test_initialize_succeeds() public view {
        assertEq(address(whitelist), Predeploys.GASLESS_WHITELIST);
        assertEq(owner, deploy.cfg().finalSystemOwner());
        assertFalse(whitelist.gaslessEnabled());
    }

    function test_initialize_cannotBeCalledAgain_reverts() public {
        vm.prank(owner);
        vm.expectRevert("Initializable: contract is already initialized");
        whitelist.initialize(owner);
    }

    function test_defaultDisabledBlocksConfiguredRules_succeeds() public {
        vm.startPrank(owner);
        whitelist.setFullyGaslessTarget(router, true, FULL_GAS_LIMIT);
        whitelist.setGaslessTransferToken(token, true, TRANSFER_GAS_LIMIT);
        whitelist.setApproveSpender(token, spender, true, APPROVE_GAS_LIMIT);
        vm.stopPrank();

        bytes memory transferPrefix = abi.encodePacked(TRANSFER_SELECTOR);
        bytes memory approvePrefix = abi.encodeWithSelector(APPROVE_SELECTOR, spender, uint256(100));

        _assertAllowance(router, abi.encodePacked(bytes4(0x12345678)), false, 0);
        _assertAllowance(token, transferPrefix, false, 0);
        _assertAllowance(token, _prefix(approvePrefix, 36), false, 0);
    }

    function test_fullyGaslessTargetAllowsAnySelectorWithConfiguredGasLimit_succeeds() public {
        vm.startPrank(owner);
        whitelist.setGaslessEnabled(true);
        whitelist.setFullyGaslessTarget(router, true, FULL_GAS_LIMIT);
        vm.stopPrank();

        _assertAllowance(router, abi.encodePacked(bytes4(0x12345678)), true, FULL_GAS_LIMIT);
        _assertAllowance(router, hex"123456", false, 0);
    }

    function test_transferRuleAllowsOnlyConfiguredTokenAndSelector_succeeds() public {
        vm.startPrank(owner);
        whitelist.setGaslessEnabled(true);
        whitelist.setGaslessTransferToken(token, true, TRANSFER_GAS_LIMIT);
        vm.stopPrank();

        bytes memory transferPrefix = abi.encodeWithSelector(TRANSFER_SELECTOR, address(0xBEEF), uint256(100));
        bytes memory approvePrefix = abi.encodeWithSelector(APPROVE_SELECTOR, spender, uint256(100));

        _assertAllowance(token, _prefix(transferPrefix, 36), true, TRANSFER_GAS_LIMIT);
        _assertAllowance(tokenTwo, _prefix(transferPrefix, 36), false, 0);
        _assertAllowance(token, _prefix(approvePrefix, 36), false, 0);
        _assertAllowance(token, abi.encodePacked(TRANSFER_SELECTOR), true, TRANSFER_GAS_LIMIT);
    }

    function test_transferFromRuleAllowsOnlyConfiguredTokenWithItsOwnGasLimit_succeeds() public {
        vm.startPrank(owner);
        whitelist.setGaslessEnabled(true);
        whitelist.setGaslessTransferFromToken(token, true, TRANSFER_FROM_GAS_LIMIT);
        vm.stopPrank();

        bytes memory transferFromPrefix =
            abi.encodeWithSelector(TRANSFER_FROM_SELECTOR, address(0xBEEF), address(0xCAFE), uint256(100));

        _assertAllowance(token, _prefix(transferFromPrefix, 68), true, TRANSFER_FROM_GAS_LIMIT);
        _assertAllowance(tokenTwo, _prefix(transferFromPrefix, 68), false, 0);
        _assertAllowance(token, abi.encodePacked(TRANSFER_SELECTOR), false, 0);
    }

    function test_approveRuleParsesSpenderFromDataPrefix_succeeds() public {
        vm.startPrank(owner);
        whitelist.setGaslessEnabled(true);
        whitelist.setApproveSpender(token, spender, true, APPROVE_GAS_LIMIT);
        vm.stopPrank();

        bytes memory allowedPrefix = abi.encodeWithSelector(APPROVE_SELECTOR, spender, uint256(100));
        bytes memory disallowedPrefix = abi.encodeWithSelector(APPROVE_SELECTOR, spenderTwo, uint256(100));
        bytes memory revokePrefix = abi.encodeWithSelector(APPROVE_SELECTOR, spender, uint256(0));

        _assertAllowance(token, _prefix(allowedPrefix, 36), true, APPROVE_GAS_LIMIT);
        _assertAllowance(token, _prefix(disallowedPrefix, 36), false, 0);
        _assertAllowance(token, _prefix(revokePrefix, 36), true, APPROVE_GAS_LIMIT);
        _assertAllowance(token, _prefix(allowedPrefix, 35), false, 0);
    }

    function test_batchSettersAddAndRemoveRules_succeeds() public {
        GaslessWhitelist.FullyGaslessTargetConfig[] memory targetConfigs =
            new GaslessWhitelist.FullyGaslessTargetConfig[](1);
        targetConfigs[0] = GaslessWhitelist.FullyGaslessTargetConfig(router, true, FULL_GAS_LIMIT);

        GaslessWhitelist.GaslessTransferTokenConfig[] memory tokenConfigs =
            new GaslessWhitelist.GaslessTransferTokenConfig[](1);
        tokenConfigs[0] = GaslessWhitelist.GaslessTransferTokenConfig(token, true, TRANSFER_GAS_LIMIT);

        GaslessWhitelist.ApproveSpenderConfig[] memory approveConfigs = new GaslessWhitelist.ApproveSpenderConfig[](1);
        approveConfigs[0] = GaslessWhitelist.ApproveSpenderConfig(token, spender, true, APPROVE_GAS_LIMIT);

        vm.startPrank(owner);
        whitelist.batchSetFullyGaslessTargets(targetConfigs);
        whitelist.batchSetGaslessTransferTokens(tokenConfigs);
        whitelist.batchSetApproveSpenders(approveConfigs);
        vm.stopPrank();

        _assertFullyGaslessTarget(router, true, FULL_GAS_LIMIT);
        _assertTransferToken(token, true, TRANSFER_GAS_LIMIT);
        _assertApproveSpender(token, spender, true, APPROVE_GAS_LIMIT);

        targetConfigs[0] = GaslessWhitelist.FullyGaslessTargetConfig(router, false, 0);
        tokenConfigs[0] = GaslessWhitelist.GaslessTransferTokenConfig(token, false, 0);
        approveConfigs[0] = GaslessWhitelist.ApproveSpenderConfig(token, spender, false, 0);

        vm.startPrank(owner);
        whitelist.batchSetFullyGaslessTargets(targetConfigs);
        whitelist.batchSetGaslessTransferTokens(tokenConfigs);
        whitelist.batchSetApproveSpenders(approveConfigs);
        vm.stopPrank();

        _assertFullyGaslessTarget(router, false, 0);
        _assertTransferToken(token, false, 0);
        _assertApproveSpender(token, spender, false, 0);
    }

    function test_settersEmitEvents_succeeds() public {
        vm.startPrank(owner);

        vm.expectEmit(true, false, false, true);
        emit FullyGaslessTargetSet(router, true, FULL_GAS_LIMIT);
        whitelist.setFullyGaslessTarget(router, true, FULL_GAS_LIMIT);

        vm.expectEmit(true, false, false, true);
        emit GaslessTransferTokenSet(token, true, TRANSFER_GAS_LIMIT);
        whitelist.setGaslessTransferToken(token, true, TRANSFER_GAS_LIMIT);

        vm.expectEmit(true, false, false, true);
        emit GaslessTransferFromTokenSet(token, true, TRANSFER_FROM_GAS_LIMIT);
        whitelist.setGaslessTransferFromToken(token, true, TRANSFER_FROM_GAS_LIMIT);

        vm.expectEmit(true, true, false, true);
        emit ApproveSpenderSet(token, spender, true, APPROVE_GAS_LIMIT);
        whitelist.setApproveSpender(token, spender, true, APPROVE_GAS_LIMIT);

        vm.expectEmit(false, false, false, true);
        emit GaslessEnabledSet(true);
        whitelist.setGaslessEnabled(true);

        vm.stopPrank();
    }

    function test_onlyOwnerCanConfigure_reverts() public {
        vm.startPrank(nonOwner);

        vm.expectRevert("Ownable: caller is not the owner");
        whitelist.setFullyGaslessTarget(router, true, FULL_GAS_LIMIT);

        vm.expectRevert("Ownable: caller is not the owner");
        whitelist.setGaslessTransferToken(token, true, TRANSFER_GAS_LIMIT);

        vm.expectRevert("Ownable: caller is not the owner");
        whitelist.setGaslessTransferFromToken(token, true, TRANSFER_FROM_GAS_LIMIT);

        vm.expectRevert("Ownable: caller is not the owner");
        whitelist.setApproveSpender(token, spender, true, APPROVE_GAS_LIMIT);

        vm.expectRevert("Ownable: caller is not the owner");
        whitelist.setGaslessEnabled(true);

        vm.stopPrank();
    }

    function test_zeroAddressConfiguration_reverts() public {
        vm.startPrank(owner);

        vm.expectRevert(GaslessWhitelist.ZeroAddress.selector);
        whitelist.setFullyGaslessTarget(address(0), true, FULL_GAS_LIMIT);

        vm.expectRevert(GaslessWhitelist.ZeroAddress.selector);
        whitelist.setGaslessTransferToken(address(0), true, TRANSFER_GAS_LIMIT);

        vm.expectRevert(GaslessWhitelist.ZeroAddress.selector);
        whitelist.setGaslessTransferFromToken(address(0), true, TRANSFER_FROM_GAS_LIMIT);

        vm.expectRevert(GaslessWhitelist.ZeroAddress.selector);
        whitelist.setApproveSpender(token, address(0), true, APPROVE_GAS_LIMIT);

        vm.stopPrank();
    }

    function test_enablingRuleWithZeroGasLimit_reverts() public {
        vm.startPrank(owner);

        vm.expectRevert(GaslessWhitelist.InvalidGasLimit.selector);
        whitelist.setFullyGaslessTarget(router, true, 0);

        vm.expectRevert(GaslessWhitelist.InvalidGasLimit.selector);
        whitelist.setGaslessTransferToken(token, true, 0);

        vm.expectRevert(GaslessWhitelist.InvalidGasLimit.selector);
        whitelist.setGaslessTransferFromToken(token, true, 0);

        vm.expectRevert(GaslessWhitelist.InvalidGasLimit.selector);
        whitelist.setApproveSpender(token, spender, true, 0);

        vm.stopPrank();
    }

    function _assertAllowance(
        address to,
        bytes memory dataPrefix,
        bool expectedAllowed,
        uint64 expectedGasLimit
    )
        internal
        view
    {
        (bool allowed, uint64 gasLimit) = whitelist.getGaslessAllowance(to, dataPrefix);
        assertEq(allowed, expectedAllowed);
        assertEq(gasLimit, expectedGasLimit);
    }

    function _assertFullyGaslessTarget(address target, bool expectedAllowed, uint64 expectedGasLimit) internal view {
        (bool allowed, uint64 gasLimit) = whitelist.fullyGaslessTargets(target);
        assertEq(allowed, expectedAllowed);
        assertEq(gasLimit, expectedGasLimit);
    }

    function _assertTransferToken(address target, bool expectedAllowed, uint64 expectedGasLimit) internal view {
        (bool allowed, uint64 gasLimit) = whitelist.gaslessTransferTokens(target);
        assertEq(allowed, expectedAllowed);
        assertEq(gasLimit, expectedGasLimit);
    }

    function _assertApproveSpender(
        address target,
        address approvedSpender,
        bool expectedAllowed,
        uint64 expectedGasLimit
    )
        internal
        view
    {
        (bool allowed, uint64 gasLimit) = whitelist.approveSpendersByToken(target, approvedSpender);
        assertEq(allowed, expectedAllowed);
        assertEq(gasLimit, expectedGasLimit);
    }

    function _prefix(bytes memory data, uint256 length) internal pure returns (bytes memory result) {
        result = new bytes(length);
        for (uint256 i; i < length; ++i) {
            result[i] = data[i];
        }
    }
}
