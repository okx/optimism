# CGT Cross-Chain Testing Scripts

Testing scripts for Custom Gas Token (CGT) cross-chain functionality in Optimism.

## Usage

### Prerequisites
- L1 and L2 nodes running (localhost:8545 and localhost:8123)
- Foundry/Cast installed
- jq installed

### Step 1: Deploy WOKB Contract
```bash
./deploy_wokb_cgt.sh
```
Deploys L1 WOKB contract and updates `.env` file with the address.

### Step 2: Test L2→L1 Withdrawal (Complete Flow)
```bash
./test_cross_chain_1_cgt.sh
```
Tests complete withdrawal flow:
1. Convert 10 OKB → WOKB on L2
2. **Burn** 5 WOKB on L2 → Initiate withdrawal to L1
3. Prove withdrawal on L1 (takes 1-2 minutes)
4. **Mint** 5 WOKB on L1 (after 20s challenge period)

### Step 3: Test L1→L2 Deposit
```bash
./test_cross_chain_2_cgt.sh
```
Tests L1→L2 deposit flow:
1. **Lock** 2 WOKB on L1 → **Mint** 2 WOKB on L2 (takes 2-5 minutes)
2. Convert remaining WOKB → OKB on L2

## Expected Timing
- **L2→L1 withdrawal**: ~3 minutes total (includes 20s challenge period)
- **L1→L2 deposit**: ~3-5 minutes (cross-chain event processing)

## Requirements
- `.env` file with RPC URLs and private keys
- L1 and L2 nodes must be running
- WOKB contract must be deployed first

## Notes
- Scripts automatically build required tools
- Balance changes are verified at each step
- All contract addresses are read from config files
