// SPDX-License-Identifier: MIT
pragma solidity 0.8.15;

// Testing
import { CommonTest } from "test/setup/CommonTest.sol";
import { TestERC20 } from "test/mocks/TestERC20.sol";

// Libraries
import { AddressAliasHelper } from "src/vendor/AddressAliasHelper.sol";
import { Constants } from "src/libraries/Constants.sol";
import { Features } from "src/libraries/Features.sol";

// Interfaces
import { IOptimismPortal2 as IOptimismPortal } from "interfaces/L1/IOptimismPortal2.sol";

/// @title OptimismPortal2_DepositERC20Transaction_Test
/// @notice Test contract for OptimismPortal2 `depositERC20Transaction` function.
/// @dev SYS_FEATURE__CUSTOM_GAS_TOKEN=true forge test --match-path test/L1/OptimismPortal2.xlayer.t.sol
/// @dev forge test --match-path test/L1/OptimismPortal2.xlayer.t.sol
contract OptimismPortal2_DepositERC20Transaction_Test is CommonTest {
    address depositor;
    TestERC20 customGasToken;

    // NOTE: `OptimismPortal_InsufficientDeposit()` is defined on the implementation contract but
    // is not currently declared on `IOptimismPortal2`, so we use the raw selector here.
    bytes4 internal constant OPTIMISM_PORTAL_INSUFFICIENT_DEPOSIT_SELECTOR =
        bytes4(keccak256("OptimismPortal_InsufficientDeposit()"));

    function setUp() public override {
        super.setUp();
        depositor = makeAddr("depositor");

        // If CGT mode is enabled, ensure the OptimismPortal sees a non-ETH gas paying token so
        // we can test the happy paths deterministically.
        if (isSysFeatureEnabled(Features.CUSTOM_GAS_TOKEN)) {
            customGasToken = new TestERC20();

            vm.mockCall(
                address(systemConfig),
                abi.encodeCall(systemConfig.gasPayingToken, ()),
                abi.encode(address(customGasToken), uint8(18))
            );
        }
    }

    /// @notice Tests that `depositERC20Transaction` reverts when custom gas token mode is disabled.
    function test_depositERC20Transaction_notCustomGasToken_reverts() external {
        skipIfSysFeatureEnabled(Features.CUSTOM_GAS_TOKEN);

        vm.expectRevert(IOptimismPortal.OptimismPortal_OnlyCustomGasToken.selector);
        optimismPortal2.depositERC20Transaction({
            _to: address(0x40),
            _mint: 100,
            _value: 100,
            _gasLimit: 21_000,
            _isCreation: false,
            _data: hex""
        });
    }

    /// @notice Tests that `depositERC20Transaction` reverts when `_mint != _value`.
    function test_depositERC20Transaction_mintValueMismatch_reverts() external {
        skipIfSysFeatureDisabled(Features.CUSTOM_GAS_TOKEN);

        vm.expectRevert(OPTIMISM_PORTAL_INSUFFICIENT_DEPOSIT_SELECTOR);
        vm.prank(depositor, depositor);
        optimismPortal2.depositERC20Transaction({
            _to: address(0x40),
            _mint: 100,
            _value: 99,
            _gasLimit: 21_000,
            _isCreation: false,
            _data: hex""
        });
    }

    /// @notice Tests that `depositERC20Transaction` reverts when gas token is invalid (ETH).
    function test_depositERC20Transaction_invalidGasToken_reverts() external {
        skipIfSysFeatureDisabled(Features.CUSTOM_GAS_TOKEN);

        vm.mockCall(
            address(systemConfig),
            abi.encodeCall(systemConfig.gasPayingToken, ()),
            abi.encode(Constants.ETHER, uint8(18))
        );

        vm.expectRevert(IOptimismPortal.OptimismPortal_InvalidGasToken.selector);
        vm.prank(depositor, depositor);
        optimismPortal2.depositERC20Transaction({
            _to: address(0x40),
            _mint: 100,
            _value: 100,
            _gasLimit: 21_000,
            _isCreation: false,
            _data: hex""
        });
    }

    /// @notice Tests that `depositERC20Transaction` reverts when destination address is non-zero
    ///         for a contract creation deposit.
    function test_depositERC20Transaction_contractCreation_reverts() external {
        skipIfSysFeatureDisabled(Features.CUSTOM_GAS_TOKEN);

        uint256 mintAmount = 100;
        customGasToken.mint(address(this), mintAmount);
        customGasToken.approve(address(optimismPortal2), mintAmount);

        vm.expectRevert(IOptimismPortal.OptimismPortal_BadTarget.selector);
        optimismPortal2.depositERC20Transaction({
            _to: address(1),
            _mint: mintAmount,
            _value: mintAmount,
            _gasLimit: 21_000,
            _isCreation: true,
            _data: hex""
        });
    }

    /// @notice Tests that `depositERC20Transaction` reverts when the gas limit is too small.
    function test_depositERC20Transaction_smallGasLimit_reverts() external {
        skipIfSysFeatureDisabled(Features.CUSTOM_GAS_TOKEN);

        uint256 mintAmount = 100;
        customGasToken.mint(depositor, mintAmount);
        vm.prank(depositor);
        customGasToken.approve(address(optimismPortal2), mintAmount);

        vm.expectRevert(IOptimismPortal.OptimismPortal_GasLimitTooLow.selector);
        vm.prank(depositor, depositor);
        optimismPortal2.depositERC20Transaction({
            _to: address(0x40),
            _mint: mintAmount,
            _value: mintAmount,
            _gasLimit: 0,
            _isCreation: false,
            _data: hex""
        });
    }

    /// @notice Tests that `depositERC20Transaction` reverts when the data is too large.
    function test_depositERC20Transaction_largeData_reverts() external {
        skipIfSysFeatureDisabled(Features.CUSTOM_GAS_TOKEN);

        uint256 size = 120_001;
        uint64 gasLimit = optimismPortal2.minimumGasLimit(uint64(size));

        uint256 mintAmount = 100;
        customGasToken.mint(depositor, mintAmount);
        vm.prank(depositor);
        customGasToken.approve(address(optimismPortal2), mintAmount);

        vm.expectRevert(IOptimismPortal.OptimismPortal_CalldataTooLarge.selector);
        vm.prank(depositor, depositor);
        optimismPortal2.depositERC20Transaction({
            _to: address(0x40),
            _mint: mintAmount,
            _value: mintAmount,
            _gasLimit: gasLimit,
            _isCreation: false,
            _data: new bytes(size)
        });
    }

    /// @notice Tests that `depositERC20Transaction` succeeds for small, but sufficient, gas limits.
    function testFuzz_depositERC20Transaction_smallGasLimit_succeeds(bytes memory _data, bool _shouldFail) external {
        skipIfSysFeatureDisabled(Features.CUSTOM_GAS_TOKEN);

        uint64 gasLimit = optimismPortal2.minimumGasLimit(uint64(_data.length));
        if (_shouldFail) {
            gasLimit = uint64(bound(gasLimit, 0, gasLimit - 1));
            vm.expectRevert(IOptimismPortal.OptimismPortal_GasLimitTooLow.selector);
        }

        vm.prank(depositor, depositor);
        optimismPortal2.depositERC20Transaction({
            _to: address(0x40),
            _mint: 0,
            _value: 0,
            _gasLimit: gasLimit,
            _isCreation: false,
            _data: _data
        });
    }

    /// @notice Tests that `depositERC20Transaction` succeeds for an EOA.
    function testFuzz_depositERC20Transaction_eoa_succeeds(
        address _to,
        uint64 _gasLimit,
        uint256 _mint,
        uint256 _value,
        bool _isCreation,
        bytes memory _data
    )
        external
    {
        skipIfSysFeatureDisabled(Features.CUSTOM_GAS_TOKEN);

        _gasLimit = uint64(
            bound(
                _gasLimit,
                optimismPortal2.minimumGasLimit(uint64(_data.length)),
                systemConfig.resourceConfig().maxResourceLimit
            )
        );
        if (_isCreation) _to = address(0);

        // `depositERC20Transaction` requires `_mint == _value`.
        _mint = bound(_mint, 0, type(uint128).max);
        _value = _mint;

        if (_mint > 0) {
            customGasToken.mint(depositor, _mint);
            vm.prank(depositor);
            customGasToken.approve(address(optimismPortal2), _mint);
        }

        vm.expectEmit(address(optimismPortal2));
        emitTransactionDeposited({
            _from: depositor,
            _to: _to,
            _mint: _mint,
            _value: _value,
            _gasLimit: _gasLimit,
            _isCreation: _isCreation,
            _data: _data
        });

        vm.prank(depositor, depositor);
        optimismPortal2.depositERC20Transaction({
            _to: _to,
            _mint: _mint,
            _value: _value,
            _gasLimit: _gasLimit,
            _isCreation: _isCreation,
            _data: _data
        });
    }

    /// @notice Tests that `depositERC20Transaction` succeeds for a contract.
    function testFuzz_depositERC20Transaction_contract_succeeds(
        address _to,
        uint64 _gasLimit,
        uint256 _mint,
        uint256 _value,
        bool _isCreation,
        bytes memory _data
    )
        external
    {
        skipIfSysFeatureDisabled(Features.CUSTOM_GAS_TOKEN);

        _gasLimit = uint64(
            bound(
                _gasLimit,
                optimismPortal2.minimumGasLimit(uint64(_data.length)),
                systemConfig.resourceConfig().maxResourceLimit
            )
        );
        if (_isCreation) _to = address(0);

        // `depositERC20Transaction` requires `_mint == _value`.
        _mint = bound(_mint, 0, type(uint128).max);
        _value = _mint;

        if (_mint > 0) {
            customGasToken.mint(address(this), _mint);
            customGasToken.approve(address(optimismPortal2), _mint);
        }

        vm.expectEmit(address(optimismPortal2));
        emitTransactionDeposited({
            _from: AddressAliasHelper.applyL1ToL2Alias(address(this)),
            _to: _to,
            _mint: _mint,
            _value: _value,
            _gasLimit: _gasLimit,
            _isCreation: _isCreation,
            _data: _data
        });

        optimismPortal2.depositERC20Transaction({
            _to: _to,
            _mint: _mint,
            _value: _value,
            _gasLimit: _gasLimit,
            _isCreation: _isCreation,
            _data: _data
        });
    }

    /// @notice Tests that `depositERC20Transaction` transfers tokens when mint is non-zero.
    function test_depositERC20Transaction_transfersTokens_succeeds() external {
        skipIfSysFeatureDisabled(Features.CUSTOM_GAS_TOKEN);

        uint256 mintAmount = 1000;

        customGasToken.mint(depositor, mintAmount);
        vm.prank(depositor);
        customGasToken.approve(address(optimismPortal2), mintAmount);

        uint256 depositorBalanceBefore = customGasToken.balanceOf(depositor);
        uint256 portalBalanceBefore = customGasToken.balanceOf(address(optimismPortal2));

        vm.prank(depositor, depositor);
        optimismPortal2.depositERC20Transaction({
            _to: address(0x40),
            _mint: mintAmount,
            _value: mintAmount,
            _gasLimit: 50_000,
            _isCreation: false,
            _data: hex""
        });

        assertEq(customGasToken.balanceOf(depositor), depositorBalanceBefore - mintAmount);
        assertEq(customGasToken.balanceOf(address(optimismPortal2)), portalBalanceBefore + mintAmount);
    }

    /// @notice Tests that `depositERC20Transaction` does not transfer tokens when mint is zero.
    function test_depositERC20Transaction_zeroMint_succeeds() external {
        skipIfSysFeatureDisabled(Features.CUSTOM_GAS_TOKEN);

        uint256 depositorBalanceBefore = customGasToken.balanceOf(depositor);
        uint256 portalBalanceBefore = customGasToken.balanceOf(address(optimismPortal2));

        vm.prank(depositor, depositor);
        optimismPortal2.depositERC20Transaction({
            _to: address(0x40),
            _mint: 0,
            _value: 0,
            _gasLimit: 50_000,
            _isCreation: false,
            _data: hex""
        });

        assertEq(customGasToken.balanceOf(depositor), depositorBalanceBefore);
        assertEq(customGasToken.balanceOf(address(optimismPortal2)), portalBalanceBefore);
    }
}


