// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import { Test } from "forge-std/Test.sol";
import { TransparentUpgradeableProxy } from
    "@openzeppelin/contracts-v5/proxy/transparent/TransparentUpgradeableProxy.sol";
import { OwnableUpgradeable } from "@openzeppelin/contracts-upgradeable-v5/access/OwnableUpgradeable.sol";
import { GaslessWhitelist } from "src/L2/XlayerGaslessWhitelistLatest.sol";

contract GaslessWhitelistTest is Test {
    GaslessWhitelist internal implementation;
    GaslessWhitelist internal whitelist;

    bytes4 internal constant TRANSFER_SELECTOR = 0xa9059cbb;
    bytes4 internal constant TRANSFER_FROM_SELECTOR = 0x23b872dd;
    bytes4 internal constant APPROVE_SELECTOR = 0x095ea7b3;
    uint64 internal constant FULL_GAS_LIMIT = 300_000;
    uint64 internal constant TRANSFER_GAS_LIMIT = 80_000;
    uint64 internal constant TRANSFER_FROM_GAS_LIMIT = 95_000;
    uint64 internal constant APPROVE_GAS_LIMIT = 65_000;

    address internal owner = makeAddr("owner");
    address internal proxyAdminOwner = makeAddr("proxyAdminOwner");
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
    event MaxGasLimitSet(uint64 maxGasLimit);

    function setUp() public {
        implementation = new GaslessWhitelist();
        bytes memory initData = abi.encodeCall(GaslessWhitelist.initialize, (owner));
        TransparentUpgradeableProxy proxy =
            new TransparentUpgradeableProxy(address(implementation), proxyAdminOwner, initData);
        whitelist = GaslessWhitelist(address(proxy));
    }

    function testInitializeThroughProxy() public view {
        assertEq(whitelist.owner(), owner);
        assertFalse(whitelist.gaslessEnabled());
        assertEq(whitelist.maxGasLimit(), 16_777_216);
    }

    function testImplementationCannotBeInitialized() public {
        vm.expectRevert();
        implementation.initialize(owner);
    }

    function testInitializeRejectsZeroOwner() public {
        GaslessWhitelist newImplementation = new GaslessWhitelist();
        bytes memory initData = abi.encodeCall(GaslessWhitelist.initialize, (address(0)));

        vm.expectRevert(abi.encodeWithSelector(OwnableUpgradeable.OwnableInvalidOwner.selector, address(0)));
        new TransparentUpgradeableProxy(address(newImplementation), proxyAdminOwner, initData);
    }

    function testDefaultDisabledBlocksConfiguredRules() public {
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

    function testZeroTargetIsNeverAllowed() public {
        vm.startPrank(owner);
        whitelist.setGaslessEnabled(true);
        whitelist.setGaslessTransferToken(token, true, TRANSFER_GAS_LIMIT);
        vm.stopPrank();

        _assertAllowance(address(0), abi.encodePacked(TRANSFER_SELECTOR), false, 0);
    }

    function testFullyGaslessTargetAllowsAnySelectorWithConfiguredGasLimit() public {
        vm.startPrank(owner);
        whitelist.setGaslessEnabled(true);
        whitelist.setFullyGaslessTarget(router, true, FULL_GAS_LIMIT);
        vm.stopPrank();

        bytes memory dataPrefix = abi.encodePacked(bytes4(0x12345678));
        _assertAllowance(router, dataPrefix, true, FULL_GAS_LIMIT);
    }

    function testFullyGaslessTargetStillRequiresSelectorPrefix() public {
        vm.startPrank(owner);
        whitelist.setGaslessEnabled(true);
        whitelist.setFullyGaslessTarget(router, true, FULL_GAS_LIMIT);
        vm.stopPrank();

        _assertAllowance(router, hex"123456", false, 0);
    }

    function testSingleSettersCanRemoveRules() public {
        vm.startPrank(owner);
        whitelist.setGaslessEnabled(true);
        whitelist.setFullyGaslessTarget(router, true, FULL_GAS_LIMIT);
        whitelist.setGaslessTransferToken(token, true, TRANSFER_GAS_LIMIT);
        whitelist.setApproveSpender(token, spender, true, APPROVE_GAS_LIMIT);
        whitelist.setFullyGaslessTarget(router, false, 0);
        whitelist.setGaslessTransferToken(token, false, 0);
        whitelist.setApproveSpender(token, spender, false, 0);
        vm.stopPrank();

        bytes memory approvePrefix = abi.encodeWithSelector(APPROVE_SELECTOR, spender, uint256(100));

        _assertAllowance(router, abi.encodePacked(bytes4(0x12345678)), false, 0);
        _assertAllowance(token, abi.encodePacked(TRANSFER_SELECTOR), false, 0);
        _assertAllowance(token, _prefix(approvePrefix, 36), false, 0);
    }

    function testTransferRuleAllowsOnlyConfiguredTokenAndSelector() public {
        vm.startPrank(owner);
        whitelist.setGaslessEnabled(true);
        whitelist.setGaslessTransferToken(token, true, TRANSFER_GAS_LIMIT);
        vm.stopPrank();

        bytes memory transferPrefix = abi.encodeWithSelector(TRANSFER_SELECTOR, address(0xBEEF), uint256(100));
        bytes memory approvePrefix = abi.encodeWithSelector(APPROVE_SELECTOR, spender, uint256(100));

        _assertAllowance(token, _prefix(transferPrefix, 36), true, TRANSFER_GAS_LIMIT);
        _assertAllowance(tokenTwo, _prefix(transferPrefix, 36), false, 0);
        _assertAllowance(token, _prefix(approvePrefix, 36), false, 0);
    }

    function testTransferRuleOnlyRequiresSelectorPrefix() public {
        vm.startPrank(owner);
        whitelist.setGaslessEnabled(true);
        whitelist.setGaslessTransferToken(token, true, TRANSFER_GAS_LIMIT);
        vm.stopPrank();

        _assertAllowance(token, abi.encodePacked(TRANSFER_SELECTOR), true, TRANSFER_GAS_LIMIT);
    }

    function testTransferFromRuleAllowsOnlyConfiguredTokenWithItsOwnGasLimit() public {
        vm.startPrank(owner);
        whitelist.setGaslessEnabled(true);
        whitelist.setGaslessTransferFromToken(token, true, TRANSFER_FROM_GAS_LIMIT);
        vm.stopPrank();

        bytes memory transferFromPrefix =
            abi.encodeWithSelector(TRANSFER_FROM_SELECTOR, address(0xBEEF), address(0xCAFE), uint256(100));

        _assertAllowance(token, _prefix(transferFromPrefix, 68), true, TRANSFER_FROM_GAS_LIMIT);
        _assertAllowance(tokenTwo, _prefix(transferFromPrefix, 68), false, 0);
    }

    function testTransferFromRuleOnlyRequiresSelectorPrefix() public {
        vm.startPrank(owner);
        whitelist.setGaslessEnabled(true);
        whitelist.setGaslessTransferFromToken(token, true, TRANSFER_FROM_GAS_LIMIT);
        vm.stopPrank();

        _assertAllowance(token, abi.encodePacked(TRANSFER_FROM_SELECTOR), true, TRANSFER_FROM_GAS_LIMIT);
    }

    function testTransferFromBlockedWhenTokenNotWhitelisted() public {
        vm.startPrank(owner);
        whitelist.setGaslessEnabled(true);
        vm.stopPrank();

        _assertAllowance(token, abi.encodePacked(TRANSFER_FROM_SELECTOR), false, 0);
    }

    /// @dev transfer and transferFrom rules are fully independent of each other.
    function testTransferAndTransferFromRulesAreIndependent() public {
        vm.startPrank(owner);
        whitelist.setGaslessEnabled(true);
        whitelist.setGaslessTransferToken(token, true, TRANSFER_GAS_LIMIT);
        whitelist.setGaslessTransferFromToken(tokenTwo, true, TRANSFER_FROM_GAS_LIMIT);
        vm.stopPrank();

        // token has only the transfer rule: transfer allowed, transferFrom blocked.
        _assertAllowance(token, abi.encodePacked(TRANSFER_SELECTOR), true, TRANSFER_GAS_LIMIT);
        _assertAllowance(token, abi.encodePacked(TRANSFER_FROM_SELECTOR), false, 0);

        // tokenTwo has only the transferFrom rule: transferFrom allowed, transfer blocked.
        _assertAllowance(tokenTwo, abi.encodePacked(TRANSFER_FROM_SELECTOR), true, TRANSFER_FROM_GAS_LIMIT);
        _assertAllowance(tokenTwo, abi.encodePacked(TRANSFER_SELECTOR), false, 0);
    }

    function testApproveRuleParsesSpenderFromDataPrefix() public {
        vm.startPrank(owner);
        whitelist.setGaslessEnabled(true);
        whitelist.setApproveSpender(token, spender, true, APPROVE_GAS_LIMIT);
        vm.stopPrank();

        bytes memory allowedPrefix = abi.encodeWithSelector(APPROVE_SELECTOR, spender, uint256(100));
        bytes memory disallowedPrefix = abi.encodeWithSelector(APPROVE_SELECTOR, spenderTwo, uint256(100));

        _assertAllowance(token, _prefix(allowedPrefix, 36), true, APPROVE_GAS_LIMIT);
        _assertAllowance(token, _prefix(disallowedPrefix, 36), false, 0);
    }

    function testApproveRuleRejectsThirtyFiveBytePrefix() public {
        vm.startPrank(owner);
        whitelist.setGaslessEnabled(true);
        whitelist.setApproveSpender(token, spender, true, APPROVE_GAS_LIMIT);
        vm.stopPrank();

        bytes memory approvePrefix = abi.encodeWithSelector(APPROVE_SELECTOR, spender, uint256(100));

        _assertAllowance(token, _prefix(approvePrefix, 35), false, 0);
    }

    function testApproveRuleRequiresSpenderPrefix() public {
        vm.startPrank(owner);
        whitelist.setGaslessEnabled(true);
        whitelist.setApproveSpender(token, spender, true, APPROVE_GAS_LIMIT);
        vm.stopPrank();

        _assertAllowance(token, abi.encodePacked(whitelist.APPROVE_SELECTOR()), false, 0);
    }

    function testApproveRuleAlsoCoversRevokeApproval() public {
        vm.startPrank(owner);
        whitelist.setGaslessEnabled(true);
        whitelist.setApproveSpender(token, spender, true, APPROVE_GAS_LIMIT);
        vm.stopPrank();

        bytes memory revokePrefix = abi.encodeWithSelector(APPROVE_SELECTOR, spender, uint256(0));

        _assertAllowance(token, _prefix(revokePrefix, 36), true, APPROVE_GAS_LIMIT);
    }

    function testGlobalSwitchDisablesAllRules() public {
        vm.startPrank(owner);
        whitelist.setGaslessEnabled(true);
        whitelist.setFullyGaslessTarget(router, true, FULL_GAS_LIMIT);
        whitelist.setGaslessEnabled(false);
        vm.stopPrank();

        _assertAllowance(router, abi.encodePacked(bytes4(0x12345678)), false, 0);
    }

    function testOwnerCanSetMaxGasLimit() public {
        vm.prank(owner);
        whitelist.setMaxGasLimit(TRANSFER_GAS_LIMIT);

        assertEq(whitelist.maxGasLimit(), TRANSFER_GAS_LIMIT);
    }

    function testConfiguredGasLimitCannotExceedMaxGasLimit() public {
        vm.startPrank(owner);
        whitelist.setMaxGasLimit(TRANSFER_GAS_LIMIT);

        vm.expectRevert(GaslessWhitelist.GasLimitExceedsMax.selector);
        whitelist.setFullyGaslessTarget(router, true, TRANSFER_GAS_LIMIT + 1);

        vm.expectRevert(GaslessWhitelist.GasLimitExceedsMax.selector);
        whitelist.setGaslessTransferToken(token, true, TRANSFER_GAS_LIMIT + 1);

        vm.expectRevert(GaslessWhitelist.GasLimitExceedsMax.selector);
        whitelist.setGaslessTransferFromToken(token, true, TRANSFER_GAS_LIMIT + 1);

        vm.expectRevert(GaslessWhitelist.GasLimitExceedsMax.selector);
        whitelist.setApproveSpender(token, spender, true, TRANSFER_GAS_LIMIT + 1);

        vm.stopPrank();
    }

    function testConfiguredGasLimitCannotExceedDefaultMaxGasLimit() public {
        vm.startPrank(owner);

        vm.expectRevert(GaslessWhitelist.GasLimitExceedsMax.selector);
        whitelist.setFullyGaslessTarget(router, true, 16_777_216 + 1);

        vm.stopPrank();
    }

    function testLoweringMaxGasLimitCapsExistingRuleReturns() public {
        vm.startPrank(owner);
        whitelist.setGaslessEnabled(true);
        whitelist.setFullyGaslessTarget(router, true, FULL_GAS_LIMIT);
        whitelist.setGaslessTransferToken(token, true, FULL_GAS_LIMIT);
        whitelist.setGaslessTransferFromToken(tokenTwo, true, FULL_GAS_LIMIT);
        whitelist.setApproveSpender(token, spender, true, FULL_GAS_LIMIT);
        whitelist.setMaxGasLimit(TRANSFER_GAS_LIMIT);
        vm.stopPrank();

        bytes memory approvePrefix = abi.encodeWithSelector(APPROVE_SELECTOR, spender, uint256(100));

        _assertAllowance(router, abi.encodePacked(bytes4(0x12345678)), true, TRANSFER_GAS_LIMIT);
        _assertAllowance(token, abi.encodePacked(TRANSFER_SELECTOR), true, TRANSFER_GAS_LIMIT);
        _assertAllowance(tokenTwo, abi.encodePacked(TRANSFER_FROM_SELECTOR), true, TRANSFER_GAS_LIMIT);
        _assertAllowance(token, _prefix(approvePrefix, 36), true, TRANSFER_GAS_LIMIT);

        _assertFullyGaslessTarget(router, true, FULL_GAS_LIMIT);
        _assertTransferToken(token, true, FULL_GAS_LIMIT);
        _assertTransferFromToken(tokenTwo, true, FULL_GAS_LIMIT);
        _assertApproveSpender(token, spender, true, FULL_GAS_LIMIT);
    }

    function testBatchSettersAddAndRemoveRules() public {
        GaslessWhitelist.FullyGaslessTargetConfig[] memory targetConfigs =
            new GaslessWhitelist.FullyGaslessTargetConfig[](2);
        targetConfigs[0] = GaslessWhitelist.FullyGaslessTargetConfig(router, true, FULL_GAS_LIMIT);
        targetConfigs[1] = GaslessWhitelist.FullyGaslessTargetConfig(makeAddr("routerTwo"), true, FULL_GAS_LIMIT + 1);

        GaslessWhitelist.GaslessTransferTokenConfig[] memory tokenConfigs =
            new GaslessWhitelist.GaslessTransferTokenConfig[](2);
        tokenConfigs[0] = GaslessWhitelist.GaslessTransferTokenConfig(token, true, TRANSFER_GAS_LIMIT);
        tokenConfigs[1] = GaslessWhitelist.GaslessTransferTokenConfig(tokenTwo, true, TRANSFER_GAS_LIMIT + 1);

        GaslessWhitelist.ApproveSpenderConfig[] memory approveConfigs = new GaslessWhitelist.ApproveSpenderConfig[](2);
        approveConfigs[0] = GaslessWhitelist.ApproveSpenderConfig(token, spender, true, APPROVE_GAS_LIMIT);
        approveConfigs[1] = GaslessWhitelist.ApproveSpenderConfig(tokenTwo, spenderTwo, true, APPROVE_GAS_LIMIT + 1);

        vm.startPrank(owner);
        whitelist.batchSetFullyGaslessTargets(targetConfigs);
        whitelist.batchSetGaslessTransferTokens(tokenConfigs);
        whitelist.batchSetApproveSpenders(approveConfigs);
        vm.stopPrank();

        _assertFullyGaslessTarget(targetConfigs[0].target, true, FULL_GAS_LIMIT);
        _assertTransferToken(tokenTwo, true, TRANSFER_GAS_LIMIT + 1);
        _assertApproveSpender(tokenTwo, spenderTwo, true, APPROVE_GAS_LIMIT + 1);

        targetConfigs[0] = GaslessWhitelist.FullyGaslessTargetConfig(router, false, 0);
        targetConfigs[1] = GaslessWhitelist.FullyGaslessTargetConfig(targetConfigs[1].target, false, 0);
        tokenConfigs[0] = GaslessWhitelist.GaslessTransferTokenConfig(token, false, 0);
        tokenConfigs[1] = GaslessWhitelist.GaslessTransferTokenConfig(tokenTwo, false, 0);
        approveConfigs[0] = GaslessWhitelist.ApproveSpenderConfig(token, spender, false, 0);
        approveConfigs[1] = GaslessWhitelist.ApproveSpenderConfig(tokenTwo, spenderTwo, false, 0);

        vm.startPrank(owner);
        whitelist.batchSetFullyGaslessTargets(targetConfigs);
        whitelist.batchSetGaslessTransferTokens(tokenConfigs);
        whitelist.batchSetApproveSpenders(approveConfigs);
        vm.stopPrank();

        _assertFullyGaslessTarget(router, false, 0);
        _assertTransferToken(tokenTwo, false, 0);
        _assertApproveSpender(tokenTwo, spenderTwo, false, 0);
    }

    function testBatchSetTransferFromTokensAddAndRemove() public {
        GaslessWhitelist.GaslessTransferTokenConfig[] memory tokenConfigs =
            new GaslessWhitelist.GaslessTransferTokenConfig[](2);
        tokenConfigs[0] = GaslessWhitelist.GaslessTransferTokenConfig(token, true, TRANSFER_FROM_GAS_LIMIT);
        tokenConfigs[1] = GaslessWhitelist.GaslessTransferTokenConfig(tokenTwo, true, TRANSFER_FROM_GAS_LIMIT + 1);

        vm.prank(owner);
        whitelist.batchSetGaslessTransferFromTokens(tokenConfigs);

        _assertTransferFromToken(token, true, TRANSFER_FROM_GAS_LIMIT);
        _assertTransferFromToken(tokenTwo, true, TRANSFER_FROM_GAS_LIMIT + 1);
        // Setting the transferFrom rule must not leak into the transfer rule.
        _assertTransferToken(token, false, 0);

        tokenConfigs[0] = GaslessWhitelist.GaslessTransferTokenConfig(token, false, 0);
        tokenConfigs[1] = GaslessWhitelist.GaslessTransferTokenConfig(tokenTwo, false, 0);

        vm.prank(owner);
        whitelist.batchSetGaslessTransferFromTokens(tokenConfigs);

        _assertTransferFromToken(token, false, 0);
        _assertTransferFromToken(tokenTwo, false, 0);
    }

    function testSettersEmitEvents() public {
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
        emit GaslessEnabledSet(false);
        whitelist.setGaslessEnabled(false);

        vm.expectEmit(false, false, false, true);
        emit MaxGasLimitSet(FULL_GAS_LIMIT);
        whitelist.setMaxGasLimit(FULL_GAS_LIMIT);

        vm.stopPrank();
    }

    function testOnlyOwnerCanConfigure() public {
        vm.prank(nonOwner);
        _expectOnlyOwnerRevert();
        whitelist.setFullyGaslessTarget(router, true, FULL_GAS_LIMIT);

        vm.prank(nonOwner);
        _expectOnlyOwnerRevert();
        whitelist.setGaslessTransferToken(token, true, TRANSFER_GAS_LIMIT);

        vm.prank(nonOwner);
        _expectOnlyOwnerRevert();
        whitelist.setGaslessTransferFromToken(token, true, TRANSFER_FROM_GAS_LIMIT);

        vm.prank(nonOwner);
        _expectOnlyOwnerRevert();
        whitelist.setApproveSpender(token, spender, true, APPROVE_GAS_LIMIT);

        vm.prank(nonOwner);
        _expectOnlyOwnerRevert();
        whitelist.setGaslessEnabled(true);

        vm.prank(nonOwner);
        _expectOnlyOwnerRevert();
        whitelist.setMaxGasLimit(FULL_GAS_LIMIT);

        GaslessWhitelist.FullyGaslessTargetConfig[] memory targetConfigs =
            new GaslessWhitelist.FullyGaslessTargetConfig[](1);
        targetConfigs[0] = GaslessWhitelist.FullyGaslessTargetConfig(router, true, FULL_GAS_LIMIT);
        vm.prank(nonOwner);
        _expectOnlyOwnerRevert();
        whitelist.batchSetFullyGaslessTargets(targetConfigs);

        GaslessWhitelist.GaslessTransferTokenConfig[] memory tokenConfigs =
            new GaslessWhitelist.GaslessTransferTokenConfig[](1);
        tokenConfigs[0] = GaslessWhitelist.GaslessTransferTokenConfig(token, true, TRANSFER_GAS_LIMIT);
        vm.prank(nonOwner);
        _expectOnlyOwnerRevert();
        whitelist.batchSetGaslessTransferTokens(tokenConfigs);

        vm.prank(nonOwner);
        _expectOnlyOwnerRevert();
        whitelist.batchSetGaslessTransferFromTokens(tokenConfigs);

        GaslessWhitelist.ApproveSpenderConfig[] memory approveConfigs = new GaslessWhitelist.ApproveSpenderConfig[](1);
        approveConfigs[0] = GaslessWhitelist.ApproveSpenderConfig(token, spender, true, APPROVE_GAS_LIMIT);
        vm.prank(nonOwner);
        _expectOnlyOwnerRevert();
        whitelist.batchSetApproveSpenders(approveConfigs);
    }

    function testZeroAddressConfigurationReverts() public {
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

    function testEnablingRuleWithZeroGasLimitReverts() public {
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

    function testSettingZeroMaxGasLimitReverts() public {
        vm.prank(owner);
        vm.expectRevert(GaslessWhitelist.InvalidMaxGasLimit.selector);
        whitelist.setMaxGasLimit(0);
    }

    function testRemovingRuleWithZeroGasLimitSucceeds() public {
        vm.startPrank(owner);
        // allowed == false with gasLimit == 0 must remain valid for removals.
        whitelist.setFullyGaslessTarget(router, false, 0);
        whitelist.setGaslessTransferToken(token, false, 0);
        whitelist.setGaslessTransferFromToken(token, false, 0);
        whitelist.setApproveSpender(token, spender, false, 0);
        vm.stopPrank();

        _assertFullyGaslessTarget(router, false, 0);
        _assertTransferToken(token, false, 0);
        _assertTransferFromToken(token, false, 0);
        _assertApproveSpender(token, spender, false, 0);
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

    function _assertTransferFromToken(address target, bool expectedAllowed, uint64 expectedGasLimit) internal view {
        (bool allowed, uint64 gasLimit) = whitelist.gaslessTransferFromTokens(target);
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

    function _expectOnlyOwnerRevert() internal {
        vm.expectRevert(abi.encodeWithSelector(OwnableUpgradeable.OwnableUnauthorizedAccount.selector, nonOwner));
    }
}
