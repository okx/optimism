# Custom Gas Token Developer Guide

This guide provides practical examples and usage patterns for working with the Custom Gas Token (CGT) implementation.

## Table of Contents
1. [Quick Start](#quick-start)
2. [For Users](#for-users)
3. [For Developers](#for-developers)
4. [For Chain Operators](#for-chain-operators)
5. [Testing Guide](#testing-guide)
6. [Common Patterns](#common-patterns)
7. [Troubleshooting](#troubleshooting)

---

## Quick Start

### Checking if a Chain Uses CGT

```solidity
// On L2
import { Predeploys } from "src/libraries/Predeploys.sol";
import { IL1Block } from "interfaces/L2/IL1Block.sol";

function isCGTChain() public view returns (bool) {
    return IL1Block(Predeploys.L1_BLOCK_ATTRIBUTES).isCustomGasToken();
}

function getTokenInfo() public view returns (string memory name, string memory symbol) {
    IL1Block l1Block = IL1Block(Predeploys.L1_BLOCK_ATTRIBUTES);
    name = l1Block.gasPayingTokenName();
    symbol = l1Block.gasPayingTokenSymbol();
}
```

### Checking on L1

```solidity
import { IOptimismPortal2 } from "interfaces/L1/IOptimismPortal2.sol";

function getL1TokenInfo(address portal) public view returns (address token, uint8 decimals) {
    return IOptimismPortal2(portal).gasPayingToken();
}
```

---

## For Users

### Depositing CGT to L2

#### Option 1: Direct Deposit (Generic CGT Token)

```solidity
import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { IOptimismPortal2 } from "interfaces/L1/IOptimismPortal2.sol";

function depositCGT(
    address portal,
    address cgtToken,
    address recipient,
    uint256 amount
) external {
    // Step 1: Approve the portal to spend your CGT
    IERC20(cgtToken).approve(portal, amount);

    // Step 2: Deposit to L2
    IOptimismPortal2(portal).depositERC20Transaction({
        _to: recipient,
        _mint: amount,      // Amount to mint on L2
        _value: amount,     // Amount to send to recipient
        _gasLimit: 100000,  // Gas limit for L2 transaction
        _isCreation: false, // Not a contract creation
        _data: ""          // No additional data
    });
}
```

#### Option 2: Using OKB Adapter (Burn and Mint)

```solidity
import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";

interface IDepositedOKBAdapter {
    function deposit(address _to, uint256 _amount) external;
}

function depositOKB(
    address adapter,
    address okbToken,
    address recipient,
    uint256 amount
) external {
    // Step 1: Approve the adapter to spend your OKB
    IERC20(okbToken).approve(adapter, amount);

    // Step 2: Deposit (this will burn OKB and mint on L2)
    IDepositedOKBAdapter(adapter).deposit(recipient, amount);
}
```

### JavaScript/TypeScript Example

```typescript
import { ethers } from 'ethers';

async function depositCGT(
    portal: string,
    cgtToken: string,
    recipient: string,
    amount: bigint,
    signer: ethers.Signer
) {
    // ERC20 ABI for approve
    const erc20 = new ethers.Contract(
        cgtToken,
        ['function approve(address spender, uint256 amount) returns (bool)'],
        signer
    );

    // OptimismPortal2 ABI for depositERC20Transaction
    const portalContract = new ethers.Contract(
        portal,
        [
            'function depositERC20Transaction(address _to, uint256 _mint, uint256 _value, uint64 _gasLimit, bool _isCreation, bytes memory _data)'
        ],
        signer
    );

    // Step 1: Approve
    const approveTx = await erc20.approve(portal, amount);
    await approveTx.wait();
    console.log('Approved CGT for portal');

    // Step 2: Deposit
    const depositTx = await portalContract.depositERC20Transaction(
        recipient,
        amount,    // _mint
        amount,    // _value
        100000,    // _gasLimit
        false,     // _isCreation
        '0x'       // _data
    );
    await depositTx.wait();
    console.log('Deposited CGT to L2');
}
```

---

## For Developers

### Writing Portable Cross-Chain Contracts

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import { IL1Block } from "interfaces/L2/IL1Block.sol";
import { Predeploys } from "src/libraries/Predeploys.sol";

/// @notice Example contract that works on both ETH and CGT chains
contract PortableBridge {
    IL1Block constant L1_BLOCK = IL1Block(Predeploys.L1_BLOCK_ATTRIBUTES);

    /// @notice Bridge funds to L2 (works on both ETH and CGT chains)
    function bridgeToL2(address _portal, address _recipient, uint256 _amount) external payable {
        if (L1_BLOCK.isCustomGasToken()) {
            // CGT chain - use ERC20 deposit
            (address token,) = IOptimismPortal2(_portal).gasPayingToken();
            IERC20(token).transferFrom(msg.sender, address(this), _amount);
            IERC20(token).approve(_portal, _amount);

            IOptimismPortal2(_portal).depositERC20Transaction({
                _to: _recipient,
                _mint: _amount,
                _value: _amount,
                _gasLimit: 100000,
                _isCreation: false,
                _data: ""
            });
        } else {
            // ETH chain - use standard deposit
            require(msg.value >= _amount, "Insufficient ETH");
            IOptimismPortal2(_portal).depositTransaction{value: _amount}({
                _to: _recipient,
                _value: _amount,
                _gasLimit: 100000,
                _isCreation: false,
                _data: ""
            });
        }
    }
}
```

### Detecting Chain Type at Runtime

```solidity
contract ChainTypeDetector {
    function getChainType() public view returns (string memory) {
        IL1Block l1Block = IL1Block(Predeploys.L1_BLOCK_ATTRIBUTES);

        if (l1Block.isCustomGasToken()) {
            return string.concat(
                "CGT Chain - ",
                l1Block.gasPayingTokenName(),
                " (",
                l1Block.gasPayingTokenSymbol(),
                ")"
            );
        }

        return "ETH Chain";
    }
}
```

---

## For Chain Operators

### Deploying a New CGT Chain

```solidity
// 1. Deploy DepositedOKBAdapter (if using OKB)
DepositedOKBAdapter adapter = new DepositedOKBAdapter({
    _okb: OKB_TOKEN_ADDRESS,
    _portal: OPTIMISM_PORTAL_ADDRESS
});

// 2. Set the gas paying token in SystemConfig storage
// This is done during deployment via the genesis configuration
// See: op-deployer/pkg/deployer/state/deploy_config.go

// 3. Enable the CUSTOM_GAS_TOKEN feature flag
systemConfig.setFeature(Features.CUSTOM_GAS_TOKEN, true);

// 4. Set token metadata in L1BlockCGT storage (during genesis)
// The token name and symbol are read from GasPayingToken library
```

### Genesis Configuration

```json
{
  "gasTokenDeployConfig": {
    "useCustomGasToken": true,
    "gasPayingTokenName": "OKB",
    "gasPayingTokenSymbol": "OKB",
    "nativeAssetLiquidityAmount": "0x0"
  }
}
```

### Monitoring CGT Operations

```solidity
// Monitor TransactionDeposited events
event TransactionDeposited(
    address indexed from,
    address indexed to,
    uint256 indexed version,
    bytes opaqueData
);

// For CGT chains, decode opaqueData to get:
// - _mint (uint256)
// - _value (uint256)
// - _gasLimit (uint64)
// - _isCreation (bool)
// - _data (bytes)
```

---

## Testing Guide

### Foundry Test Example

```solidity
// SPDX-License-Identifier: MIT
pragma solidity 0.8.15;

import { Test } from "forge-std/Test.sol";
import { OptimismPortal2 } from "src/L1/OptimismPortal2.sol";
import { DepositedOKBAdapter } from "src/L1/DepositedOKBAdapter.sol";
import { MockERC20 } from "test/mocks/MockERC20.sol";

contract CGTDepositTest is Test {
    OptimismPortal2 portal;
    DepositedOKBAdapter adapter;
    MockERC20 okb;

    address user = address(0x1);
    address recipient = address(0x2);
    uint256 depositAmount = 100 ether;

    function setUp() public {
        // Deploy mock OKB token
        okb = new MockERC20("OKB", "OKB", 18);
        okb.mint(user, 1000 ether);

        // Deploy portal (with necessary dependencies)
        portal = new OptimismPortal2(/* constructor args */);

        // Deploy adapter
        adapter = new DepositedOKBAdapter(address(okb), address(portal));

        // Enable CGT mode
        systemConfig.setFeature(Features.CUSTOM_GAS_TOKEN, true);
    }

    function test_depositERC20Transaction_succeeds() public {
        vm.startPrank(user);

        // Approve portal
        okb.approve(address(portal), depositAmount);

        // Expect TransactionDeposited event
        vm.expectEmit(true, true, true, false);
        emit TransactionDeposited(user, recipient, 0, "");

        // Deposit
        portal.depositERC20Transaction({
            _to: recipient,
            _mint: depositAmount,
            _value: depositAmount,
            _gasLimit: 100000,
            _isCreation: false,
            _data: ""
        });

        vm.stopPrank();

        // Verify tokens transferred
        assertEq(okb.balanceOf(address(portal)), depositAmount);
    }

    function test_adapter_deposit_burnsOKB() public {
        vm.startPrank(user);

        // Approve adapter
        okb.approve(address(adapter), depositAmount);

        uint256 balanceBefore = okb.balanceOf(user);

        // Deposit via adapter
        adapter.deposit(recipient, depositAmount);

        vm.stopPrank();

        // Verify OKB was burned
        assertEq(okb.balanceOf(address(0)), depositAmount);
        assertEq(okb.balanceOf(user), balanceBefore - depositAmount);
    }
}
```

---

## Common Patterns

### Pattern 1: Safe Deposit with Approval Check

```solidity
function safeDepositCGT(
    address portal,
    address token,
    address recipient,
    uint256 amount
) external {
    IERC20 cgt = IERC20(token);

    // Check balance
    require(cgt.balanceOf(msg.sender) >= amount, "Insufficient balance");

    // Check allowance
    uint256 currentAllowance = cgt.allowance(msg.sender, portal);
    if (currentAllowance < amount) {
        // Approve if needed
        cgt.approve(portal, type(uint256).max);
    }

    // Deposit
    IOptimismPortal2(portal).depositERC20Transaction(
        recipient,
        amount,
        amount,
        100000,
        false,
        ""
    );
}
```

### Pattern 2: Batch Deposits

```solidity
function batchDeposit(
    address portal,
    address token,
    address[] calldata recipients,
    uint256[] calldata amounts
) external {
    require(recipients.length == amounts.length, "Length mismatch");

    IERC20 cgt = IERC20(token);
    uint256 totalAmount;

    // Calculate total
    for (uint256 i = 0; i < amounts.length; i++) {
        totalAmount += amounts[i];
    }

    // Single approval for all deposits
    cgt.approve(portal, totalAmount);

    // Perform deposits
    for (uint256 i = 0; i < recipients.length; i++) {
        IOptimismPortal2(portal).depositERC20Transaction(
            recipients[i],
            amounts[i],
            amounts[i],
            100000,
            false,
            ""
        );
    }
}
```

### Pattern 3: Contract Creation on L2

```solidity
function deployContractOnL2(
    address portal,
    address token,
    uint256 fundingAmount,
    bytes memory initCode
) external {
    IERC20 cgt = IERC20(token);
    cgt.approve(portal, fundingAmount);

    IOptimismPortal2(portal).depositERC20Transaction({
        _to: address(0),        // Contract creation
        _mint: fundingAmount,   // Fund the new contract
        _value: fundingAmount,  // Send to new contract
        _gasLimit: 500000,      // Higher gas for deployment
        _isCreation: true,      // Flag as creation
        _data: initCode        // Contract bytecode
    });
}
```

---

## Troubleshooting

### Error: `OptimismPortal_OnlyCustomGasToken`

**Problem:** Trying to use `depositERC20Transaction` on a non-CGT chain.

**Solution:** Check if the chain uses CGT:
```solidity
require(
    systemConfig.isFeatureEnabled(Features.CUSTOM_GAS_TOKEN),
    "Chain does not use CGT"
);
```

### Error: `OptimismPortal_NotAllowedOnCGTMode`

**Problem:** Trying to use `depositTransaction` with `msg.value > 0` on a CGT chain.

**Solution:** Use `depositERC20Transaction` instead:
```solidity
// ❌ Wrong on CGT chain
portal.depositTransaction{value: 1 ether}(...);

// ✅ Correct on CGT chain
portal.depositERC20Transaction(...);
```

### Error: `ERC20: insufficient allowance`

**Problem:** Portal doesn't have approval to transfer tokens.

**Solution:** Approve before depositing:
```solidity
IERC20(token).approve(portal, amount);
portal.depositERC20Transaction(...);
```

### Error: `DepositedOKBAdapter_TransferNotAllowed`

**Problem:** Trying to transfer dOKB tokens.

**Solution:** dOKB tokens cannot be transferred. They can only be used by the portal for deposits. Don't try to send them to other addresses.

### Transaction Reverts with No Reason

**Check these common issues:**

1. **Token decimals:** CGT must have 18 decimals
2. **Gas limit:** Must be >= `minimumGasLimit(data.length)`
3. **Feature flag:** Ensure CGT feature is enabled
4. **Token address:** Verify correct token address in storage

---

## Additional Resources

- **Design Document:** `/custom-gas-token.md`
- **Implementation Summary:** `/IMPLEMENTATION_SUMMARY.md`
- **Architecture Diagrams:** `/ARCHITECTURE_DIAGRAM.md`
- **Source Code:**
  - Portal: `packages/contracts-bedrock/src/L1/OptimismPortal2.sol`
  - Adapter: `packages/contracts-bedrock/src/L1/DepositedOKBAdapter.sol`
  - L1Block: `packages/contracts-bedrock/src/L2/L1BlockCGT.sol`

## Support

For issues or questions:
1. Check the troubleshooting section above
2. Review the test files for examples
3. Read the inline documentation in the source code
4. Consult the OP Stack documentation

---

**Last Updated:** October 8, 2025
**Version:** 1.0.0
**Compatible with:** OP Stack v5.2.0+
