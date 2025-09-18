# CGT Cross-Chain Testing Scripts

Testing scripts for Custom Gas Token (CGT) cross-chain functionality in Optimism with **WOKB Auto-Unwrap** feature.

## Key Features

### 🚀 WOKB Auto-Unwrap Technology
- **L1→L2**: WOKB automatically unwraps to OKB on L2, providing seamless cross-chain OKB transfer
- **L2→L1**: Standard lock/mint mechanism for L2→L1 withdrawals
- **One Transaction**: L1 WOKB → L2 OKB in a single cross-chain transaction

## Usage

### Prerequisites
- L1 and L2 nodes running (localhost:8545 and localhost:8123)
- Foundry/Cast installed
- jq installed
- Go installed (for withdrawal tool)

### Step 1: Deploy WOKB Contracts
```bash
./deploy_wokb_cgt.sh
```
Deploys complete WOKB cross-chain infrastructure:
- L2 WOKB contract (custom implementation with auto-unwrap)
- L1 OptimismMintableERC20 (represents L2 WOKB)
- Updates `.env` file with both addresses

### Step 2: Test L2→L1 Withdrawal (Complete Flow)
```bash
./test_cross_chain_1_cgt.sh
```
Tests complete withdrawal flow:
1. Convert 10 OKB → WOKB on L2
2. **Lock** 5 WOKB on L2 → Initiate withdrawal to L1
3. Prove withdrawal on L1 (takes 1-2 minutes)
4. **Mint** 5 WOKB on L1 (after 20s challenge period)

### Step 3: Test L1→L2 Deposit (Auto-Unwrap)
```bash
./test_cross_chain_2_cgt.sh
```
Tests L1→L2 deposit with **auto-unwrap**:
1. **Burn** 2 WOKB on L1 → **Auto-unwrap to OKB** on L2 (takes 2-5 minutes)
2. ✅ **No manual conversion needed** - WOKB automatically becomes OKB!

## Cross-Chain Flow Comparison

### L2→L1 (Standard Lock/Mint)
```
L2: 5 WOKB → Lock → L1: 5 WOKB (minted)
```

### L1→L2 (Auto-Unwrap) ⭐
```
L1: 2 WOKB → Burn → L2: 2 OKB (auto-unwrapped)
```

## Expected Timing
- **L2→L1 withdrawal**: ~3 minutes total (includes 20s challenge period)
- **L1→L2 deposit**: ~3-5 minutes (cross-chain event processing + auto-unwrap)

## Requirements
- `.env` file with RPC URLs and private keys
- L1 and L2 nodes must be running
- WOKB contracts must be deployed first
- `~/go/bin` in PATH (for withdrawal tool)

## Technical Details

### WOKB Contract Features
- **Auto-Unwrap**: When bridge calls `transfer()`, WOKB automatically unwraps to OKB
- **Dynamic Naming**: Uses L1Block to get native token name/symbol
- **Bridge Integration**: Seamlessly works with Optimism's StandardBridge

### Script Features
- **Automatic Tool Building**: Withdrawal tool built and installed to `~/go/bin/`
- **Real-time Verification**: Balance changes monitored throughout process
- **Error Handling**: Comprehensive error checking and user feedback
- **Environment Management**: Automatic `.env` file updates with contract addresses

## Notes
- Scripts automatically build and install required tools
- Balance changes are verified at each step
- All contract addresses are read from config files
- WOKB auto-unwrap provides superior UX for L1→L2 transfers
