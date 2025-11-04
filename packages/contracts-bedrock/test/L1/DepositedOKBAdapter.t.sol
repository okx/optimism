// SPDX-License-Identifier: MIT
pragma solidity 0.8.15;

// Testing
import { CommonTest } from "test/setup/CommonTest.sol";
import { Test } from "forge-std/Test.sol";

// Contracts
import { DepositedOKBAdapter } from "src/L1/DepositedOKBAdapter.sol";
import { ERC20 } from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";

// Libraries
import { Types } from "src/libraries/Types.sol";
import { GameType } from "src/dispute/lib/LibUDT.sol";

// Interfaces
import { IOKB } from "interfaces/L1/IOKB.sol";
import { IOptimismPortal2 } from "interfaces/L1/IOptimismPortal2.sol";
import { IDisputeGame } from "interfaces/dispute/IDisputeGame.sol";
import { IDisputeGameFactory } from "interfaces/dispute/IDisputeGameFactory.sol";
import { ISystemConfig } from "interfaces/L1/ISystemConfig.sol";
import { ISuperchainConfig } from "interfaces/L1/ISuperchainConfig.sol";
import { IAnchorStateRegistry } from "interfaces/dispute/IAnchorStateRegistry.sol";
import { IProxyAdminOwnedBase } from "interfaces/L1/IProxyAdminOwnedBase.sol";
import { IProxyAdmin } from "interfaces/universal/IProxyAdmin.sol";
import { IETHLockbox } from "interfaces/L1/IETHLockbox.sol";

/// @title MockOKB
/// @notice Mock OKB contract for testing
contract MockOKB is ERC20, IOKB {
    bool public burnTriggered = false;

    constructor(uint256 _totalSupply) ERC20("OKX Token", "OKB") {
        _mint(address(this), _totalSupply);
    }

    function mint(address _to, uint256 _amount) external {
        _mint(_to, _amount);
    }

    function triggerBridge() external override {
        burnTriggered = true;
        // Burn all tokens held by the caller
        _burn(msg.sender, balanceOf(msg.sender));
    }

    function resetBurnTriggered() external {
        burnTriggered = false;
    }
}

/// @title MockOptimismPortal2
/// @notice Mock OptimismPortal2 contract for testing
contract MockOptimismPortal2 is IOptimismPortal2 {
    struct DepositCall {
        address to;
        uint256 mint;
        uint256 value;
        uint64 gasLimit;
        bool isCreation;
        bytes data;
    }

    DepositCall[] public depositCalls;
    address public depositToken;
    bool public shouldRevert = false;

    function setDepositToken(address _token) external {
        depositToken = _token;
    }

    function setShouldRevert(bool _shouldRevert) external {
        shouldRevert = _shouldRevert;
    }

    function depositERC20Transaction(
        address _to,
        uint256 _mint,
        uint256 _value,
        uint64 _gasLimit,
        bool _isCreation,
        bytes memory _data
    )
        external
        override
    {
        if (shouldRevert) {
            revert("MockPortal: forced revert");
        }

        // Simulate portal pulling tokens from the adapter
        if (depositToken != address(0)) {
            bool success = IERC20(depositToken).transferFrom(msg.sender, address(this), _mint);
            require(success, "MockPortal: token transfer failed");
        }

        depositCalls.push(
            DepositCall({
                to: _to, mint: _mint, value: _value, gasLimit: _gasLimit, isCreation: _isCreation, data: _data
            })
        );
    }

    function getDepositCallsLength() external view returns (uint256) {
        return depositCalls.length;
    }

    function getLastDepositCall() external view returns (DepositCall memory) {
        require(depositCalls.length > 0, "No deposit calls");
        return depositCalls[depositCalls.length - 1];
    }

    // Implement other IOptimismPortal2 functions as no-ops for compilation
    receive() external payable { }

    function anchorStateRegistry() external pure override returns (IAnchorStateRegistry) {
        return IAnchorStateRegistry(address(0));
    }

    function ethLockbox() external pure override returns (IETHLockbox) {
        return IETHLockbox(address(0));
    }

    function checkWithdrawal(bytes32, address) external pure override { }
    function depositTransaction(address, uint256, uint64, bool, bytes memory) external payable override { }

    function disputeGameBlacklist(IDisputeGame) external pure override returns (bool) {
        return false;
    }

    function disputeGameFactory() external pure override returns (IDisputeGameFactory) {
        return IDisputeGameFactory(address(0));
    }

    function disputeGameFinalityDelaySeconds() external pure override returns (uint256) {
        return 0;
    }

    function donateETH() external payable override { }

    function superchainConfig() external pure override returns (ISuperchainConfig) {
        return ISuperchainConfig(address(0));
    }

    function finalizeWithdrawalTransaction(Types.WithdrawalTransaction memory) external pure override { }
    function finalizeWithdrawalTransactionExternalProof(
        Types.WithdrawalTransaction memory,
        address
    )
        external
        pure
        override
    { }

    function finalizedWithdrawals(bytes32) external pure override returns (bool) {
        return false;
    }

    function guardian() external pure override returns (address) {
        return address(0);
    }

    function initialize(ISystemConfig, IAnchorStateRegistry) external pure override { }

    function initVersion() external pure override returns (uint8) {
        return 0;
    }

    function l2Sender() external pure override returns (address) {
        return address(0);
    }

    function minimumGasLimit(uint64) external pure override returns (uint64) {
        return 0;
    }

    function numProofSubmitters(bytes32) external pure override returns (uint256) {
        return 0;
    }

    function params() external pure override returns (uint128, uint64, uint64) {
        return (0, 0, 0);
    }

    function paused() external pure override returns (bool) {
        return false;
    }

    function proofMaturityDelaySeconds() external pure override returns (uint256) {
        return 0;
    }

    function proofSubmitters(bytes32, uint256) external pure override returns (address) {
        return address(0);
    }

    function proveWithdrawalTransaction(
        Types.WithdrawalTransaction memory,
        uint256,
        Types.OutputRootProof memory,
        bytes[] memory
    )
        external
        pure
        override
    { }

    function provenWithdrawals(bytes32, address) external pure override returns (IDisputeGame, uint64) {
        return (IDisputeGame(address(0)), 0);
    }

    function respectedGameType() external pure override returns (GameType) {
        return GameType.wrap(0);
    }

    function respectedGameTypeUpdatedAt() external pure override returns (uint64) {
        return 0;
    }

    function systemConfig() external pure override returns (ISystemConfig) {
        return ISystemConfig(address(0));
    }

    function version() external pure override returns (string memory) {
        return "1.0.0";
    }

    function __constructor__(uint256) external pure override { }

    function proxiedInterface() external pure returns (IProxyAdminOwnedBase) {
        return IProxyAdminOwnedBase(address(0));
    }

    function proxyAdmin() external pure returns (IProxyAdmin) {
        return IProxyAdmin(address(0));
    }

    function proxyAdminOwner() external pure returns (address) {
        return address(0);
    }
}

/// @title DepositedOKBAdapter_TestInit
/// @notice Test setup contract for DepositedOKBAdapter tests
contract DepositedOKBAdapter_TestInit is CommonTest {
    // Events for testing
    event WhitelistAdded(address indexed account);
    event WhitelistRemoved(address indexed account);
    event Deposited(address indexed from, address indexed to, uint256 amount);

    uint256 constant TOTAL_SUPPLY = 21_000_000e18; // 21 million OKB
    uint256 constant TEST_AMOUNT = 1000e18; // 1000 OKB

    MockOKB okb;
    MockOptimismPortal2 portal;
    DepositedOKBAdapter adapter;
    address owner;
    address user1;
    address user2;

    function setUp() public virtual override {
        super.setUp();

        owner = makeAddr("owner");
        user1 = makeAddr("user1");
        user2 = makeAddr("user2");

        // Deploy mock contracts
        okb = new MockOKB(TOTAL_SUPPLY);
        portal = new MockOptimismPortal2();

        // Deploy the adapter
        adapter = new DepositedOKBAdapter(address(okb), payable(address(portal)), owner);

        // Set up the portal to accept the adapter as deposit token
        portal.setDepositToken(address(adapter));

        // Give some OKB to test users
        okb.mint(user1, TEST_AMOUNT * 10);
        okb.mint(user2, TEST_AMOUNT * 5);

        vm.deal(user1, 10 ether);
        vm.deal(user2, 10 ether);
    }
}

/// @title DepositedOKBAdapter_Constructor_Test
/// @notice Test contract for DepositedOKBAdapter constructor
contract DepositedOKBAdapter_Constructor_Test is DepositedOKBAdapter_TestInit {
    /// @notice Test successful constructor execution
    function test_constructor_succeeds() public view {
        // Check that the adapter was deployed correctly
        assertEq(address(adapter.OKB()), address(okb));
        assertEq(address(adapter.PORTAL()), address(portal));
        assertEq(adapter.owner(), owner);
        assertEq(adapter.name(), "Deposited OKB");
        assertEq(adapter.symbol(), "dOKB");
        assertEq(adapter.totalSupply(), TOTAL_SUPPLY);
        assertEq(adapter.balanceOf(address(adapter)), TOTAL_SUPPLY);
        assertEq(adapter.DEFAULT_GAS_LIMIT(), 100_000);
    }

    /// @notice Test constructor reverts with zero OKB address
    function test_constructor_zeroOKBAddress_reverts() public {
        vm.expectRevert(DepositedOKBAdapter.AddressCannotBeZero.selector);
        new DepositedOKBAdapter(address(0), payable(address(portal)), owner);
    }

    /// @notice Test constructor reverts with zero portal address
    function test_constructor_zeroPortalAddress_reverts() public {
        vm.expectRevert(DepositedOKBAdapter.AddressCannotBeZero.selector);
        new DepositedOKBAdapter(address(okb), payable(address(0)), owner);
    }
}

/// @title DepositedOKBAdapter_WhitelistManagement_Test
/// @notice Test contract for whitelist management functions
contract DepositedOKBAdapter_WhitelistManagement_Test is DepositedOKBAdapter_TestInit {
    /// @notice Test adding single address to whitelist
    function test_addToWhitelistBatch_single_succeeds() public {
        address[] memory accounts = new address[](1);
        accounts[0] = user1;

        vm.expectEmit(true, false, false, false);
        emit WhitelistAdded(user1);

        vm.prank(owner);
        adapter.addToWhitelistBatch(accounts);

        assertTrue(adapter.whitelist(user1));
    }

    /// @notice Test adding multiple addresses to whitelist
    function test_addToWhitelistBatch_multiple_succeeds() public {
        address[] memory accounts = new address[](2);
        accounts[0] = user1;
        accounts[1] = user2;

        vm.expectEmit(true, false, false, false);
        emit WhitelistAdded(user1);
        vm.expectEmit(true, false, false, false);
        emit WhitelistAdded(user2);

        vm.prank(owner);
        adapter.addToWhitelistBatch(accounts);

        assertTrue(adapter.whitelist(user1));
        assertTrue(adapter.whitelist(user2));
    }

    /// @notice Test adding zero address to whitelist reverts
    function test_addToWhitelistBatch_zeroAddress_reverts() public {
        address[] memory accounts = new address[](1);
        accounts[0] = address(0);

        vm.expectRevert(DepositedOKBAdapter.AddressCannotBeZero.selector);
        vm.prank(owner);
        adapter.addToWhitelistBatch(accounts);
    }

    /// @notice Test non-owner cannot add to whitelist
    function test_addToWhitelistBatch_nonOwner_reverts() public {
        address[] memory accounts = new address[](1);
        accounts[0] = user1;

        vm.expectRevert("Ownable: caller is not the owner");
        vm.prank(user1);
        adapter.addToWhitelistBatch(accounts);
    }

    /// @notice Test removing address from whitelist
    function test_removeFromWhitelistBatch_succeeds() public {
        // First add to whitelist
        address[] memory accounts = new address[](1);
        accounts[0] = user1;
        vm.prank(owner);
        adapter.addToWhitelistBatch(accounts);

        // Then remove
        vm.expectEmit(true, false, false, false);
        emit WhitelistRemoved(user1);

        vm.prank(owner);
        adapter.removeFromWhitelistBatch(accounts);

        assertFalse(adapter.whitelist(user1));
    }

    /// @notice Test removing zero address from whitelist reverts
    function test_removeFromWhitelistBatch_zeroAddress_reverts() public {
        address[] memory accounts = new address[](1);
        accounts[0] = address(0);

        vm.expectRevert(DepositedOKBAdapter.AddressCannotBeZero.selector);
        vm.prank(owner);
        adapter.removeFromWhitelistBatch(accounts);
    }
}

/// @title DepositedOKBAdapter_Deposit_Test
/// @notice Test contract for deposit functionality
contract DepositedOKBAdapter_Deposit_Test is DepositedOKBAdapter_TestInit {
    function setUp() public override {
        super.setUp();

        // Add user1 to whitelist
        address[] memory accounts = new address[](1);
        accounts[0] = user1;
        vm.prank(owner);
        adapter.addToWhitelistBatch(accounts);

        // Approve adapter to spend user1's OKB
        vm.prank(user1);
        okb.approve(address(adapter), type(uint256).max);
    }

    /// @notice Test successful deposit
    function test_deposit_succeeds() public {
        uint256 depositAmount = TEST_AMOUNT;
        address target = makeAddr("target");

        uint256 userBalanceBefore = okb.balanceOf(user1);
        uint256 adapterBalanceBefore = adapter.balanceOf(address(adapter));

        vm.expectEmit(true, true, false, true);
        emit Deposited(user1, target, depositAmount);

        vm.prank(user1);
        adapter.deposit(target, depositAmount);

        // Check that OKB was burned
        assertTrue(okb.burnTriggered());
        assertEq(okb.balanceOf(user1), userBalanceBefore - depositAmount);
        assertEq(okb.balanceOf(address(adapter)), 0); // Should be burned

        // Check that portal was called
        MockOptimismPortal2.DepositCall memory lastCall = portal.getLastDepositCall();
        assertEq(lastCall.to, target);
        assertEq(lastCall.mint, depositAmount);
        assertEq(lastCall.value, depositAmount);
        assertEq(lastCall.gasLimit, adapter.DEFAULT_GAS_LIMIT());
        assertFalse(lastCall.isCreation);
        assertEq(lastCall.data, "");

        // Check adapter token balance decreased (tokens were transferred to portal)
        assertEq(adapter.balanceOf(address(adapter)), adapterBalanceBefore - depositAmount);
    }

    /// @notice Test deposit with non-whitelisted user reverts
    function test_deposit_notWhitelisted_reverts() public {
        vm.expectRevert(DepositedOKBAdapter.NotWhitelisted.selector);
        vm.prank(user2);
        adapter.deposit(makeAddr("target"), TEST_AMOUNT);
    }

    /// @notice Test deposit with zero amount reverts
    function test_deposit_zeroAmount_reverts() public {
        vm.expectRevert(DepositedOKBAdapter.AmountMustBeGreaterThanZero.selector);
        vm.prank(user1);
        adapter.deposit(makeAddr("target"), 0);
    }

    /// @notice Test deposit with insufficient balance reverts
    function test_deposit_insufficientBalance_reverts() public {
        uint256 userBalance = okb.balanceOf(user1);

        vm.expectRevert(DepositedOKBAdapter.InsufficientBalance.selector);
        vm.prank(user1);
        adapter.deposit(makeAddr("target"), userBalance + 1);
    }

    /// @notice Test deposit handles existing OKB balance in contract
    function test_deposit_handlesExistingOKBBalance_succeeds() public {
        uint256 depositAmount = TEST_AMOUNT;
        address target = makeAddr("target");

        // Send some OKB directly to the adapter (simulating mistaken transfer)
        vm.prank(user1);
        okb.transfer(address(adapter), 100e18);

        uint256 ownerBalanceBefore = okb.balanceOf(owner);
        uint256 adapterOKBBefore = okb.balanceOf(address(adapter));

        vm.prank(user1);
        adapter.deposit(target, depositAmount);

        // Check that the existing OKB was transferred to owner
        assertEq(okb.balanceOf(owner), ownerBalanceBefore + adapterOKBBefore);

        // Check that the burn was triggered
        assertTrue(okb.burnTriggered());
    }
}

/// @title DepositedOKBAdapter_Transfer_Test
/// @notice Test contract for transfer restrictions
contract DepositedOKBAdapter_Transfer_Test is DepositedOKBAdapter_TestInit {
    /// @notice Test regular transfer is not allowed
    function test_transfer_notAllowed_reverts() public {
        vm.expectRevert(DepositedOKBAdapter.TransferNotAllowed.selector);
        vm.prank(user1);
        adapter.transfer(user2, 1000);
    }

    /// @notice Test transferFrom is not allowed except from adapter to portal
    function test_transferFrom_notAllowed_reverts() public {
        vm.expectRevert(DepositedOKBAdapter.TransferNotAllowed.selector);
        vm.prank(user1);
        adapter.transferFrom(user1, user2, 1000);
    }

    /// @notice Test transferFrom from adapter to portal is allowed
    function test_transferFrom_adapterToPortal_succeeds() public {
        uint256 amount = 1000e18;

        // First approve the portal to spend from the adapter
        vm.prank(address(adapter));
        adapter.approve(address(portal), amount);

        // This should succeed (simulating portal pulling tokens)
        vm.prank(address(portal));
        bool success = adapter.transferFrom(address(adapter), address(portal), amount);
        assertTrue(success);

        assertEq(adapter.balanceOf(address(portal)), amount);
        assertEq(adapter.balanceOf(address(adapter)), TOTAL_SUPPLY - amount);
    }

    /// @notice Test portal calling transferFrom from adapter to user1 reverts
    function test_transferFrom_portalCallerAdapterToUser1_reverts() public {
        uint256 amount = 1000e18;

        // First approve the portal to spend from the adapter
        vm.prank(address(adapter));
        adapter.approve(address(portal), amount);

        // Portal calling transferFrom(adapter, user1, amount) should revert
        // because 'to' is not the portal address
        vm.expectRevert(DepositedOKBAdapter.TransferNotAllowed.selector);
        vm.prank(address(portal));
        adapter.transferFrom(address(adapter), user1, amount);
    }

    /// @notice Test portal calling transferFrom from portal to adapter reverts
    function test_transferFrom_portalCallerPortalToAdapter_reverts() public {
        uint256 amount = 1000e18;

        // First approve the portal to spend from the portal (though this wouldn't make sense in practice)
        vm.prank(address(portal));
        adapter.approve(address(portal), amount);

        // Portal calling transferFrom(portal, adapter, amount) should revert
        // because 'from' is not the adapter address
        vm.expectRevert(DepositedOKBAdapter.TransferNotAllowed.selector);
        vm.prank(address(portal));
        adapter.transferFrom(address(portal), address(adapter), amount);
    }

    /// @notice Test portal calling transferFrom from portal to user1 reverts
    function test_transferFrom_portalCallerPortalToUser1_reverts() public {
        uint256 amount = 1000e18;

        // First approve the portal to spend from the portal
        vm.prank(address(portal));
        adapter.approve(address(portal), amount);

        // Portal calling transferFrom(portal, user1, amount) should revert
        // because 'from' is not the adapter address and 'to' is not the portal
        vm.expectRevert(DepositedOKBAdapter.TransferNotAllowed.selector);
        vm.prank(address(portal));
        adapter.transferFrom(address(portal), user1, amount);
    }
}

/// @title MockUSDT
/// @notice Mock USDT contract that mimics real USDT behavior (no return value for transfer)
contract MockUSDT {
    mapping(address => uint256) private _balances;
    mapping(address => mapping(address => uint256)) private _allowances;

    uint256 private _totalSupply;
    string public name = "Tether USD";
    string public symbol = "USDT";
    uint8 public decimals = 6;

    bool public shouldRevert = false;

    constructor(uint256 _supply) {
        _totalSupply = _supply;
        _balances[msg.sender] = _supply;
    }

    function totalSupply() public view returns (uint256) {
        return _totalSupply;
    }

    function balanceOf(address account) public view returns (uint256) {
        return _balances[account];
    }

    function allowance(address owner, address spender) public view returns (uint256) {
        return _allowances[owner][spender];
    }

    /// @notice USDT-style transfer that doesn't return bool and reverts on failure
    function transfer(address to, uint256 amount) public {
        if (shouldRevert) {
            revert("USDT: transfer failed");
        }
        require(to != address(0), "USDT: transfer to zero address");
        require(_balances[msg.sender] >= amount, "USDT: insufficient balance");

        _balances[msg.sender] -= amount;
        _balances[to] += amount;
    }

    /// @notice USDT-style transferFrom that doesn't return bool and reverts on failure
    function transferFrom(address from, address to, uint256 amount) public {
        if (shouldRevert) {
            revert("USDT: transfer failed");
        }
        require(to != address(0), "USDT: transfer to zero address");
        require(_balances[from] >= amount, "USDT: insufficient balance");
        require(_allowances[from][msg.sender] >= amount, "USDT: insufficient allowance");

        _balances[from] -= amount;
        _balances[to] += amount;
        _allowances[from][msg.sender] -= amount;
    }

    function approve(address spender, uint256 amount) public returns (bool) {
        _allowances[msg.sender][spender] = amount;
        return true;
    }

    function setShouldRevert(bool _shouldRevert) external {
        shouldRevert = _shouldRevert;
    }

    /// @notice Mint tokens for testing
    function mint(address to, uint256 amount) external {
        _balances[to] += amount;
        _totalSupply += amount;
    }
}

/// @title DepositedOKBAdapter_Rescue_Test
/// @notice Test contract for ERC20 rescue functionality
contract DepositedOKBAdapter_Rescue_Test is DepositedOKBAdapter_TestInit {
    ERC20 testToken;
    MockUSDT mockUSDT;

    function setUp() public override {
        super.setUp();
        testToken = new ERC20("Test Token", "TEST");
        // Mint some tokens to the adapter (simulating accidental transfer)
        deal(address(testToken), address(adapter), 1000e18);

        // Create mock USDT with 6 decimals (1M USDT)
        mockUSDT = new MockUSDT(1_000_000e6);
        // Transfer some USDT to the adapter (simulating accidental transfer)
        mockUSDT.mint(address(adapter), 10_000e6); // 10,000 USDT
    }

    /// @notice Test successful ERC20 rescue
    function test_rescueERC20_succeeds() public {
        uint256 rescueAmount = 500e18;
        address rescueTo = makeAddr("rescueTo");

        uint256 balanceBefore = testToken.balanceOf(rescueTo);

        vm.prank(owner);
        adapter.rescueERC20(address(testToken), rescueTo, rescueAmount);

        assertEq(testToken.balanceOf(rescueTo), balanceBefore + rescueAmount);
        assertEq(testToken.balanceOf(address(adapter)), 1000e18 - rescueAmount);
    }

    /// @notice Test rescue with zero token address reverts
    function test_rescueERC20_zeroTokenAddress_reverts() public {
        vm.expectRevert(DepositedOKBAdapter.AddressCannotBeZero.selector);
        vm.prank(owner);
        adapter.rescueERC20(address(0), makeAddr("rescueTo"), 100);
    }

    /// @notice Test rescue with zero recipient address reverts
    function test_rescueERC20_zeroRecipientAddress_reverts() public {
        vm.expectRevert(DepositedOKBAdapter.AddressCannotBeZero.selector);
        vm.prank(owner);
        adapter.rescueERC20(address(testToken), address(0), 100);
    }

    /// @notice Test rescue with zero amount reverts
    function test_rescueERC20_zeroAmount_reverts() public {
        vm.expectRevert(DepositedOKBAdapter.AmountMustBeGreaterThanZero.selector);
        vm.prank(owner);
        adapter.rescueERC20(address(testToken), makeAddr("rescueTo"), 0);
    }

    /// @notice Test non-owner cannot rescue
    function test_rescueERC20_nonOwner_reverts() public {
        vm.expectRevert("Ownable: caller is not the owner");
        vm.prank(user1);
        adapter.rescueERC20(address(testToken), makeAddr("rescueTo"), 100);
    }

    /// @notice Test successful USDT rescue (USDT doesn't return bool from transfer)
    function test_rescueUSDT_succeeds() public {
        uint256 rescueAmount = 5000e6; // 5,000 USDT (6 decimals)
        address rescueTo = makeAddr("rescueTo");

        uint256 balanceBefore = mockUSDT.balanceOf(rescueTo);
        uint256 adapterBalanceBefore = mockUSDT.balanceOf(address(adapter));

        vm.prank(owner);
        adapter.rescueERC20(address(mockUSDT), rescueTo, rescueAmount);

        assertEq(mockUSDT.balanceOf(rescueTo), balanceBefore + rescueAmount);
        assertEq(mockUSDT.balanceOf(address(adapter)), adapterBalanceBefore - rescueAmount);
    }

    /// @notice Test USDT rescue handles revert case properly
    function test_rescueUSDT_transferReverts_fails() public {
        uint256 rescueAmount = 5000e6; // 5,000 USDT (6 decimals)
        address rescueTo = makeAddr("rescueTo");

        // Make USDT transfers revert
        mockUSDT.setShouldRevert(true);

        // The rescue should revert when USDT transfer fails
        vm.expectRevert("USDT: transfer failed");
        vm.prank(owner);
        adapter.rescueERC20(address(mockUSDT), rescueTo, rescueAmount);
    }
}

/// @title DepositedOKBAdapter_Integration_Test
/// @notice Integration tests combining multiple functionalities
contract DepositedOKBAdapter_Integration_Test is DepositedOKBAdapter_TestInit {
    function setUp() public override {
        super.setUp();

        // Add users to whitelist
        address[] memory accounts = new address[](2);
        accounts[0] = user1;
        accounts[1] = user2;
        vm.prank(owner);
        adapter.addToWhitelistBatch(accounts);

        // Approve adapter to spend users' OKB
        vm.prank(user1);
        okb.approve(address(adapter), type(uint256).max);
        vm.prank(user2);
        okb.approve(address(adapter), type(uint256).max);
    }

    /// @notice Test multiple deposits from different users
    function test_multipleDeposits_succeeds() public {
        address target1 = makeAddr("target1");
        address target2 = makeAddr("target2");
        uint256 amount1 = TEST_AMOUNT;
        uint256 amount2 = TEST_AMOUNT / 2;

        // First deposit
        vm.prank(user1);
        adapter.deposit(target1, amount1);

        // Reset burn trigger for second deposit
        okb.resetBurnTriggered();

        // Second deposit
        vm.prank(user2);
        adapter.deposit(target2, amount2);

        // Check both deposits were recorded
        assertEq(portal.getDepositCallsLength(), 2);

        // Check that both deposits were recorded
        assertTrue(portal.getDepositCallsLength() >= 2);

        // We can't easily access individual array elements, so we'll just verify the last call
        MockOptimismPortal2.DepositCall memory lastCall = portal.getLastDepositCall();
        assertEq(lastCall.to, target2);
        assertEq(lastCall.mint, amount2);
    }

    /// @notice Test whitelist management followed by deposits
    function test_whitelistManagementThenDeposit_succeeds() public {
        address user3 = makeAddr("user3");
        okb.mint(user3, TEST_AMOUNT);
        vm.prank(user3);
        okb.approve(address(adapter), type(uint256).max);

        // Initially user3 is not whitelisted
        vm.expectRevert(DepositedOKBAdapter.NotWhitelisted.selector);
        vm.prank(user3);
        adapter.deposit(makeAddr("target"), TEST_AMOUNT);

        // Add user3 to whitelist
        address[] memory accounts = new address[](1);
        accounts[0] = user3;
        vm.prank(owner);
        adapter.addToWhitelistBatch(accounts);

        // Now deposit should succeed
        vm.prank(user3);
        adapter.deposit(makeAddr("target"), TEST_AMOUNT);

        // Remove user3 from whitelist
        vm.prank(owner);
        adapter.removeFromWhitelistBatch(accounts);

        // Deposit should fail again
        vm.expectRevert(DepositedOKBAdapter.NotWhitelisted.selector);
        vm.prank(user3);
        adapter.deposit(makeAddr("target"), TEST_AMOUNT);
    }
}
