# Custom Gas Token (CGT) Implementation Summary

This document summarizes the implementation of the custom gas token design using a generic lock and mint pattern for the OP Stack.

## Overview

The implementation follows the design specification to enable custom gas tokens (CGT) in the OP Stack, with a focus on:
- Generic lock-and-mint pattern for CGT deposits
- Maintaining backward compatibility with ETH-based chains
- Enabling OKB burning for strict 21 million supply cap

## Changes Implemented

### L1 Changes

#### 1. **OptimismPortal2.sol** - Main Portal Contract

**New Functions:**

- `gasPayingToken() public view returns (address, uint8)`
  - Returns the gas paying token address and decimals (must be 18 decimals for CGT)
  - Uses the `GasPayingToken` library to read from magic storage slots

- `depositERC20Transaction(...) public metered(_gasLimit)`
  - Accepts ERC20 token deposits for CGT chains
  - Transfers custom gas token from user via `transferFrom` (requires prior approval)
  - Emits `TransactionDeposited` event for L2 derivation
  - Reverts with `OptimismPortal_OnlyCustomGasToken` if called on non-CGT chain

**Modified Functions:**

- `depositTransaction(...) public payable metered(_gasLimit)`
  - Now reverts with `OptimismPortal_NotAllowedOnCGTMode` if `msg.value > 0` on CGT chains
  - Preserves functionality for cross-chain calls without value

**New Errors:**
- `OptimismPortal_OnlyCustomGasToken` - Thrown when trying to use `depositERC20Transaction` on a non-CGT chain

**New Imports:**
- `GasPayingToken` library for reading token information from storage
- `IERC20` from OpenZeppelin for ERC20 token transfers

#### 2. **DepositedOKBAdapter.sol** - OKB Burning Adapter

A specialized ERC20 adapter contract that enables burning OKB tokens on L1 and depositing equivalent amounts to L2.

**Key Features:**
- Burns OKB by transferring to `address(0)`
- Mints deposit tokens (dOKB) that are locked to the contract
- Only allows transfers to/from the OptimismPortal
- Automatically initiates L2 deposit transactions
- Pre-approves portal for `type(uint256).max` tokens

**Functions:**
- `deposit(address _to, uint256 _amount, uint64 _gasLimit, bool _isCreation, bytes memory _data)`
  - Full deposit with custom parameters
- `deposit(address _to, uint256 _amount)`
  - Convenience function with default gas limit (100,000)

**Transfer Restrictions:**
- Overrides `transfer()` and `transferFrom()` to only allow portal operations
- Prevents token trading or transfers outside of the deposit flow
- Enables refunds back to adapter in case of failed deposits

**Immutable References:**
- `PORTAL` - OptimismPortal2 address
- `OKB` - OKB token address

### L2 Changes

#### 3. **L1BlockCGT.sol** - Custom Gas Token L1 Block Attributes

**Modified Functions:**

- `gasPayingTokenName() public view returns (string)`
  - Now reads directly from storage using `GasPayingToken.getName()`
  - Removes dependency on `LiquidityController` predeploy

- `gasPayingTokenSymbol() public view returns (string)`
  - Now reads directly from storage using `GasPayingToken.getSymbol()`
  - Removes dependency on `LiquidityController` predeploy

**Removed Dependencies:**
- No longer imports `ILiquidityController` interface
- Token name and symbol now stored directly in the contract's storage

**Preserved Functions:**
- `isCustomGasToken()` - Still reads from `IS_CUSTOM_GAS_TOKEN_SLOT`
- `setCustomGasToken()` - Still callable by depositor account only
- `gasPayingToken()` - Still reverts with deprecation message

### Interface Changes

#### 4. **IOptimismPortal2.sol** - Portal Interface

**New Functions:**
- `gasPayingToken() external view returns (address address_, uint8 decimals_)`
- `depositERC20Transaction(address _to, uint256 _mint, uint256 _value, uint64 _gasLimit, bool _isCreation, bytes memory _data) external`

**New Errors:**
- `OptimismPortal_OnlyCustomGasToken()`

## Design Patterns

### Lock and Mint Pattern

The implementation uses a generic lock-and-mint pattern:

1. **L1 Deposit Flow:**
   ```
   User approves Portal for ERC20 tokens
   → User calls depositERC20Transaction()
   → Portal transfers tokens from user via transferFrom()
   → Portal emits TransactionDeposited event
   → Rollup node derives deposit transaction
   → L2 mints equivalent tokens to user
   ```

2. **OKB Burning Flow (via DepositedOKBAdapter):**
   ```
   User approves Adapter for OKB tokens
   → User calls Adapter.deposit()
   → Adapter transfers OKB from user
   → Adapter burns OKB to address(0)
   → Adapter mints dOKB to itself
   → Adapter calls Portal.depositERC20Transaction()
   → Portal transfers dOKB from Adapter
   → L2 mints equivalent tokens to user
   ```

### Storage Pattern

Uses the `GasPayingToken` library for consistent storage access:
- `GAS_PAYING_TOKEN_SLOT` - Stores address and decimals
- `GAS_PAYING_TOKEN_NAME_SLOT` - Stores token name
- `GAS_PAYING_TOKEN_SYMBOL_SLOT` - Stores token symbol

These slots are shared between L1 (Portal) and L2 (L1Block) for consistency.

## Security Considerations

1. **Transfer Restrictions:** DepositedOKBAdapter strictly limits token transfers to prevent misuse
2. **Approval Management:** Adapter pre-approves portal to avoid per-transaction approvals
3. **Burning Mechanism:** OKB is burned to address(0) to enforce supply cap
4. **Feature Flags:** CGT mode is checked via SystemConfig feature flags
5. **Backward Compatibility:** ETH deposits still work via `depositTransaction()` on non-CGT chains

## Contracts Dropped (per Design)

As specified in the design document, the following L2 contracts are no longer needed:
- `LiquidityController` - Token name/symbol now in L1BlockCGT directly
- `NativeAssetLiquidity` - Not needed for lock-and-mint pattern

## Contracts Kept

The following L2 contracts are preserved:
- `L2ToL1MessagePasserCGT` - Prevents withdrawals with value on CGT chains
- `L1BlockCGT` - Now stores token name/symbol directly

## Testing Recommendations

1. **Portal Tests:**
   - Test `depositERC20Transaction` on CGT chains
   - Verify `depositTransaction` reverts with value on CGT chains
   - Test `gasPayingToken()` returns correct address and decimals

2. **Adapter Tests:**
   - Verify OKB burning mechanism
   - Test transfer restrictions
   - Verify automatic portal deposits
   - Test refund flows on failed deposits

3. **L2 Tests:**
   - Verify token name/symbol reads from storage
   - Test CGT flag setting and reading
   - Verify withdrawal restrictions on CGT chains

4. **Integration Tests:**
   - End-to-end deposit flow from L1 to L2
   - OKB burning and L2 minting
   - Cross-chain message passing on CGT chains

## Migration Path

For existing chains to adopt this design:

1. Deploy new `DepositedOKBAdapter` contract
2. Set adapter address as `gasPayingToken` in Portal storage
3. Enable `CUSTOM_GAS_TOKEN` feature flag in SystemConfig
4. Update L1Block predeploy to L1BlockCGT
5. Set token name/symbol in L1BlockCGT storage
6. Call `setCustomGasToken()` to activate CGT mode on L2

## Benefits of This Design

1. **Generic:** Works for any ERC20 token with 18 decimals
2. **Secure:** Follows OP Stack security model and patterns
3. **Flexible:** Supports both lock-and-mint and burn-and-mint
4. **Compatible:** Maintains backward compatibility with ETH chains
5. **Upgradeable:** Easy to upgrade alongside OP Stack
6. **Auditable:** Uses standard ERC20 patterns and interfaces

## References

- Design Document: `/custom-gas-token.md`
- GasPayingToken Library: `src/libraries/GasPayingToken.sol`
- Features Library: `src/libraries/Features.sol`
- OpenZeppelin ERC20: `@openzeppelin/contracts/token/ERC20/ERC20.sol`
